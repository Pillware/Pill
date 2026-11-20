// Roslyn plumbing that lets an in-process compile behave like csc.exe.
//
// Responsibilities:
// - Load analyzer and source-generator assemblies into this plugin's context.
// - Replay the `/analyzerconfig` files MSBuild passed, so diagnostic severities
//   and generator-visible build properties match the real build.
// - Adapt captured `AdditionalFiles` to the shape analyzers expect.
//
// Design:
// Roslyn exposes the analyzer-config machinery (AnalyzerConfigSet) publicly but
// keeps csc's own adapters internal, so the four small types here exist only to
// bridge that gap. Without them the compilation would still succeed, but every
// severity configured in an .editorconfig or in the SDK's
// analysislevel_8_default.globalconfig would be ignored - a hot reload would
// then disagree with the build about which rules are errors, which is exactly
// the class of divergence this whole path must not introduce.

using System.Collections.Immutable;
using System.Reflection;
using System.Runtime.Loader;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Diagnostics;
using Microsoft.CodeAnalysis.Text;

namespace TracyLive.Compiler;

// =============================================================================
// Analyzer assembly loading
// =============================================================================

/// <summary>Loads analyzer and generator assemblies beside this plugin.</summary>
///
/// <remarks>
/// Roslyn requires an <see cref="IAnalyzerAssemblyLoader"/> but ships no public
/// implementation. Loading into the context that already holds this assembly is
/// what makes an analyzer's own Microsoft.CodeAnalysis reference bind to the
/// Roslyn already running here, rather than pulling a second copy in: two copies
/// would give the analyzer interfaces different type identities and every
/// analyzer would silently fail to load.
///
/// Analyzers are never unloaded. They do not change between hot reloads - only
/// gameplay source does - so keeping them loaded is both correct and what makes
/// the second reload cheaper than the first.
/// </remarks>
internal sealed class PluginAnalyzerAssemblyLoader : IAnalyzerAssemblyLoader
{
    /// <summary>The context holding this assembly and its Roslyn dependencies.</summary>
    private static readonly AssemblyLoadContext HostContext =
        AssemblyLoadContext.GetLoadContext(typeof(PluginAnalyzerAssemblyLoader).Assembly)
        ?? AssemblyLoadContext.Default;

    /// <summary>Known analyzer file paths, keyed by assembly simple name.</summary>
    private readonly Dictionary<string, string> _pathsBySimpleName =
        new(StringComparer.OrdinalIgnoreCase);

    /// <summary>Assemblies already loaded, keyed by full path.</summary>
    private readonly Dictionary<string, Assembly> _loadedByPath =
        new(StringComparer.OrdinalIgnoreCase);

    internal PluginAnalyzerAssemblyLoader()
    {
        // An analyzer may depend on another analyzer assembly that Roslyn never
        // asks for by path. This resolves those by simple name from the same set
        // of locations Roslyn declared.
        HostContext.Resolving += ResolveDependency;
    }

    /// <summary>Record a path Roslyn may later need to resolve by name.</summary>
    public void AddDependencyLocation(string fullPath)
    {
        string simpleName = Path.GetFileNameWithoutExtension(fullPath);
        lock (_pathsBySimpleName)
        {
            _pathsBySimpleName[simpleName] = fullPath;
        }
    }

    /// <summary>Load one analyzer assembly, at most once per path.</summary>
    public Assembly LoadFromPath(string fullPath)
    {
        lock (_loadedByPath)
        {
            if (_loadedByPath.TryGetValue(fullPath, out Assembly? already))
            {
                return already;
            }
            Assembly loaded = HostContext.LoadFromAssemblyPath(fullPath);
            _loadedByPath[fullPath] = loaded;
            return loaded;
        }
    }

    /// <summary>Resolve an analyzer's own dependency from the recorded paths.</summary>
    ///
    /// <remarks>
    /// Returning null hands the request back to the context's normal resolution,
    /// which is what lets Microsoft.CodeAnalysis bind to the copy already here.
    /// </remarks>
    private Assembly? ResolveDependency(AssemblyLoadContext context, AssemblyName name)
    {
        if (name.Name is null)
        {
            return null;
        }
        string? path;
        lock (_pathsBySimpleName)
        {
            if (!_pathsBySimpleName.TryGetValue(name.Name, out path))
            {
                return null;
            }
        }
        return File.Exists(path) ? LoadFromPath(path) : null;
    }
}

// =============================================================================
// Analyzer config options
// =============================================================================

/// <summary>One section of resolved analyzer-config key/value pairs.</summary>
internal sealed class CapturedAnalyzerConfigOptions : AnalyzerConfigOptions
{
    /// <summary>Empty options, used for any file the config set does not cover.</summary>
    internal static readonly CapturedAnalyzerConfigOptions Empty =
        new(ImmutableDictionary<string, string>.Empty);

    private readonly ImmutableDictionary<string, string> _values;

    internal CapturedAnalyzerConfigOptions(ImmutableDictionary<string, string> values) =>
        _values = values;

    public override bool TryGetValue(string key, out string value) =>
        _values.TryGetValue(key, out value!);

    public override IEnumerable<string> Keys => _values.Keys;
}

/// <summary>Serves per-file analyzer-config options from the captured set.</summary>
///
/// <remarks>
/// This is what carries `build_property.*` entries from the SDK's generated
/// editorconfig through to source generators, so a generator that reads an
/// MSBuild property sees the same value it saw during the real build.
/// </remarks>
internal sealed class CapturedAnalyzerConfigOptionsProvider : AnalyzerConfigOptionsProvider
{
    private readonly AnalyzerConfigSet _configSet;

    /// <summary>Resolved options per source path; a compile touches each repeatedly.</summary>
    private readonly Dictionary<string, CapturedAnalyzerConfigOptions> _cache =
        new(StringComparer.OrdinalIgnoreCase);

    private readonly CapturedAnalyzerConfigOptions _globalOptions;

    internal CapturedAnalyzerConfigOptionsProvider(AnalyzerConfigSet configSet)
    {
        _configSet = configSet;
        _globalOptions =
            new CapturedAnalyzerConfigOptions(configSet.GlobalConfigOptions.AnalyzerOptions);
    }

    public override AnalyzerConfigOptions GlobalOptions => _globalOptions;

    public override AnalyzerConfigOptions GetOptions(SyntaxTree tree) => ForPath(tree.FilePath);

    public override AnalyzerConfigOptions GetOptions(AdditionalText textFile) =>
        ForPath(textFile.Path);

    /// <summary>Resolve and memoize the options that apply to one path.</summary>
    private CapturedAnalyzerConfigOptions ForPath(string path)
    {
        if (string.IsNullOrEmpty(path))
        {
            return CapturedAnalyzerConfigOptions.Empty;
        }
        lock (_cache)
        {
            if (_cache.TryGetValue(path, out CapturedAnalyzerConfigOptions? cached))
            {
                return cached;
            }
            var resolved = new CapturedAnalyzerConfigOptions(
                _configSet.GetOptionsForSourcePath(path).AnalyzerOptions);
            _cache[path] = resolved;
            return resolved;
        }
    }
}

/// <summary>Applies configured diagnostic severities to the compilation.</summary>
///
/// <remarks>
/// Separate from the options provider above because Roslyn asks two different
/// questions: analyzers read key/value options, while the compiler asks whether
/// a given diagnostic id was reconfigured for a given file. Only this second one
/// can turn a rule into an error, which is why a compile without it can accept
/// source that the real build rejects.
/// </remarks>
internal sealed class CapturedSyntaxTreeOptionsProvider : SyntaxTreeOptionsProvider
{
    private readonly AnalyzerConfigSet _configSet;
    private readonly ImmutableDictionary<string, ReportDiagnostic> _globalTreeOptions;

    /// <summary>Per-path severity maps, memoized for the duration of a compile.</summary>
    private readonly Dictionary<string, AnalyzerConfigOptionsResult> _cache =
        new(StringComparer.OrdinalIgnoreCase);

    internal CapturedSyntaxTreeOptionsProvider(AnalyzerConfigSet configSet)
    {
        _configSet = configSet;
        _globalTreeOptions = configSet.GlobalConfigOptions.TreeOptions;
    }

    public override GeneratedKind IsGenerated(SyntaxTree tree, CancellationToken cancellationToken)
    {
        // `generated_code` in an editorconfig is how a build marks a file as
        // generated; anything else is left for Roslyn's own heuristics.
        AnalyzerConfigOptionsResult resolved = ForPath(tree.FilePath);
        if (!resolved.AnalyzerOptions.TryGetValue("generated_code", out string? value))
        {
            return GeneratedKind.Unknown;
        }
        if (string.Equals(value, "true", StringComparison.OrdinalIgnoreCase))
        {
            return GeneratedKind.MarkedGenerated;
        }
        return string.Equals(value, "false", StringComparison.OrdinalIgnoreCase)
            ? GeneratedKind.NotGenerated
            : GeneratedKind.Unknown;
    }

    public override bool TryGetDiagnosticValue(
        SyntaxTree tree,
        string diagnosticId,
        CancellationToken cancellationToken,
        out ReportDiagnostic severity) =>
        ForPath(tree.FilePath).TreeOptions.TryGetValue(diagnosticId, out severity);

    public override bool TryGetGlobalDiagnosticValue(
        string diagnosticId,
        CancellationToken cancellationToken,
        out ReportDiagnostic severity) =>
        _globalTreeOptions.TryGetValue(diagnosticId, out severity);

    /// <summary>Resolve and memoize the config result that applies to one path.</summary>
    private AnalyzerConfigOptionsResult ForPath(string path)
    {
        lock (_cache)
        {
            if (_cache.TryGetValue(path, out AnalyzerConfigOptionsResult cached))
            {
                return cached;
            }
            AnalyzerConfigOptionsResult resolved = _configSet.GetOptionsForSourcePath(path);
            _cache[path] = resolved;
            return resolved;
        }
    }
}

// =============================================================================
// Additional files
// =============================================================================

/// <summary>An `AdditionalFiles` entry, read from disk on demand.</summary>
internal sealed class CapturedAdditionalText : AdditionalText
{
    private readonly SourceHashAlgorithm _checksumAlgorithm;

    internal CapturedAdditionalText(string path, SourceHashAlgorithm checksumAlgorithm)
    {
        Path = path;
        _checksumAlgorithm = checksumAlgorithm;
    }

    public override string Path { get; }

    /// <summary>Read the file, or report nothing when it cannot be read.</summary>
    ///
    /// <remarks>
    /// A missing additional file is the analyzer's problem to diagnose, not a
    /// reason to fail the compile before analyzers have run.
    /// </remarks>
    public override SourceText? GetText(CancellationToken cancellationToken = default)
    {
        try
        {
            using FileStream stream = File.OpenRead(Path);
            return SourceText.From(stream, checksumAlgorithm: _checksumAlgorithm);
        }
        catch (IOException)
        {
            return null;
        }
        catch (UnauthorizedAccessException)
        {
            return null;
        }
    }
}
