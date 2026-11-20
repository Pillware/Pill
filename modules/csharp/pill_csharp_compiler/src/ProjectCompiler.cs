// In-process Roslyn compilation of a gameplay project, for hot reload.
//
// Responsibilities:
// - Replay a captured csc command line through Roslyn inside the host process.
// - Run the project's source generators and analyzers exactly as the build does.
// - Emit the project assembly where the collectible managed loader watches.
//
// Design:
// A `dotnet build` of a one-file gameplay edit costs about three seconds cold
// and one second warm, of which the actual C# compile is around thirty-five
// milliseconds; the rest is process start, MSBuild evaluation, NuGet restore and
// a walk of the project reference graph, all of which recompute an answer that
// did not change since the host started. This class skips all of it by replaying
// the compiler invocation the startup build already reported, which is what
// keeps the fast path faithful rather than approximate: the references, defines,
// language version, analyzers and source list are MSBuild's own, not a guess.
//
// Everything expensive is cached across reloads and keyed on the response file's
// timestamp - metadata references, analyzer instances, the generator driver's
// incremental state. A reload that only changes a method body therefore reuses
// all of it, which is where the remaining cost goes from tens of milliseconds to
// a handful.
//
// Nothing here holds a file handle open. References are read as bytes rather
// than memory-mapped, so a concurrent `dotnet build` can always replace any
// assembly this has read; the whole reference set is about six megabytes.

using System.Collections.Immutable;
using System.Globalization;
using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Diagnostics;
using Microsoft.CodeAnalysis.Emit;
using Microsoft.CodeAnalysis.Text;

namespace TracyLive.Compiler;

// =============================================================================
// Results
// =============================================================================

/// <summary>What one compile attempt produced.</summary>
internal enum CompileStatus
{
    /// <summary>The assembly was compiled and written.</summary>
    Compiled = 0,

    /// <summary>The project has compile or analyzer errors; nothing was written.</summary>
    Failed = 1,

    /// <summary>The fast path could not run at all; the caller must fall back.</summary>
    Unavailable = 2,
}

/// <summary>One compile attempt's status and human-readable detail.</summary>
internal readonly struct CompileOutcome
{
    internal CompileOutcome(CompileStatus status, string detail)
    {
        Status = status;
        Detail = detail;
    }

    internal CompileStatus Status { get; }

    /// <summary>Formatted diagnostics, or the reason the fast path is unusable.</summary>
    internal string Detail { get; }
}

// =============================================================================
// ProjectCompiler
// =============================================================================

/// <summary>Compiles a gameplay project from a captured csc command line.</summary>
internal static class ProjectCompiler
{
    /// <summary>Serializes compiles against the shared caches below.</summary>
    ///
    /// <remarks>
    /// The host compiles from its main loop, but warmup runs on a thread pool
    /// thread so that starting the host does not wait for Roslyn to JIT. Those
    /// two can overlap exactly once, on the first reload after a fast startup.
    /// </remarks>
    private static readonly object Gate = new();

    /// <summary>Most diagnostics reported back to the host in one failure.</summary>
    ///
    /// <remarks>
    /// One bad edit can produce hundreds of cascading errors. The first handful
    /// are the ones that explain the failure; the rest would only push the real
    /// cause out of the console scrollback.
    /// </remarks>
    private const int MaxReportedDiagnostics = 25;

    // ---- Cached plan, rebuilt whenever the captured command line changes ----

    private static string? _planResponseFile;
    private static DateTime _planWriteUtc;
    private static CSharpCommandLineArguments? _arguments;
    private static string _planBaseDirectory = "";
    private static ImmutableArray<DiagnosticAnalyzer> _analyzers =
        ImmutableArray<DiagnosticAnalyzer>.Empty;
    private static ImmutableArray<AdditionalText> _additionalFiles =
        ImmutableArray<AdditionalText>.Empty;
    private static CapturedAnalyzerConfigOptionsProvider? _analyzerConfigOptions;
    private static CapturedSyntaxTreeOptionsProvider? _syntaxTreeOptions;
    private static GeneratorDriver? _generatorDriver;
    private static PluginAnalyzerAssemblyLoader? _analyzerLoader;
    private static readonly List<string> _analyzerLoadFailures = new();

    /// <summary>Reference images keyed by path, with the timestamp they were read at.</summary>
    private static readonly Dictionary<string, CachedReference> _references =
        new(StringComparer.OrdinalIgnoreCase);

    /// <summary>One cached metadata reference and the file version behind it.</summary>
    private readonly struct CachedReference
    {
        internal CachedReference(DateTime writeUtc, long length, PortableExecutableReference reference)
        {
            WriteUtc = writeUtc;
            Length = length;
            Reference = reference;
        }

        internal DateTime WriteUtc { get; }
        internal long Length { get; }
        internal PortableExecutableReference Reference { get; }
    }

    // =========================================================================
    // Entry points
    // =========================================================================

    /// <summary>Load the plan and run one throwaway compilation, without emitting.</summary>
    ///
    /// <remarks>
    /// Called once at startup so the first real reload does not pay for JITting
    /// Roslyn, reading the reference set and loading the analyzers - together
    /// close to a second, which would otherwise land on the first edit the
    /// developer makes and look exactly like the slowness this path removes.
    /// </remarks>
    internal static CompileOutcome Warmup(string responseFilePath) =>
        Run(responseFilePath, outputAssemblyPath: null);

    /// <summary>Compile the project and write the assembly, plus its symbols.</summary>
    internal static CompileOutcome Compile(string responseFilePath, string outputAssemblyPath) =>
        Run(responseFilePath, outputAssemblyPath);

    // =========================================================================
    // Compilation
    // =========================================================================

    /// <summary>Run the whole pipeline; a null output path stops before emitting.</summary>
    private static CompileOutcome Run(string responseFilePath, string? outputAssemblyPath)
    {
        lock (Gate)
        {
            try
            {
                CompileOutcome? planFailure = EnsurePlan(responseFilePath);
                if (planFailure is not null)
                {
                    return planFailure.Value;
                }

                CSharpCommandLineArguments arguments = _arguments!;

                // Step 1: Read and parse every source file the build compiled.
                // Parsing is the one step with no useful cross-reload cache: the
                // file that changed is precisely the one being reloaded, and the
                // others are small enough that re-parsing them costs less than
                // tracking which ones went stale.
                var syntaxTrees = new List<SyntaxTree>(arguments.SourceFiles.Length);
                foreach (CommandLineSourceFile sourceFile in arguments.SourceFiles)
                {
                    SyntaxTree? tree = ParseSourceFile(sourceFile.Path, arguments);
                    if (tree is null)
                    {
                        return new CompileOutcome(
                            CompileStatus.Unavailable,
                            $"could not read source file {sourceFile.Path}");
                    }
                    syntaxTrees.Add(tree);
                }

                // Step 2: Resolve the reference set, reusing images whose file on
                // disk has not changed since the last compile.
                var references = new List<MetadataReference>(arguments.MetadataReferences.Length);
                foreach (CommandLineReference reference in arguments.MetadataReferences)
                {
                    PortableExecutableReference? resolved = ResolveReference(reference);
                    if (resolved is null)
                    {
                        return new CompileOutcome(
                            CompileStatus.Unavailable,
                            $"could not read referenced assembly {reference.Reference}");
                    }
                    references.Add(resolved);
                }

                // Step 3: Build the compilation under the build's own options,
                // with the severity configuration the analyzer configs declared.
                CSharpCompilationOptions options = arguments.CompilationOptions
                    .WithSyntaxTreeOptionsProvider(_syntaxTreeOptions)
                    .WithSourceReferenceResolver(new SourceFileResolver(
                        ImmutableArray<string>.Empty, _planBaseDirectory));

                CSharpCompilation compilation = CSharpCompilation.Create(
                    arguments.CompilationName,
                    syntaxTrees,
                    references,
                    options);

                // Step 4: Run the source generators. The driver is kept between
                // reloads so its incremental state survives: an edit that does
                // not touch a generator's inputs reuses the previous output
                // rather than regenerating it.
                var generatorDiagnostics = ImmutableArray<Diagnostic>.Empty;
                Compilation finalCompilation = compilation;
                if (_generatorDriver is not null)
                {
                    _generatorDriver = _generatorDriver.RunGeneratorsAndUpdateCompilation(
                        compilation,
                        out finalCompilation,
                        out generatorDiagnostics);
                }

                // Step 5: Collect compiler and analyzer diagnostics together.
                ImmutableArray<Diagnostic> diagnostics =
                    CollectDiagnostics(finalCompilation, generatorDiagnostics);
                if (HasError(diagnostics))
                {
                    return new CompileOutcome(CompileStatus.Failed, Format(diagnostics));
                }

                // A warmup has now done everything a real compile does except
                // produce bytes, which is the point: the next call is warm.
                if (outputAssemblyPath is null)
                {
                    return new CompileOutcome(CompileStatus.Compiled, "");
                }

                return Emit(finalCompilation, arguments, outputAssemblyPath);
            }
            catch (Exception failure)
            {
                // Any escape here means the fast path is broken rather than the
                // gameplay source, so the host must fall back to a full build
                // instead of reporting a compile error the developer cannot fix.
                return new CompileOutcome(
                    CompileStatus.Unavailable,
                    $"in-process compilation failed: {failure}");
            }
        }
    }

    /// <summary>Read and parse one source file, or report that it is unreadable.</summary>
    private static SyntaxTree? ParseSourceFile(string path, CSharpCommandLineArguments arguments)
    {
        SourceText text;
        try
        {
            using FileStream stream = File.Open(
                path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            text = SourceText.From(
                stream,
                arguments.Encoding ?? Encoding.UTF8,
                arguments.ChecksumAlgorithm);
        }
        catch (IOException)
        {
            return null;
        }
        catch (UnauthorizedAccessException)
        {
            return null;
        }
        return CSharpSyntaxTree.ParseText(text, arguments.ParseOptions, path);
    }

    /// <summary>Resolve one reference, reading it only when its file changed.</summary>
    ///
    /// <remarks>
    /// The image is read into memory rather than memory-mapped so that nothing
    /// here can block a `dotnet build` from replacing a referenced assembly -
    /// most of all csharp_runtime.dll, which the engine's own build rewrites.
    /// </remarks>
    private static PortableExecutableReference? ResolveReference(CommandLineReference reference)
    {
        string path = reference.Reference;
        FileInfo file;
        try
        {
            file = new FileInfo(path);
            if (!file.Exists)
            {
                return null;
            }
        }
        catch (ArgumentException)
        {
            // A malformed path in the capture, rather than a missing file.
            return null;
        }
        catch (PathTooLongException)
        {
            return null;
        }
        catch (NotSupportedException)
        {
            return null;
        }

        if (_references.TryGetValue(path, out CachedReference cached)
            && cached.WriteUtc == file.LastWriteTimeUtc
            && cached.Length == file.Length)
        {
            return cached.Reference;
        }

        byte[] image;
        try
        {
            image = File.ReadAllBytes(path);
        }
        catch (IOException)
        {
            return null;
        }
        catch (UnauthorizedAccessException)
        {
            return null;
        }

        PortableExecutableReference resolved = MetadataReference.CreateFromImage(
            image, reference.Properties, filePath: path);
        _references[path] = new CachedReference(file.LastWriteTimeUtc, file.Length, resolved);
        return resolved;
    }

    /// <summary>Run analyzers if any are loaded, and merge every diagnostic source.</summary>
    private static ImmutableArray<Diagnostic> CollectDiagnostics(
        Compilation compilation,
        ImmutableArray<Diagnostic> generatorDiagnostics)
    {
        ImmutableArray<Diagnostic> compilationDiagnostics;
        if (_analyzers.IsEmpty)
        {
            compilationDiagnostics = compilation.GetDiagnostics();
        }
        else
        {
            // Non-null whenever a plan is loaded, which every caller of this
            // has already established.
            var analyzerOptions = new AnalyzerOptions(_additionalFiles, _analyzerConfigOptions!);
            CompilationWithAnalyzers withAnalyzers =
                compilation.WithAnalyzers(_analyzers, analyzerOptions);
            // One pass rather than GetDiagnostics() plus a separate analyzer run:
            // both bind the whole compilation, and doing it twice doubles the
            // most expensive step of the compile.
            compilationDiagnostics =
                withAnalyzers.GetAllDiagnosticsAsync().GetAwaiter().GetResult();
        }
        return generatorDiagnostics.IsEmpty
            ? compilationDiagnostics
            : compilationDiagnostics.AddRange(generatorDiagnostics);
    }

    /// <summary>Whether any diagnostic blocks the build.</summary>
    private static bool HasError(ImmutableArray<Diagnostic> diagnostics)
    {
        foreach (Diagnostic diagnostic in diagnostics)
        {
            if (!diagnostic.IsSuppressed && diagnostic.Severity == DiagnosticSeverity.Error)
            {
                return true;
            }
        }
        return false;
    }

    /// <summary>Format the blocking diagnostics the way the compiler prints them.</summary>
    private static string Format(ImmutableArray<Diagnostic> diagnostics)
    {
        var builder = new StringBuilder();
        int reported = 0;
        int suppressedCount = 0;
        foreach (Diagnostic diagnostic in diagnostics)
        {
            if (diagnostic.IsSuppressed || diagnostic.Severity != DiagnosticSeverity.Error)
            {
                continue;
            }
            if (reported == MaxReportedDiagnostics)
            {
                suppressedCount++;
                continue;
            }
            builder.AppendLine(
                CSharpDiagnosticFormatter.Instance.Format(diagnostic, CultureInfo.InvariantCulture));
            reported++;
        }
        if (suppressedCount > 0)
        {
            builder.AppendLine($"... and {suppressedCount} more error(s)");
        }
        return builder.ToString();
    }

    /// <summary>Emit the assembly and its symbols, then move both into place.</summary>
    ///
    /// <remarks>
    /// Written to temporary files beside the destination and renamed over it, so
    /// the managed loader - which polls the assembly's timestamp - can never
    /// observe a half-written file. The rename is what makes the swap atomic;
    /// writing in place would not be.
    /// </remarks>
    private static CompileOutcome Emit(
        Compilation compilation,
        CSharpCommandLineArguments arguments,
        string outputAssemblyPath)
    {
        string symbolsPath = Path.ChangeExtension(outputAssemblyPath, ".pdb");
        EmitOptions emitOptions = arguments.EmitOptions
            .WithDebugInformationFormat(DebugInformationFormat.PortablePdb)
            .WithPdbFilePath(symbolsPath);

        using var assemblyStream = new MemoryStream();
        using var symbolsStream = new MemoryStream();
        // The default Win32 version resource csc would synthesize. Nothing about
        // loading the assembly needs it, but emitting without it is the one way
        // the fast path's output differs structurally from the build's, and a
        // difference that costs four lines to remove is not worth keeping.
        // `noManifest` because a library never carries an application manifest.
        using Stream win32Resources = compilation.CreateDefaultWin32Resources(
            versionResource: true,
            noManifest: true,
            manifestContents: null,
            iconInIcoFormat: null);
        EmitResult result = compilation.Emit(
            assemblyStream,
            symbolsStream,
            xmlDocumentationStream: null,
            win32Resources: win32Resources,
            manifestResources: arguments.ManifestResources,
            options: emitOptions);

        if (!result.Success)
        {
            return new CompileOutcome(CompileStatus.Failed, Format(result.Diagnostics));
        }

        string? directory = Path.GetDirectoryName(outputAssemblyPath);
        if (!string.IsNullOrEmpty(directory))
        {
            Directory.CreateDirectory(directory);
        }
        // Symbols first: the assembly's rename is what the loader reacts to, so
        // the pdb has to be in place before it or the reload reads the previous
        // build's symbols, refuses them by ID and runs without breakpoints.
        // Symbols are best effort: a locked or otherwise unwritable pdb costs
        // line numbers and debugging, which is not worth failing a reload for.
        try
        {
            ReplaceFile(symbolsPath, symbolsStream.ToArray());
        }
        catch (IOException)
        {
        }
        catch (UnauthorizedAccessException)
        {
        }
        ReplaceFile(outputAssemblyPath, assemblyStream.ToArray());
        return new CompileOutcome(CompileStatus.Compiled, "");
    }

    /// <summary>Write bytes to a sibling temporary file and rename it over the target.</summary>
    private static void ReplaceFile(string path, byte[] contents)
    {
        string temporaryPath = path + ".pill_new";
        File.WriteAllBytes(temporaryPath, contents);
        File.Move(temporaryPath, path, overwrite: true);
    }

    // =========================================================================
    // Plan
    // =========================================================================

    /// <summary>
    /// Parse the captured command line and rebuild every cached artifact, unless
    /// the capture is unchanged since last time.
    /// </summary>
    ///
    /// <returns>Null when the plan is usable, or the reason it is not.</returns>
    private static CompileOutcome? EnsurePlan(string responseFilePath)
    {
        FileInfo responseFile;
        try
        {
            responseFile = new FileInfo(responseFilePath);
        }
        catch (Exception failure)
        {
            return new CompileOutcome(
                CompileStatus.Unavailable,
                $"invalid compiler argument file path: {failure.Message}");
        }
        if (!responseFile.Exists)
        {
            return new CompileOutcome(
                CompileStatus.Unavailable,
                $"no captured compiler arguments at {responseFilePath}");
        }

        bool unchanged = _arguments is not null
            && string.Equals(_planResponseFile, responseFilePath, StringComparison.OrdinalIgnoreCase)
            && _planWriteUtc == responseFile.LastWriteTimeUtc;
        if (unchanged)
        {
            return null;
        }

        string[] argumentLines;
        try
        {
            argumentLines = File.ReadAllLines(responseFilePath);
        }
        catch (IOException failure)
        {
            return new CompileOutcome(
                CompileStatus.Unavailable,
                $"could not read captured compiler arguments: {failure.Message}");
        }

        // The capture sits in the project's obj directory, so the project
        // directory is what MSBuild wrote its relative paths against.
        string baseDirectory = Path.GetFullPath(
            Path.Combine(responseFile.DirectoryName ?? ".", ".."));

        CSharpCommandLineArguments parsed = CSharpCommandLineParser.Default.Parse(
            argumentLines, baseDirectory, sdkDirectory: null);
        if (!parsed.Errors.IsEmpty)
        {
            return new CompileOutcome(
                CompileStatus.Unavailable,
                "captured compiler arguments could not be parsed: " + Format(parsed.Errors));
        }

        // A capture with no source files would "succeed" and emit an empty
        // assembly over a working one. It means the capture was truncated
        // rather than that the project has no code, so refuse it and let the
        // caller run a real build - which rewrites it.
        if (parsed.SourceFiles.IsEmpty)
        {
            return new CompileOutcome(
                CompileStatus.Unavailable,
                $"the captured compiler arguments at {responseFilePath} list no source files");
        }

        // Everything derived from the previous plan is now stale.
        _arguments = parsed;
        _planResponseFile = responseFilePath;
        _planWriteUtc = responseFile.LastWriteTimeUtc;
        _planBaseDirectory = baseDirectory;
        _references.Clear();
        _analyzerLoadFailures.Clear();

        LoadAnalyzerConfiguration(parsed, baseDirectory);
        LoadAnalyzersAndGenerators(parsed, baseDirectory);
        return null;
    }

    /// <summary>Parse every `/analyzerconfig` file into one resolved config set.</summary>
    private static void LoadAnalyzerConfiguration(
        CSharpCommandLineArguments arguments,
        string baseDirectory)
    {
        var configs = new List<AnalyzerConfig>(arguments.AnalyzerConfigPaths.Length);
        foreach (string configPath in arguments.AnalyzerConfigPaths)
        {
            string fullPath = Path.GetFullPath(Path.Combine(baseDirectory, configPath));
            try
            {
                configs.Add(AnalyzerConfig.Parse(File.ReadAllText(fullPath), fullPath));
            }
            catch (IOException)
            {
                // A missing analyzer config costs configured severities, not the
                // compile; the remaining configs still apply.
            }
        }

        AnalyzerConfigSet configSet = AnalyzerConfigSet.Create(configs);
        _analyzerConfigOptions = new CapturedAnalyzerConfigOptionsProvider(configSet);
        _syntaxTreeOptions = new CapturedSyntaxTreeOptionsProvider(configSet);

        var additional = ImmutableArray.CreateBuilder<AdditionalText>(
            arguments.AdditionalFiles.Length);
        foreach (CommandLineSourceFile file in arguments.AdditionalFiles)
        {
            additional.Add(new CapturedAdditionalText(file.Path, arguments.ChecksumAlgorithm));
        }
        _additionalFiles = additional.ToImmutable();
    }

    /// <summary>Load every `/analyzer` assembly and split it into rules and generators.</summary>
    ///
    /// <remarks>
    /// Both halves matter and for different reasons. The generators produce the
    /// named query-row wrappers gameplay code iterates through, so skipping them
    /// would not compile at all. The analyzers are the PILLxxxx rules, most of
    /// which exist precisely to keep a project assembly unloadable-safe - a
    /// reload that skipped them would happily load the static event or the
    /// background thread that then pins the old assembly forever.
    /// </remarks>
    private static void LoadAnalyzersAndGenerators(
        CSharpCommandLineArguments arguments,
        string baseDirectory)
    {
        _analyzerLoader ??= new PluginAnalyzerAssemblyLoader();
        var analyzers = ImmutableArray.CreateBuilder<DiagnosticAnalyzer>();
        var generators = ImmutableArray.CreateBuilder<ISourceGenerator>();

        var analyzerPaths = new List<string>(arguments.AnalyzerReferences.Length);
        foreach (CommandLineAnalyzerReference analyzerReference in arguments.AnalyzerReferences)
        {
            analyzerPaths.Add(
                Path.GetFullPath(Path.Combine(baseDirectory, analyzerReference.FilePath)));
        }

        // Declare every analyzer path before loading any of them. Analyzers ship
        // in sets that reference each other - the SDK's CSharp.NetAnalyzers needs
        // NetAnalyzers, the interop generators need SourceGeneration - and each
        // is passed as its own `/analyzer:`. Loading one before the whole set is
        // known leaves its siblings unresolvable, which Roslyn reports as a
        // silent analyzer load failure rather than an error.
        foreach (string analyzerPath in analyzerPaths)
        {
            _analyzerLoader.AddDependencyLocation(analyzerPath);
        }

        foreach (string fullPath in analyzerPaths)
        {
            if (!File.Exists(fullPath))
            {
                _analyzerLoadFailures.Add($"missing analyzer assembly {fullPath}");
                continue;
            }
            var reference = new AnalyzerFileReference(fullPath, _analyzerLoader);
            reference.AnalyzerLoadFailed += (_, failure) =>
                _analyzerLoadFailures.Add($"{fullPath}: {failure.Message}");
            try
            {
                analyzers.AddRange(reference.GetAnalyzers(LanguageNames.CSharp));
                generators.AddRange(reference.GetGenerators(LanguageNames.CSharp));
            }
            catch (Exception failure)
            {
                _analyzerLoadFailures.Add($"{fullPath}: {failure.Message}");
            }
        }

        _analyzers = analyzers.ToImmutable();
        // A fresh driver, because the previous one's incremental state belongs to
        // the previous plan's generators and parse options.
        _generatorDriver = generators.Count == 0
            ? null
            : CSharpGeneratorDriver.Create(
                generators.ToImmutable(),
                _additionalFiles,
                (CSharpParseOptions)arguments.ParseOptions,
                _analyzerConfigOptions);

        if (_analyzerLoadFailures.Count > 0)
        {
            Console.Error.WriteLine(
                "[csharp_compiler] analyzer load problems: "
                + string.Join("; ", _analyzerLoadFailures));
        }
    }
}
