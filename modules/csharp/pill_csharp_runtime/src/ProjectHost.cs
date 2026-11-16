// Collectible managed gameplay loader and ECS system discovery.
//
// Responsibilities:
// - Loads project assemblies without locking their build output.
// - Discovers and deterministically orders methods marked with System.
// - Derives scheduler access from every query parameter a method declares.
// - Reloads method bodies, parks a changed component manifest, and lets the
//   host re-register a changed system set. Only the startup set is refused.
//
// Design:
// - Every project version lives in a collectible AssemblyLoadContext and is read
//   from bytes so the compiler can replace the source DLL on Windows. Its
//   portable PDB is read the same way and handed to the context, which is what
//   gives a managed debugger symbols without either file being held open.
// - A reload always swaps method bodies. A changed component manifest parks
//   until the host applies it (`PollStatus.ManifestPending`), a changed system
//   set is re-registered by the host from the arriving assembly, and only the
//   startup set is pinned - startup methods do not run again on reload.
// - Once the bridge is bound, every line about a load or a reload goes through
//   `Log`, so it carries the engine's timestamp, level and target. `Console` is
//   only for failures that happen before anything is bound - the startup path
//   in this file and in `LoaderInterop` - because `Log` reaches the engine
//   through the bridge and has nowhere to go yet.

using System.Linq.Expressions;
using System.Reflection;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Runtime.CompilerServices;
using System.Runtime.Loader;
using System.Threading;

namespace TracyLive.Loader;

// =============================================================================
// Discovered System Metadata
// =============================================================================

/// <summary>One 128-bit component ID and its native access mode.</summary>
internal readonly record struct ManagedAccess(
    ulong ComponentKey, ulong ComponentKeyHigh, byte Mode, byte Kind)
{
    /// <summary>The key names a component column.</summary>
    internal const byte ComponentKind = 0;

    /// <summary>The key names a world resource.</summary>
    internal const byte ResourceKind = 1;

    /// <summary>
    /// The key names a Rust-owned resource by its shared name
    /// (<c>ResMut&lt;AssetManager&gt;</c>), whose value managed code never sees.
    /// </summary>
    internal const byte NativeResourceKind = 2;
}

/// <summary>Compiled managed system plus its scheduler declaration.</summary>
/// <remarks>
/// `Accesses` is the union of every query's component terms: one entry per
/// component, positioned by first occurrence, with a write in any query
/// upgrading a shared declaration. The host consumes exactly this list for
/// scheduling, access checks, and reload verification, so query grouping
/// never crosses the native ABI. `Queries` preserves the grouping for
/// iteration and for the reload signature. `Order`, `Phase`, `RunInEditMode`
/// and `Conditions` are the `[System]` declaration's schedule values, which
/// both discovery postures fill the same way.
/// </remarks>
internal sealed record ManagedSystem(
    string Name, ManagedAccess[] Accesses, QueryDescriptor?[] Queries,
    bool UsesCommands, int Order, RunPhase Phase, bool RunInEditMode,
    ManagedCondition[] Conditions, Action Run)
{
    internal string Signature
    {
        get
        {
            var builder = new System.Text.StringBuilder(Name);
            // A changed schedule changes where the system runs, so it belongs
            // to the signature the reload compares: a changed value has to
            // re-register the system, not keep a schedule the source no
            // longer describes.
            builder.Append(":order=").Append(Order)
                .Append(":phase=").Append((byte)Phase)
                .Append(":edit=").Append(RunInEditMode)
                .Append(":commands=").Append(UsesCommands);
            // One separator per query keeps a grouping change visible even
            // when the merged access list happens to be identical.
            foreach (QueryDescriptor? query in Queries)
            {
                builder.Append('|');
                if (query is null)
                    continue;
                for (int index = 0; index < query.Terms.Count; index++)
                {
                    if (index > 0)
                        builder.Append(',');
                    QueryTermDescriptor term = query.Terms[index];
                    if (term.IsEntity)
                    {
                        builder.Append("entity");
                        continue;
                    }
                    builder.Append((byte)term.Access)
                        .Append(':')
                        .Append(term.ComponentKeyHigh.ToString("X16"))
                        .Append(term.ComponentKey.ToString("X16"));
                }
            }
            return builder.ToString();
        }
    }
}

/// <summary>Compiled one-shot startup method.</summary>
internal sealed record ManagedStartup(string Name, Action Run);

/// <summary>
/// Outcome of one reload poll, reported back to the Rust host.
/// </summary>
internal enum PollStatus : byte
{
    /// <summary>No reload was due or the assembly has not changed.</summary>
    NoChange = 0,

    /// <summary>A behavior-compatible assembly was swapped in.</summary>
    Reloaded = 1,

    /// <summary>The new assembly was rejected; the old one stays loaded.</summary>
    Rejected = 2,

    /// <summary>
    /// A behaviour-compatible assembly is loaded and waiting, but its component
    /// manifest differs from the one in force.
    /// </summary>
    /// <remarks>
    /// The managed side cannot decide this one. Whether a manifest change can
    /// be applied depends on what each component is bound to natively - a
    /// descriptor column can be relaid out and its rows migrated, a mirror of a
    /// Rust type cannot - and only the host holds those bindings.
    ///
    /// So the swap stops here and waits. Applying the manifest *after* swapping
    /// would leave a running assembly against a world that never took its
    /// layout; parking first means a refusal costs an unload and nothing else.
    /// </remarks>
    ManifestPending = 3,
}

// =============================================================================
// ProjectHost
// =============================================================================

/// <summary>
/// Loads the collectible gameplay assembly and discovers methods marked with
/// <see cref="SystemAttribute"/>. Every query parameter is both an
/// executable iterator and part of the authoritative scheduler access list;
/// the merged list is what the host registers.
/// </summary>
internal sealed class ProjectHost
{
    /// <summary>
    /// Collectible context for one project version. Requests for csharp_runtime types
    /// resolve to the already-loaded stable runtime assembly.
    /// </summary>
    private sealed class ProjectContext : AssemblyLoadContext
    {
        public ProjectContext() : base(isCollectible: true)
        {
            // Dropping the resolved delegate cache here rather than only
            // before a successful swap covers every unload path, the
            // rejection one included, and keeps the runtime from being the
            // thing that holds a retiring context alive. Only the cache goes:
            // the trampoline addresses the host published still belong to the
            // next context, so the unload neither clears the table nor bumps
            // the generation a live call is resolving against.
            Unloading += _ => MirrorMethods.DropDelegateCache();
        }

        protected override Assembly? Load(AssemblyName assemblyName)
        {
            if (assemblyName.Name == "csharp_runtime")
                return typeof(ProjectHost).Assembly;
            return null;
        }
    }

    /// <summary>
    /// One loaded, validated project version waiting on the host's verdict
    /// about its component manifest.
    /// </summary>
    /// <remarks>
    /// Nothing about the running generation changes while a version sits here:
    /// its context is loaded but not installed, so an abort costs an unload and
    /// leaves the world exactly as it was.
    /// </remarks>
    private sealed record PendingReload(
        ProjectContext Context,
        ManagedSystem[] Systems,
        ManagedStartup[] Startups,
        byte[] Manifest,
        DateTime WriteUtc);

    /// <summary>The version awaiting a verdict, if any.</summary>
    private PendingReload? _pending;

    /// <summary>Interval between assembly mtime checks on the ordinary path.</summary>
    /// <remarks>
    /// Part of a deliberate two-mechanism design shared with the Rust host.
    /// The interval exists only so the loader can tell settled bytes from a
    /// half-written assembly: a `dotnet build` offers no completion signal it
    /// can trust. When the host compiles in-process it knows the file is
    /// complete and calls <see cref="RequestImmediatePoll"/>, which skips the
    /// wait. This interval is not the reload trigger - the trigger is always
    /// a <see cref="PollReload"/> call.
    /// </remarks>
    private static readonly TimeSpan PollInterval = TimeSpan.FromMilliseconds(500);
    private readonly string _assemblyPath;
    private ProjectContext? _context;
    private ManagedSystem[] _systems = [];
    private ManagedStartup[] _startups = [];
    private byte[] _componentManifest = [];
    private string?[] _lastSystemErrors = [];
    /// <summary>
    /// Write time of the assembly this loader last EXAMINED, accepted or not.
    /// Not "the write time of the loaded assembly": after a rejection the loaded
    /// assembly is the previous one, and the point of the field is to stop the
    /// poll re-examining bytes it has already judged.
    /// </summary>
    private DateTime _lastWriteUtc;
    private DateTime _lastPollUtc;
    /// <summary>
    /// Every context this loader has retired, weakly referenced so the check
    /// itself never keeps one alive.
    /// </summary>
    private readonly List<WeakReference<ProjectContext>> _retiredContexts = new();
    /// <summary>Versions retired since startup, and versions confirmed dead.</summary>
    private int _retiredCount;
    private int _collectedCount;

    /// <summary>Create a loader for the configured gameplay assembly.</summary>
    public ProjectHost(string assemblyPath) => _assemblyPath = assemblyPath;

    /// <summary>Number of systems exposed by the active project version.</summary>
    public int SystemCount => _systems.Length;
    public int StartupCount => _startups.Length;

    /// <summary>
    /// The write time of the version last examined, as the bookkeeping
    /// remembers it.
    /// </summary>
    /// <remarks>
    /// Never later than the stamp sampled before that version's bytes were
    /// read, so a build that landed during the load is examined by the next
    /// poll rather than skipped.
    /// </remarks>
    internal DateTime LastExaminedWriteUtc => _lastWriteUtc;
    public ReadOnlySpan<byte> ComponentManifest => _componentManifest;

    /// <summary>Return one reflected scheduler access.</summary>
    public ManagedAccess GetAccess(int systemIndex, int accessIndex) =>
        _systems[systemIndex].Accesses[accessIndex];

    /// <summary>Return the reflected name of one system.</summary>
    public string GetSystemName(int systemIndex) => _systems[systemIndex].Name;

    /// <summary>Return the UTF-8 byte count of one system's name.</summary>
    public int GetSystemNameLength(int systemIndex) =>
        System.Text.Encoding.UTF8.GetByteCount(_systems[systemIndex].Name);

    /// <summary>Return the number of accesses declared by one system.</summary>
    public int GetAccessCount(int systemIndex) => _systems[systemIndex].Accesses.Length;
    public bool UsesCommands(int systemIndex) => _systems[systemIndex].UsesCommands;

    /// <summary>Return one system's declared order within its phase.</summary>
    public int GetSystemOrder(int systemIndex) => _systems[systemIndex].Order;

    /// <summary>Return one system's declared frame phase (0, 1 or 2).</summary>
    public byte GetSystemRunPhase(int systemIndex) => (byte)_systems[systemIndex].Phase;

    /// <summary>Return whether one system also runs while the editor edits.</summary>
    public bool RunsInEditMode(int systemIndex) => _systems[systemIndex].RunInEditMode;

    /// <summary>Load the initial project version (reflection or AOT registry).</summary>
    public void Init()
    {
        // NativeAOT cannot load a project assembly or reflect over it at
        // runtime, so the generated registry (installed at module init) is the
        // only source of systems. The JIT path keeps loading + reflecting.
        if (!RuntimeFeature.IsDynamicCodeSupported)
        {
            LoadFromAotRegistry();
            return;
        }
        Load(isReload: false, File.GetLastWriteTimeUtc(_assemblyPath));
    }

    /// <summary>
    /// Load systems from the compile-time generated registry (NativeAOT).
    /// </summary>
    private void LoadFromAotRegistry()
    {
        ManagedSystem[] systems = AotRegistry.Systems
            .Select(registration =>
            {
                QueryDescriptor?[] queries = registration.Queries ?? [];
                ValidateQueries(registration.Name, queries, registration.QueryNames);
                // The two postures must declare identical access, or a system
                // that works in development would be refused its resource in
                // the shipping build. Same list, same order, different source.
                ManagedAccess[] accesses = MergeAccesses(queries)
                    .Concat((registration.ResourceAccesses ?? [])
                        .Select(resource => new ManagedAccess(
                            resource.Low, resource.High, resource.Mode, resource.Kind)))
                    .ToArray();
                return new ManagedSystem(
                    registration.Name,
                    accesses,
                    queries,
                    registration.UsesCommands,
                    registration.Order,
                    registration.RunPhase,
                    registration.RunInEditMode,
                    registration.Conditions ?? [],
                    registration.Run);
            })
            .ToArray();
        ManagedStartup[] startups = AotRegistry.Startups
            .Select(registration => new ManagedStartup(registration.Name, registration.Run))
            .ToArray();
        if (systems.Length == 0)
            throw new InvalidOperationException(
                "No [System] methods were generated for the AOT build.");
        _systems = systems;
        _startups = startups;
        _componentManifest = ProjectManifestBuilder.Build(
            systems, AotRegistry.ProjectAssembly);
        _lastSystemErrors = new string?[systems.Length];
        _lastAllocatedBytes = new long[systems.Length];
    }

    /// <summary>Invoke a discovered system by its stable index.</summary>
    public void RunSystem(int index) => _systems[index].Run();
    public void RunStartup(int index) => _startups[index].Run();

    /// <summary>Name one system for a diagnostic, without throwing on a bad index.</summary>
    public string DescribeSystem(int systemIndex) =>
        (uint)systemIndex < (uint)_systems.Length ? _systems[systemIndex].Name : $"system {systemIndex}";

    /// <summary>Bytes the last run of each system allocated on the managed heap.</summary>
    private long[] _lastAllocatedBytes = [];

    /// <summary>Record what one system's last run allocated.</summary>
    /// <remarks>
    /// Attribution, not prevention: the query path itself allocates nothing, so
    /// a non-zero figure here is always the script's own. Surfacing it per
    /// system is what turns a frame spike into a name.
    /// </remarks>
    public void RecordAllocation(int systemIndex, long bytes)
    {
        if ((uint)systemIndex < (uint)_lastAllocatedBytes.Length)
            _lastAllocatedBytes[systemIndex] = bytes;
    }

    /// <summary>Bytes the last run of one system allocated, or zero.</summary>
    public long GetAllocatedBytes(int systemIndex) =>
        (uint)systemIndex < (uint)_lastAllocatedBytes.Length ? _lastAllocatedBytes[systemIndex] : 0;

    /// <summary>When the allocation summary was last written.</summary>
    private DateTime _lastAllocationReportUtc = DateTime.UtcNow;

    /// <summary>Interval between allocation summaries.</summary>
    private static readonly TimeSpan AllocationReportInterval = TimeSpan.FromSeconds(10);

    /// <summary>
    /// Periodically name the systems that allocate on the managed heap.
    /// </summary>
    /// <remarks>
    /// The query path allocates nothing - rows and enumerators are ref structs
    /// and the generated wrappers do not box - so any figure here is the
    /// script's own. It matters more than the raw number suggests: the worker
    /// threads that run systems become attached managed threads, so a
    /// collection triggered by one system suspends the whole pool.
    ///
    /// Reported on an interval rather than per frame, because the point is to
    /// notice a steady allocator, not to narrate every frame.
    /// </remarks>
    private void ReportAllocationsIfDue()
    {
        DateTime now = DateTime.UtcNow;
        if (now - _lastAllocationReportUtc < AllocationReportInterval)
            return;
        _lastAllocationReportUtc = now;
        var offenders = new List<string>();
        for (int index = 0; index < _lastAllocatedBytes.Length; index++)
        {
            if (_lastAllocatedBytes[index] > 0)
                offenders.Add($"{_systems[index].Name} ({_lastAllocatedBytes[index]} B/frame)");
        }
        if (offenders.Count > 0)
            Console.WriteLine(
                "[csharp_runtime] managed systems allocating per frame: " +
                string.Join(", ", offenders));
    }

    /// <summary>Record a failure against the system with this reflected name.</summary>
    /// <remarks>
    /// The frame-scope context knows which system it was installed for by name
    /// rather than by index, because that is what makes its message readable.
    /// </remarks>
    public void SetSystemErrorByName(string systemName, string message)
    {
        for (int index = 0; index < _systems.Length; index++)
        {
            if (_systems[index].Name == systemName)
            {
                SetSystemError(index, message);
                return;
            }
        }
    }

    /// <summary>Return the last failure message recorded for one system, if any.</summary>
    public string GetSystemError(int systemIndex) =>
        (uint)systemIndex < (uint)_lastSystemErrors.Length
            ? _lastSystemErrors[systemIndex] ?? ""
            : "";

    /// <summary>Record the failure message reported by one managed system.</summary>
    /// <remarks>Out-of-range indices are ignored so a hostile or stale system
    /// index can never corrupt the error store.</remarks>
    public void SetSystemError(int systemIndex, string message)
    {
        if ((uint)systemIndex < (uint)_lastSystemErrors.Length)
            _lastSystemErrors[systemIndex] = message;
    }

    /// <summary>Let the next <see cref="PollReload"/> run without waiting.</summary>
    ///
    /// <remarks>
    /// The poll interval exists because the loader cannot tell when a build has
    /// finished writing the assembly, so it samples instead. When the host
    /// compiles the project in-process it does know: it wrote the file itself,
    /// through an atomic rename. Telling the loader so turns the interval's
    /// average quarter-second of waiting into nothing, and only in the case
    /// where the certainty is real - a `dotnet build` still copies the assembly
    /// non-atomically, and that path keeps sampling.
    /// </remarks>
    public void RequestImmediatePoll() => _lastPollUtc = DateTime.MinValue;

    /// <summary>
    /// Poll the project DLL timestamp and reload a newer build.
    /// Returns the swap outcome so the Rust host can distinguish a clean
    /// behavior reload from a rejection that requires a host restart.
    /// </summary>
    public byte PollReload()
    {
        ReportAllocationsIfDue();
        // NativeAOT builds are static shipping artifacts: no project assembly
        // exists to watch, so a reload poll is always a no-op.
        if (!RuntimeFeature.IsDynamicCodeSupported)
            return (byte)PollStatus.NoChange;
        // A version already waiting on the host's verdict owns the slot;
        // polling again would load a second one on top of it. The host answers
        // within the same frame, so this is a guard rather than a state the
        // loader sits in.
        if (_pending is not null)
            return (byte)PollStatus.ManifestPending;
        var now = DateTime.UtcNow;
        if (now - _lastPollUtc < PollInterval)
            return (byte)PollStatus.NoChange;
        _lastPollUtc = now;

        // Before deciding anything: see whether the previous versions actually
        // unloaded. The API only requests an unload, so this is the only place
        // a leaked context becomes visible.
        SweepRetiredContexts();

        DateTime written;
        try
        {
            written = File.GetLastWriteTimeUtc(_assemblyPath);
        }
        catch (IOException)
        {
            return (byte)PollStatus.NoChange;
        }

        if (written <= _lastWriteUtc)
            return (byte)PollStatus.NoChange;

        try
        {
            if (Load(isReload: true, written) == PollStatus.ManifestPending)
            {
                Log.Info(
                    "[csharp_runtime] " +
                    $"{Path.GetFileName(_assemblyPath)} changed its component manifest; " +
                    "awaiting the host's verdict");
                return (byte)PollStatus.ManifestPending;
            }
            Log.Info(
                $"[csharp_runtime] reloaded {Path.GetFileName(_assemblyPath)} " +
                $"(retired {_retiredCount}, collected {_collectedCount})");
            return (byte)PollStatus.Reloaded;
        }
        catch (Exception e)
        {
            Log.Error($"[csharp_runtime] reload failed: {e}");
            // Remember the rejected build too. Without this the same bytes are
            // re-examined every poll interval for as long as the source stays
            // broken - reading the whole assembly, creating a collectible
            // context, reflecting over it and unloading it again, twice a
            // second, forever. The Rust host logs the rejection only once, so
            // the churn is invisible. A genuinely new build has a later write
            // time and is examined normally.
            _lastWriteUtc = written;
            return (byte)PollStatus.Rejected;
        }
    }

    /// <summary>
    /// Count the retired assembly versions the runtime has actually released.
    /// </summary>
    /// <remarks>
    /// A collectible context unloads only when nothing roots it, and
    /// <see cref="AssemblyLoadContext.Unload"/> returns before that happens -
    /// it cannot await a collection. Every blocker Microsoft lists for the
    /// feature (a static holding a delegate, an event handler, a cached
    /// <see cref="Type"/>, a thread-pool callback, a thread-static) is
    /// something a project writes by accident, and each one leaves the whole
    /// previous assembly - and its statics - alive with no symptom but memory
    /// growth. Holding a weak reference and looking after a collection turns
    /// that into an observable fact. Survivors are reported on stderr, the
    /// same channel as the reload-failed line, and the counts ride on the
    /// reloaded line.
    /// </remarks>
    private void SweepRetiredContexts()
    {
        if (_retiredContexts.Count == 0)
            return;
        // A collection is what turns "unload requested" into an answer. One
        // pass is often enough, but a context whose assembly is referenced
        // until the finalizer pass can need another; the loop is the pattern
        // Microsoft's own sample uses, and it keeps a merely-slow collection
        // from being reported as a leak.
        for (int attempt = 0; attempt < 10 && AnyRetiredContextAlive(); attempt++)
        {
            GC.Collect();
            GC.WaitForPendingFinalizers();
        }
        int survivors = 0;
        for (int index = _retiredContexts.Count - 1; index >= 0; index--)
        {
            if (_retiredContexts[index].TryGetTarget(out _))
            {
                survivors++;
                continue;
            }
            _retiredContexts.RemoveAt(index);
            _collectedCount++;
        }
        if (survivors > 0)
        {
            Log.Warn(
                $"[csharp_runtime] {survivors} retired version(s) of " +
                $"{Path.GetFileName(_assemblyPath)} are still loaded after " +
                $"{_collectedCount} of {_retiredCount} reloads; something in the " +
                "assembly roots its load context (a static event, a cached Type, " +
                "a thread-pool callback)");
        }
    }

    /// <summary>Whether any retired context the loader still tracks is alive.</summary>
    private bool AnyRetiredContextAlive()
    {
        foreach (WeakReference<ProjectContext> reference in _retiredContexts)
        {
            if (reference.TryGetTarget(out _))
                return true;
        }
        return false;
    }

    /// <summary>
    /// Queue one retiring context for the unload check, then request the unload.
    /// </summary>
    private void RetireContext(ProjectContext context)
    {
        _retiredContexts.Add(new WeakReference<ProjectContext>(context));
        _retiredCount++;
        context.Unload();
    }

    /// <summary>
    /// Load one assembly version, validate its startup set, then swap it
    /// atomically with the active collectible context.
    /// </summary>
    /// <param name="isReload">Whether this is a swap over a running version.</param>
    /// <param name="writeUtc">
    /// The write time of the version being loaded, sampled before its bytes
    /// were read. That is the stamp the load remembers: a build renamed over
    /// the file while these bytes load has a later stamp and has not been
    /// examined, so recording a fresh sample at the end would skip it until
    /// the file changed again.
    /// </param>
    /// <remarks>
    /// Internal so a test can drive one load with an explicit stamp and assert
    /// what the bookkeeping remembers; the runtime only reaches it through
    /// <see cref="PollReload"/> and <see cref="Init"/>.
    /// </remarks>
    internal PollStatus Load(bool isReload, DateTime writeUtc)
    {
        var bytes = ReadAllBytesWithRetry(_assemblyPath);
        var symbols = ReadMatchingSymbols(_assemblyPath, bytes);
        var context = new ProjectContext();
        try
        {
            // Symbols go in with the image: an assembly loaded from a stream
            // has no path, so this is the only way a managed debugger can bind
            // breakpoints in it.
            Assembly assembly;
            using (var stream = new MemoryStream(bytes))
            using (var symbolStream = symbols is null ? null : new MemoryStream(symbols))
                assembly = context.LoadFromStream(stream, symbolStream);

            var systems = DiscoverSystems(assembly);
            var startups = DiscoverStartups(assembly);
            if (systems.Length == 0)
                throw new InvalidOperationException(
                    "No [System] methods with a supported query parameter were found.");

            byte[] manifest = ProjectManifestBuilder.Build(systems, assembly);

            // A changed system set is no longer refused: the host clears the
            // project's systems and re-registers them from this assembly's
            // reflected metadata, so the scheduler is rebuilt rather than left
            // stale. Startups are different and still refuse - they are not
            // re-run on reload, so a changed set would silently never execute.
            if (isReload && !_startups.Select(s => s.Name).SequenceEqual(startups.Select(s => s.Name)))
                throw new InvalidOperationException(
                    "C# startup methods changed; restart the host. Startup methods are not rerun during hot reload.");

            // A changed manifest parks rather than refusing: whether it can be
            // applied depends on the native bindings, which only the host
            // holds. It answers with commit or abort.
            if (isReload && !_componentManifest.AsSpan().SequenceEqual(manifest))
            {
                _pending = new PendingReload(
                    context, systems, startups, manifest, writeUtc);
                return PollStatus.ManifestPending;
            }

            var oldContext = _context;
            // The new assembly carries its own mirror delegate types (each
            // collectible context defines them), and an extension reload
            // may have moved every mirrored method to fresh trampoline
            // addresses, so re-read the host's table and drop the old
            // context's resolved delegates. Done before the swap so the old
            // context holds nothing back and can actually unload.
            Engine.ReloadMirrorMethods();
            _context = context;
            _systems = systems;
            _startups = startups;
            _componentManifest = manifest;
            _lastSystemErrors = new string?[systems.Length];
            _lastAllocatedBytes = new long[systems.Length];
            _lastWriteUtc = writeUtc;
            if (oldContext is not null)
                RetireContext(oldContext);
            return PollStatus.Reloaded;
        }
        catch
        {
            RetireContext(context);
            throw;
        }
    }

    /// <summary>UTF-8 manifest of the version awaiting a verdict.</summary>
    public ReadOnlySpan<byte> PendingComponentManifest =>
        _pending is null ? ReadOnlySpan<byte>.Empty : _pending.Manifest;

    /// <summary>Install the parked version; the host accepted its manifest.</summary>
    /// <remarks>
    /// The host has already relaid out the columns the new manifest asks for,
    /// so the world and this assembly agree the moment the swap lands.
    /// </remarks>
    public void CommitPendingReload()
    {
        if (_pending is null)
            return;
        PendingReload pending = _pending;
        _pending = null;

        ProjectContext? oldContext = _context;
        // Same ordering as the ordinary swap: drop the old context's resolved
        // delegates before it retires, so nothing holds it back.
        Engine.ReloadMirrorMethods();
        _context = pending.Context;
        _systems = pending.Systems;
        _startups = pending.Startups;
        _componentManifest = pending.Manifest;
        _lastSystemErrors = new string?[pending.Systems.Length];
        _lastAllocatedBytes = new long[pending.Systems.Length];
        _lastWriteUtc = pending.WriteUtc;
        if (oldContext is not null)
            RetireContext(oldContext);

        // A commit is a completed reload, so it reports like one. The two paths
        // reaching this point - an ordinary swap and a manifest the host
        // applied first - are one event to anything watching the log, and the
        // suites wait on this line.
        Log.Info(
            $"[csharp_runtime] reloaded {Path.GetFileName(_assemblyPath)} " +
            $"(retired {_retiredCount}, collected {_collectedCount})");
    }

    /// <summary>Discard the parked version; the host refused its manifest.</summary>
    /// <remarks>
    /// The running generation never moved, so this unloads what was loaded
    /// speculatively and nothing else. The write time is remembered either way,
    /// so the same refused bytes are not re-examined twice a second for as long
    /// as the source stays that way.
    /// </remarks>
    public void AbortPendingReload()
    {
        if (_pending is null)
            return;
        PendingReload pending = _pending;
        _pending = null;
        _lastWriteUtc = pending.WriteUtc;
        RetireContext(pending.Context);
    }

    // =========================================================================
    // System Discovery and Compilation
    // =========================================================================

    /// <summary>Discover attributed static methods in deterministic order.</summary>
    internal static ManagedSystem[] DiscoverSystems(Assembly assembly)
    {
        ReportAttributedInstanceMethods(assembly);
        return assembly.GetTypes()
            .SelectMany(type => type.GetMethods(
                BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Static))
            .Where(method => method.GetCustomAttribute<SystemAttribute>() is not null)
            .OrderBy(method => method.DeclaringType?.FullName, StringComparer.Ordinal)
            .ThenBy(method => method.Name, StringComparer.Ordinal)
            .Select(CreateSystem)
            .ToArray();
    }

    /// <summary>
    /// Name attributed instance methods, which discovery cannot register.
    /// </summary>
    /// <remarks>
    /// Discovery looks for static methods only, so an <c>[System]</c> on an
    /// instance method is not merely rejected - it is invisible. The system
    /// never runs and nothing says why, which is the one declaration mistake
    /// here that produces no symptom at all. One extra reflection pass over an
    /// assembly that is already being reflected over buys a name.
    /// </remarks>
    private static void ReportAttributedInstanceMethods(Assembly assembly)
    {
        foreach (Type type in assembly.GetTypes())
        {
            foreach (MethodInfo method in type.GetMethods(
                         BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance))
            {
                bool attributed =
                    method.GetCustomAttribute<SystemAttribute>() is not null ||
                    method.GetCustomAttribute<StartupAttribute>() is not null;
                if (attributed)
                    Console.Error.WriteLine(
                        $"[csharp_runtime] {type.FullName}.{method.Name} carries an ECS attribute " +
                        "but is an instance method, so it was not registered and will never run. " +
                        "Make it static.");
            }
        }
    }

    /// <summary>Upper bound on the total parameters one managed system may declare.</summary>
    /// <remarks>
    /// Matches the native `SystemParam` tuple arity, so a system ported
    /// between the two languages can keep the same signature.
    /// </remarks>
    internal const int MaxSystemParameters = 6;

    /// <summary>One method's bound parameters, as `CreateSystem` and its conditions read them.</summary>
    private sealed class MethodBinding
    {
        /// <summary>Call arguments in declared parameter order.</summary>
        internal List<Expression> Arguments { get; } = new();

        /// <summary>Query descriptors in parameter order, one per query parameter.</summary>
        internal List<QueryDescriptor?> Queries { get; } = new();

        /// <summary>Query parameter names, aligned with <see cref="Queries"/>.</summary>
        internal List<string?> QueryNames { get; } = new();

        /// <summary>Resource declarations keyed by identity, so a repeat is caught.</summary>
        internal Dictionary<(ulong Low, ulong High), ManagedAccess> ResourceAccesses { get; } = new();

        /// <summary>Whether the method declared a Commands parameter.</summary>
        internal bool UsesCommands { get; set; }
    }

    /// <summary>
    /// Validate one managed method, derive component access from every query
    /// parameter it declares, and compile a parameterless runner for the Rust
    /// scheduler.
    /// </summary>
    /// <remarks>
    /// A system may declare any number of query parameters plus at most one
    /// `Commands`, within the parameter budget. Each query keeps its own
    /// iteration, exactly like a native system that takes several query
    /// parameters. Its `RunIf` conditions are compiled ahead of the body, so
    /// one runner invocation evaluates all of them and the body: a skipped
    /// system still costs one native-to-managed transition.
    /// </remarks>
    internal static ManagedSystem CreateSystem(MethodInfo method)
    {
        if (method.ReturnType != typeof(void))
            throw new InvalidOperationException($"{method} must return void.");

        // The declaration is read here, the one place both discovery paths
        // share, so a system's schedule cannot differ between postures.
        SystemAttribute? declaration = method.GetCustomAttribute<SystemAttribute>();
        string name = string.IsNullOrEmpty(declaration?.Name)
            ? $"{method.DeclaringType?.FullName}.{method.Name}"
            : declaration.Name;
        ManagedCondition[] conditions = (declaration?.RunIf ?? [])
            .Select(condition => new ManagedCondition(
                condition,
                declaration?.RunIfType ?? method.DeclaringType
                    ?? throw new InvalidOperationException(
                        $"{method} declares RunIf but has no declaring type to resolve it on.")))
            .ToArray();

        MethodBinding body = BindParameters(method, readOnly: false);
        QueryDescriptor?[] queryArray = body.Queries.ToArray();
        ValidateQueries(method.ToString() ?? method.Name, queryArray, body.QueryNames);
        Action bodyRunner = Expression.Lambda<Action>(
            Expression.Call(method, body.Arguments)).Compile();

        List<Func<bool>> conditionRunners = new(conditions.Length);
        List<ManagedAccess> conditionAccesses = new();
        foreach (ManagedCondition condition in conditions)
        {
            MethodInfo conditionMethod = ResolveCondition(method, condition);
            MethodBinding bound = BindParameters(conditionMethod, readOnly: true);
            QueryDescriptor?[] conditionQueries = bound.Queries.ToArray();
            ValidateQueries(
                conditionMethod.ToString() ?? conditionMethod.Name,
                conditionQueries,
                bound.QueryNames);
            conditionRunners.Add(Expression.Lambda<Func<bool>>(
                Expression.Call(conditionMethod, bound.Arguments)).Compile());
            conditionAccesses.AddRange(MergeAccesses(conditionQueries));
            conditionAccesses.AddRange(bound.ResourceAccesses.Values
                .OrderBy(access => access.ComponentKeyHigh)
                .ThenBy(access => access.ComponentKey));
        }
        Action runner = bodyRunner;
        if (conditionRunners.Count > 0)
        {
            runner = () =>
            {
                foreach (Func<bool> condition in conditionRunners)
                {
                    if (!condition())
                        return;
                }
                bodyRunner();
            };
        }

        // Component and resource accesses go to the scheduler in one list,
        // distinguished by kind: the native side derives one SystemAccess from
        // it, and a system's whole declaration has to arrive together for the
        // scheduler to order it against its peers. A condition's reads are part
        // of that declaration, so the scheduler never batches a writer of what
        // a condition reads beside the system.
        List<ManagedAccess> accesses = new();
        foreach (ManagedAccess access in MergeAccesses(queryArray))
            AddAccess(accesses, access);
        foreach (ManagedAccess access in body.ResourceAccesses.Values
                     .OrderBy(entry => entry.ComponentKeyHigh)
                     .ThenBy(entry => entry.ComponentKey))
            AddAccess(accesses, access);
        foreach (ManagedAccess access in conditionAccesses)
            AddAccess(accesses, access);

        return new ManagedSystem(
            name, accesses.ToArray(), queryArray, body.UsesCommands,
            declaration?.Order ?? 0,
            declaration?.RunPhase ?? RunPhase.Project,
            declaration?.RunInEditMode ?? false,
            conditions, runner);
    }

    /// <summary>
    /// Bind one method's parameters and enforce the access contract.
    /// </summary>
    /// <remarks>
    /// The body and every condition go through this one path: a condition is
    /// the read-only half of a system's parameter contract, so it is refused
    /// `Commands`, a `ResMut&lt;T&gt;` and a query that declares a write, and the
    /// refusal names the method.
    /// </remarks>
    private static MethodBinding BindParameters(MethodInfo method, bool readOnly)
    {
        ParameterInfo[] parameters = method.GetParameters();
        // A system must reach something; a condition may be a plain global
        // flag with no parameters at all.
        if (!readOnly && parameters.Length == 0)
            throw new InvalidOperationException(
                $"{method} must declare at least one parameter: queries, Commands, or both.");
        if (parameters.Length > MaxSystemParameters)
            throw new InvalidOperationException(
                $"{method} declares {parameters.Length} parameters; managed systems support at " +
                $"most {MaxSystemParameters}, matching the native system parameter arity.");

        var binding = new MethodBinding();
        foreach (ParameterInfo parameter in parameters)
        {
            Type parameterType = parameter.ParameterType;
            if (parameterType == typeof(Commands))
            {
                if (readOnly)
                    throw new InvalidOperationException(
                        $"{method} is a [System] condition and cannot take Commands; " +
                        "conditions are read-only.");
                if (binding.UsesCommands)
                    throw new InvalidOperationException($"{method} declares Commands more than once.");
                binding.UsesCommands = true;
                binding.Arguments.Add(Expression.Default(typeof(Commands)));
                continue;
            }
            if (TryDescribeResourceParameter(parameterType, out ManagedAccess resourceAccess))
            {
                if (readOnly && resourceAccess.Mode != 0)
                    throw new InvalidOperationException(
                        $"{method} is a [System] condition and takes " +
                        $"ResMut<{parameterType.GetGenericArguments()[0].Name}>; " +
                        "conditions are read-only.");
                var key = (resourceAccess.ComponentKey, resourceAccess.ComponentKeyHigh);
                if (!binding.ResourceAccesses.TryAdd(key, resourceAccess))
                    throw new InvalidOperationException(
                        $"{method} declares resource {parameterType.GetGenericArguments()[0].FullName} " +
                        "more than once; one Res<T> or ResMut<T> parameter per resource.");
                binding.Arguments.Add(Expression.Default(parameterType));
                continue;
            }
            if (!typeof(IQueryDescriptor).IsAssignableFrom(parameterType))
                throw UnsupportedQuery(method);
            object query;
            try
            {
                query = Activator.CreateInstance(parameterType, nonPublic: true)
                    ?? throw new InvalidOperationException($"Could not create query parameter {parameterType}.");
            }
            catch (TargetInvocationException exception) when (exception.InnerException is not null)
            {
                throw new InvalidOperationException(
                    $"Invalid query parameter on {method}: {exception.InnerException.Message}",
                    exception.InnerException);
            }
            QueryDescriptor descriptor = ((IQueryDescriptor)query).Descriptor;
            if (readOnly)
            {
                foreach (QueryTermDescriptor term in descriptor.Terms)
                {
                    if (!term.IsEntity && term.Access == QueryAccess.Write)
                        throw new InvalidOperationException(
                            $"{method} is a [System] condition and its query writes " +
                            $"{term.ComponentType?.FullName ?? "a component"}; " +
                            "conditions are read-only.");
                }
            }
            binding.Queries.Add(descriptor);
            binding.QueryNames.Add(parameter.Name);
            binding.Arguments.Add(Expression.Constant(query, parameterType));
        }
        return binding;
    }

    /// <summary>Resolve one declared condition to the method that evaluates it.</summary>
    /// <remarks>
    /// A condition is a static method by name on the declaring type (or the
    /// type `RunIfType` names), returning `bool`. A missing method or a
    /// non-bool return refuses here, by name, before anything registers.
    /// </remarks>
    private static MethodInfo ResolveCondition(MethodInfo system, ManagedCondition condition)
    {
        MethodInfo? resolved = condition.DeclaringType.GetMethod(
            condition.Name,
            BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Static);
        if (resolved is null)
            throw new InvalidOperationException(
                $"{system} declares the [System] condition '{condition.Name}', but " +
                $"{condition.DeclaringType.FullName} declares no static method by that name.");
        if (resolved.ReturnType != typeof(bool))
            throw new InvalidOperationException(
                $"[System] condition {condition.DeclaringType.FullName}.{condition.Name} must return " +
                $"bool, not {resolved.ReturnType.Name}.");
        return resolved;
    }

    /// <summary>Add one access unless an equivalent or stronger one is present.</summary>
    private static void AddAccess(List<ManagedAccess> accesses, ManagedAccess access)
    {
        for (int index = 0; index < accesses.Count; index++)
        {
            ManagedAccess existing = accesses[index];
            if (existing.ComponentKey == access.ComponentKey &&
                existing.ComponentKeyHigh == access.ComponentKeyHigh &&
                existing.Kind == access.Kind)
            {
                if (access.Mode > existing.Mode)
                    accesses[index] = existing with { Mode = access.Mode };
                return;
            }
        }
        accesses.Add(access);
    }

    /// <summary>
    /// Describe a <c>Res&lt;T&gt;</c> or <c>ResMut&lt;T&gt;</c> parameter as one
    /// reflected access, or report that the parameter is something else.
    /// </summary>
    /// <remarks>
    /// The declaration is read off the closed generic type rather than an
    /// instance: <see cref="IResourceParameter"/>'s members are static
    /// abstract, so there is nothing to construct, and reflecting over the
    /// generic definition keeps this AOT-safe - no MakeGenericMethod is
    /// involved, which NativeAOT could not service over a value type anyway.
    /// </remarks>
    private static bool TryDescribeResourceParameter(Type parameterType, out ManagedAccess access)
    {
        access = default;
        if (!parameterType.IsGenericType ||
            !typeof(IResourceParameter).IsAssignableFrom(parameterType))
            return false;
        Type definition = parameterType.GetGenericTypeDefinition();
        QueryAccess mode;
        if (definition == typeof(Res<>))
            mode = QueryAccess.Read;
        else if (definition == typeof(ResMut<>))
            mode = QueryAccess.Write;
        else
            return false;
        Type resource = parameterType.GetGenericArguments()[0];
        // The resource's declared identity, not its C# type name: a resource
        // that meets a Rust module halfway is registered under the name both
        // sides write down, and the access has to name the same thing.
        StableComponentId id = Engine.StableIdOf(ResourceNames.Of(resource));
        access = new ManagedAccess(id.Low, id.High, (byte)mode, ResourceNames.KindOf(resource));
        return true;
    }

    /// <summary>
    /// Reject a component that two query parameters both touch when either
    /// access is a write.
    /// </summary>
    /// <remarks>
    /// The native access check treats a write declaration as permitting
    /// reads, so without this validation both queries would be authorized to
    /// reach the same component - one through writable pointers, the other
    /// through read-only ones - which aliases storage the native scheduler
    /// cannot see. Sharing a component is allowed when every access is a
    /// read.
    /// </remarks>
    private static void ValidateQueries(
        string systemName, QueryDescriptor?[] queries, IReadOnlyList<string?>? queryNames)
    {
        var firstUse = new Dictionary<(ulong High, ulong Low), (QueryAccess Access, int Query)>();
        for (int queryIndex = 0; queryIndex < queries.Length; queryIndex++)
        {
            QueryDescriptor? query = queries[queryIndex];
            if (query is null)
                continue;
            foreach (QueryTermDescriptor term in query.Terms)
            {
                if (term.IsEntity)
                    continue;
                var key = (term.ComponentKeyHigh, term.ComponentKey);
                if (firstUse.TryGetValue(key, out var existing))
                {
                    if (existing.Access == QueryAccess.Write || term.Access == QueryAccess.Write)
                        throw new InvalidOperationException(
                            $"{systemName}: component {term.ComponentType!.FullName} is accessed by " +
                            $"{DescribeQuery(queryNames, existing.Query)} and " +
                            $"{DescribeQuery(queryNames, queryIndex)}; declare it in one query " +
                            "parameter or use read access only.");
                    continue;
                }
                firstUse[key] = (term.Access, queryIndex);
            }
        }
    }

    /// <summary>Name one query parameter for a validation message.</summary>
    private static string DescribeQuery(IReadOnlyList<string?>? names, int queryIndex) =>
        names is not null && queryIndex < names.Count && !string.IsNullOrWhiteSpace(names[queryIndex])
            ? $"query parameter '{names[queryIndex]}'"
            : $"query parameter #{queryIndex}";

    /// <summary>
    /// Merge every query's component terms into the flat scheduler access
    /// list: one entry per component, first-seen order, and a write in any
    /// query upgrades a shared declaration.
    /// </summary>
    internal static ManagedAccess[] MergeAccesses(QueryDescriptor?[] queries)
    {
        var merged = new List<ManagedAccess>();
        var indexByKey = new Dictionary<(ulong High, ulong Low), int>();
        foreach (QueryDescriptor? query in queries)
        {
            if (query is null)
                continue;
            foreach (QueryTermDescriptor term in query.Terms)
            {
                if (term.IsEntity)
                    continue;
                var key = (term.ComponentKeyHigh, term.ComponentKey);
                if (indexByKey.TryGetValue(key, out int existing))
                {
                    if (term.Access == QueryAccess.Write)
                        merged[existing] = merged[existing] with { Mode = 1 };
                    continue;
                }
                indexByKey[key] = merged.Count;
                merged.Add(new ManagedAccess(
                    term.ComponentKey, term.ComponentKeyHigh, (byte)term.Access,
                    ManagedAccess.ComponentKind));
            }
        }
        return merged.ToArray();
    }

    /// <summary>Discover and compile deterministic one-shot startup methods.</summary>
    internal static ManagedStartup[] DiscoverStartups(Assembly assembly) => assembly.GetTypes()
        .SelectMany(type => type.GetMethods(
            BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Static))
        .Where(method => method.GetCustomAttribute<StartupAttribute>() is not null)
        .OrderBy(method => method.DeclaringType?.FullName, StringComparer.Ordinal)
        .ThenBy(method => method.Name, StringComparer.Ordinal)
        .Select(CreateStartup)
        .ToArray();

    /// <summary>
    /// Validate one startup method and compile a parameterless runner for it.
    /// </summary>
    /// <remarks>
    /// A startup may take at most one <c>Commands</c> and any
    /// <c>Res&lt;T&gt;</c>/<c>ResMut&lt;T&gt;</c> parameters - asset loading
    /// takes <c>ResMut&lt;AssetManager&gt;</c>. Every one of them is stateless,
    /// so each is passed as its default, and none needs a declared access: a
    /// startup runs alone before the scheduler starts and holds the world.
    /// </remarks>
    internal static ManagedStartup CreateStartup(MethodInfo method)
    {
        if (method.ReturnType != typeof(void))
            throw new InvalidOperationException($"{method} must be static void.");
        ParameterInfo[] parameters = method.GetParameters();
        if (parameters.Length > MaxSystemParameters)
            throw new InvalidOperationException(
                $"{method} declares {parameters.Length} parameters; a startup takes at most " +
                $"{MaxSystemParameters}.");
        bool usesCommands = false;
        var arguments = new List<Expression>(parameters.Length);
        foreach (ParameterInfo parameter in parameters)
        {
            Type parameterType = parameter.ParameterType;
            if (parameterType == typeof(Commands))
            {
                if (usesCommands)
                    throw new InvalidOperationException($"{method} declares Commands more than once.");
                usesCommands = true;
            }
            else if (!TryDescribeResourceParameter(parameterType, out _))
            {
                throw new InvalidOperationException(
                    $"{method} has an unsupported parameter {parameter.Name}. A startup takes " +
                    "Commands, Res<T> and ResMut<T> parameters.");
            }
            arguments.Add(Expression.Default(parameterType));
        }
        Action runner = Expression.Lambda<Action>(Expression.Call(method, arguments)).Compile();
        return new ManagedStartup($"{method.DeclaringType?.FullName}.{method.Name}", runner);
    }

    private static InvalidOperationException UnsupportedQuery(MethodInfo method) => new(
        $"{method} has an unsupported parameter. Use Query<...> composed from " +
        "Read<T>, Write<T>, OptionalRead<T>, OptionalWrite<T>, and EntityTerm.");

    // =========================================================================
    // File Loading
    // =========================================================================

    /// <summary>
    /// Read the portable PDB beside an assembly, if it was built with these bytes.
    /// </summary>
    /// <remarks>
    /// The PDB and the DLL are separate files written one after the other, so a
    /// reload can see the new assembly next to the previous build's symbols.
    /// Symbols whose ID differs from the one the image records are refused
    /// rather than attached to code they do not describe. Null means "load
    /// without symbols", which costs only debugging, so no failure here may
    /// fail the load.
    /// </remarks>
    private static byte[]? ReadMatchingSymbols(string assemblyPath, byte[] assemblyBytes)
    {
        string symbolsPath = Path.ChangeExtension(assemblyPath, ".pdb");
        if (!File.Exists(symbolsPath))
            return null;
        try
        {
            byte[] symbols = ReadAllBytesWithRetry(symbolsPath);

            // The image records the ID of the PDB it was built with in its
            // CodeView debug directory entry: a GUID plus the entry's stamp.
            using var peReader = new PEReader(new MemoryStream(assemblyBytes));
            DebugDirectoryEntry? codeView = null;
            foreach (DebugDirectoryEntry entry in peReader.ReadDebugDirectory())
            {
                if (entry.Type == DebugDirectoryEntryType.CodeView)
                {
                    codeView = entry;
                    break;
                }
            }
            if (codeView is not DebugDirectoryEntry recorded)
                return null;
            Guid expectedGuid = peReader.ReadCodeViewDebugDirectoryData(recorded).Guid;

            // A portable PDB carries the same pair as its 20-byte ID.
            using var symbolsProvider =
                MetadataReaderProvider.FromPortablePdbStream(new MemoryStream(symbols));
            var header = symbolsProvider.GetMetadataReader().DebugMetadataHeader;
            if (header is null)
                return null;
            var actual = new BlobContentId(header.Id);
            return actual.Guid == expectedGuid && actual.Stamp == recorded.Stamp
                ? symbols
                : null;
        }
        catch (Exception failure) when (failure is IOException
            or UnauthorizedAccessException
            or BadImageFormatException)
        {
            return null;
        }
    }

    /// <summary>Read a just-built DLL, retrying transient compiler file locks.</summary>
    private static byte[] ReadAllBytesWithRetry(string path)
    {
        const int maxAttempts = 10;
        const int delayMs = 50;
        for (var attempt = 1; ; attempt++)
        {
            try
            {
                return File.ReadAllBytes(path);
            }
            catch (IOException) when (attempt < maxAttempts)
            {
                Thread.Sleep(delayMs);
            }
        }
    }
}
