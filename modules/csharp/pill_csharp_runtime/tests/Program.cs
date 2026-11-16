// Executable regression tests for the C# ECS runtime and current C# project.
//
// Responsibilities:
// - Verifies scheduler access derived from every supported query shape.
// - Validates the actual bouncing-ball system discovered from project_cs.dll.
// - Guards native/managed component sizes and field offsets.
// - Checks invalid managed system signatures are rejected before registration.

using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using TracyLive;
using TracyLive.Loader;
// The renderer's components, generated from their Rust registration.
using pill_master_renderer.component;

namespace TracyLive.Tests;

// =============================================================================
// Test-only component and system declarations
// =============================================================================

[StructLayout(LayoutKind.Sequential)]
internal struct TestPosition { public float X, Y; }

[StructLayout(LayoutKind.Sequential)]
internal struct TestVelocity { public float X, Y; }

[StructLayout(LayoutKind.Sequential)]
internal struct TestHealth { public float Value; }

[StructLayout(LayoutKind.Sequential)]
internal struct TestComponent4 { public uint Value; }

[StructLayout(LayoutKind.Sequential)]
internal struct TestComponent5 { public uint Value; }

[StructLayout(LayoutKind.Sequential)]
internal struct TestComponent6 { public uint Value; }

[StructLayout(LayoutKind.Sequential)]
internal struct TestComponent7 { public uint Value; }

[StructLayout(LayoutKind.Sequential)]
internal struct TestComponent8 { public uint Value; }

[StructLayout(LayoutKind.Sequential)]
internal struct CommandOnlyComponent { public uint Value; }

/// <summary>
/// A project component whose fields declare the presentation decorators, used
/// to prove the manifest carries them to the Inspector untouched.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct DecoratedProbe
{
    [Header("Combat")]
    [Tooltip("Current hit points")]
    [Range(0f, 100f)]
    public float Health;

    [Space(10)]
    [Min(0f)]
    public int Ammo;

    [HideInInspector]
    public uint InternalId;
}

/// <summary>
/// An asset mirror the way the codegen emits one: what a handle names is its
/// <c>RustType</c> constant.
/// </summary>
internal sealed class ProbeMeshAsset
{
    public const string RustType = "probe::assets::mesh::ProbeMesh";
}

/// <summary>
/// A project component holding an asset reference, used to prove the manifest
/// describes a <c>Handle&lt;T&gt;</c> field by the Rust type path of T.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct HandleProbe
{
    public TracyLive.Handle<ProbeMeshAsset> Mesh;
    public float Scale;
}

[StructLayout(LayoutKind.Sequential)]
internal struct InvalidBoolComponent { public bool Value; }

/// A deliberately padded sequential layout: the CLR inserts padding before `B`
/// and before `D`, which is exactly what a mis-ordered walk reports wrongly.
[StructLayout(LayoutKind.Sequential)]
internal struct PaddedProbe { public byte A; public long B; public short C; public int D; }

/// An explicit layout that declares no size. The CLR sizes it at 68 bytes; a
/// sequential walk would answer 4.
[StructLayout(LayoutKind.Explicit)]
internal struct ExplicitWithoutSizeProbe { [FieldOffset(64)] public int A; }

/// A component carrying the size-less explicit probe, so the manifest path has
/// something real to refuse.
[StructLayout(LayoutKind.Sequential)]
internal struct ExplicitMemberComponent { public ExplicitWithoutSizeProbe Probe; }

internal static class TestSystems
{
    internal static bool WasRun;

    internal static int QueriesVisited;

    public static void Runner(Query<Write<TestPosition>, Read<TestVelocity>> query) =>
        WasRun = query is not null;

    public static void SingleWriter(Query<Write<TestPosition>> query) { }

    public static void MixedAccess(Query<Write<TestPosition>, Read<TestVelocity>> query) { }

    public static void TripleWriter(
        Query<Write<TestPosition>, Write<TestVelocity>, Write<TestHealth>> query)
    { }

    public static void OptionalAndEntity(
        Query<EntityTerm, Read<TestPosition>, OptionalWrite<TestHealth>> query)
    { }

    public static void EightTerms(
        Query<
            Read<TestPosition>, Write<TestVelocity>, OptionalRead<TestHealth>,
            Write<TestComponent4>, Read<TestComponent5>, Write<TestComponent6>,
            Read<TestComponent7>, OptionalWrite<TestComponent8>> query)
    { }

    public static void DuplicateReadWrite(
        Query<Write<TestPosition>, Read<TestPosition>> query)
    { }

    public static void DuplicateTriple(
        Query<Write<TestPosition>, Write<TestPosition>, Write<TestHealth>> query)
    { }

    public static void DuplicateEntity(Query<EntityTerm, EntityTerm> query) { }

    public static void NoParameters() { }

    public static int NonVoid(Query<Write<TestPosition>> query) => 0;

    public static void Unsupported(string value) { }

    public static void InvalidLayout(Query<Read<InvalidBoolComponent>> query) { }

    public static void ExplicitLayoutMember(Query<Read<ExplicitMemberComponent>> query) { }

    public static void CommandsOnly(Commands commands) { }

    public static void QueryAndCommands(
        Query<Read<TestPosition>> query, Commands commands)
    { }

    public static void TwoQueries(
        Query<Read<TestPosition>> positions, Query<Read<TestVelocity>> velocities)
    {
        foreach (var row in positions)
        {
            _ = row.Read<TestPosition>().X;
            QueriesVisited++;
        }
        foreach (var row in velocities)
        {
            _ = row.Read<TestVelocity>().X;
            QueriesVisited++;
        }
    }

    public static void ThreeQueriesAndCommands(
        Query<Read<TestPosition>> positions, Query<Write<TestVelocity>> velocities,
        Query<Read<TestHealth>> healths, Commands commands)
    { }

    public static void SharedReadQueries(
        Query<Read<TestPosition>> first, Query<Read<TestPosition>> second)
    { }

    public static void ConflictingQueries(
        Query<Write<TestPosition>> first, Query<Read<TestPosition>> second)
    { }

    public static void TwoCommands(
        Query<Read<TestPosition>> query, Commands first, Commands second)
    { }

    public static void SevenParameters(
        Query<Read<TestPosition>> first, Query<Read<TestVelocity>> second,
        Query<Read<TestHealth>> third, Query<Read<TestComponent4>> fourth,
        Query<Read<TestComponent5>> fifth, Query<Read<TestComponent6>> sixth,
        Commands seventh)
    { }

    public static void DespawnSystem(
        Query<EntityTerm, Read<TestVelocity>> query, Commands commands)
    {
        foreach (var row in query)
        {
            commands.DestroyEntity(row.Entity);
            break;
        }
    }
}

/// <summary>
/// A component whose packed layout the manifest cannot model.
/// </summary>
/// <remarks>
/// `NativeLayout` computes a component's size with natural alignment, so this
/// is 8 bytes to the manifest and 5 to the runtime. A column strided by the
/// manifest figure would leave three bytes of every row unwritten, and the
/// mismatch is what `ComponentTypeMetadata<T>` exists to catch.
/// </remarks>
[StructLayout(LayoutKind.Sequential, Pack = 1)]
internal struct PackedProbe
{
    public byte Flag;
    public float Value;
}

internal static unsafe class MockNativeWorld
{
    internal static TestPosition* Positions;
    internal static TestVelocity* Velocities;
    internal static TestHealth* Healths;
    internal static Entity* Entities;
    internal static NativeComponentTicks* PositionTicks;
    internal static NativeComponentTicks* VelocityTicks;
    internal static NativeComponentTicks* HealthTicks;
    internal static uint Length;
    internal static uint ChangeTick;
    /// <summary>Dispatch calls the parallel path made through the mock slot.</summary>
    internal static int ParallelDispatchCalls;
    internal static ulong NextEntityId;
    internal static int QueuedCreates;
    internal static int LastCreateComponentCount;
    internal static int QueuedDestroys;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte EntityCount(uint* output)
    {
        if (output is null)
            return 5;
        *output = Length;
        return 0;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte GetComponentChunk(
        ulong key, ulong keyHigh, byte mode, uint index, NativeComponentChunk* output)
    {
        if (index != 0 || !TryResolveChunk(key, keyHigh, mode, out var chunk))
            return 0;
        *output = chunk;
        return 1;
    }

    /// <summary>Mirror of the host's archetype-scoped chunk entry point.</summary>
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte GetArchetypeChunk(
        ulong archetypeLow, ulong archetypeHigh, ulong key, ulong keyHigh, byte mode,
        NativeComponentChunk* output)
    {
        if (archetypeLow != 7 || archetypeHigh != 11)
            return 0;
        if (mode == 2)
        {
            *output = EntityChunk(Entities);
            return 1;
        }
        if (!TryResolveChunk(key, keyHigh, mode, out var chunk))
            return 0;
        *output = chunk;
        return 1;
    }

    /// <summary>Shared component-key matching for both chunk entry points.</summary>
    private static bool TryResolveChunk(
        ulong key, ulong keyHigh, byte mode, out NativeComponentChunk chunk)
    {
        chunk = default;
        if (key == Engine.ComponentKey(typeof(TestPosition)) &&
            keyHigh == Engine.ComponentKeyHigh(typeof(TestPosition)) &&
            (mode == 0 || mode == 1))
        {
            chunk = Chunk(Positions, PositionTicks, sizeof(TestPosition));
            return true;
        }
        if (key == Engine.ComponentKey(typeof(TestVelocity)) &&
            keyHigh == Engine.ComponentKeyHigh(typeof(TestVelocity)) && mode == 0)
        {
            chunk = Chunk(Velocities, VelocityTicks, sizeof(TestVelocity));
            return true;
        }
        if (key == Engine.ComponentKey(typeof(TestHealth)) &&
            keyHigh == Engine.ComponentKeyHigh(typeof(TestHealth)) &&
            (mode == 0 || mode == 1) && Healths != null)
        {
            chunk = Chunk(Healths, HealthTicks, sizeof(TestHealth));
            return true;
        }
        return false;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte GetEntityChunk(uint index, NativeComponentChunk* output)
    {
        if (index != 0)
            return 0;
        *output = EntityChunk(Entities);
        return 1;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte ReserveEntity(Entity* output)
    {
        *output = new Entity(++NextEntityId, 0);
        return 1;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte QueueCreate(Entity* entity, NativeComponentBlob* blobs, uint count)
    {
        if (entity is null || (count != 0 && blobs is null))
            return 6;
        QueuedCreates++;
        LastCreateComponentCount = checked((int)count);
        return 1;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte QueueDestroy(Entity* entity)
    {
        if (entity->Generation == 99)
            return 5;
        QueuedDestroys++;
        return 1;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte QueueAddComponent(
        Entity* entity, ulong low, ulong high, byte* data, uint size) => 1;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte QueueRemoveComponent(Entity* entity, ulong low, ulong high) => 1;

    internal static EngineApi Api() => new()
    {
        AbiVersion = LoaderInterop.InteropContractVersion,
        TableSize = (uint)sizeof(EngineApi),
        EntityCount = &EntityCount,
        GetComponentChunk = &GetComponentChunk,
        GetArchetypeChunk = &GetArchetypeChunk,
        GetEntityChunk = &GetEntityChunk,
        ReserveEntity = &ReserveEntity,
        QueueCreate = &QueueCreate,
        QueueDestroy = &QueueDestroy,
        QueueAddComponent = &QueueAddComponent,
        QueueRemoveComponent = &QueueRemoveComponent,
        MirrorMethodCount = &MirrorMethodCount,
        CopyMirrorMethods = &CopyMirrorMethods,
        MirrorEpoch = &MirrorEpoch,
        CurrentScopeToken = &CurrentScopeToken,
        ParallelFor = &ParallelFor,
        TakeMirrorText = &TakeMirrorText,
        GetNativeResource = &GetNativeResource,
    };

    // ---- Mirror ABI ------------------------------------------------------

    /// <summary>The message the mock host's last-error channel holds, if any.</summary>
    internal static string? MirrorError;

    /// <summary>The string the mock host's return channel holds, if any.</summary>
    internal static string? MirrorReturnString;

    /// <summary>Native copy of the last text handed out, kept until the next take.</summary>
    private static IntPtr _heldText;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte TakeMirrorText(byte kind, byte** data, uint* length)
    {
        string? text = kind == 0 ? MirrorError : MirrorReturnString;
        if (kind == 0)
            MirrorError = null;
        else
            MirrorReturnString = null;
        if (text is null)
            return 1;
        if (_heldText != IntPtr.Zero)
            Marshal.FreeHGlobal(_heldText);
        byte[] bytes = System.Text.Encoding.UTF8.GetBytes(text);
        _heldText = Marshal.AllocHGlobal(Math.Max(bytes.Length, 1));
        Marshal.Copy(bytes, 0, _heldText, bytes.Length);
        *data = (byte*)_heldText;
        *length = (uint)bytes.Length;
        return 0;
    }

    /// <summary>What the mock host answers a native resource request with.</summary>
    internal static byte NativeResourceStatus;

    /// <summary>The address the mock host hands out for a native resource.</summary>
    internal static IntPtr NativeResourceAddress = (IntPtr)0x5000;

    /// <summary>The mode the last native resource request asked for.</summary>
    internal static byte LastNativeResourceMode;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte GetNativeResource(ulong low, ulong high, byte mode, void** output)
    {
        LastNativeResourceMode = mode;
        if (NativeResourceStatus != 0)
            return NativeResourceStatus;
        *output = (void*)NativeResourceAddress;
        return 0;
    }

    /// <summary>The slots the last probe call received, copied out.</summary>
    internal static byte[] ProbedSlots = [];

    /// <summary>
    /// A trampoline that records its slots and answers with the sum of the
    /// first slot's <c>uint</c> and the string's length - enough to show the
    /// arguments arrived and the result travels back.
    /// </summary>
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte MirrorProbe(byte* args, byte* ret)
    {
        ProbedSlots = new ReadOnlySpan<byte>(args, 16 * 8).ToArray();
        uint first = *(uint*)args;
        uint stringLength = *(uint*)(args + 16 + 8);
        *(uint*)ret = first + stringLength;
        return 0;
    }

    /// <summary>A trampoline that adds its first slot to one, touching no managed memory.</summary>
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte MirrorIncrement(byte* args, byte* ret)
    {
        *(uint*)ret = *(uint*)args + 1;
        return 0;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte MirrorFails(byte* args, byte* ret)
    {
        MirrorError = "the OBJ has no triangles";
        return 1;
    }

    /// <summary>Boxes the mock trampolines handed out and dropped.</summary>
    internal static int ObjectsCreated;
    internal static int ObjectsDropped;
    internal static IntPtr LastDropped;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte MirrorMakeObject(byte* args, byte* ret)
    {
        ObjectsCreated++;
        *(IntPtr*)ret = (IntPtr)(0x9000 + ObjectsCreated);
        return 0;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte MirrorDropObject(byte* args, byte* ret)
    {
        ObjectsDropped++;
        LastDropped = *(IntPtr*)args;
        return 0;
    }

    /// <summary>Records the box a by-value call consumed.</summary>
    internal static IntPtr LastConsumed;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte MirrorConsumeObject(byte* args, byte* ret)
    {
        LastConsumed = *(IntPtr*)args;
        return 0;
    }

    /// <summary>
    /// The epoch the host would bump when it republishes its mirror-method
    /// table; a test bumps it to simulate a rebind.
    /// </summary>
    internal static uint MirrorEpochValue;

    /// <summary>
    /// The invocation token the host would stamp into every chunk it issues.
    /// </summary>
    /// <remarks>
    /// A test bumps this to simulate the next scheduled system invocation, so
    /// a chunk fetched under the old value becomes exactly what a retained
    /// chunk is in a real frame.
    /// </remarks>
    internal static uint ScopeTokenValue = 1;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static uint CurrentScopeToken() => ScopeTokenValue;

    /// <summary>
    /// The host's parallel dispatch, run sequentially on the calling thread:
    /// deterministic for tests, while the real host fans the same callbacks
    /// out across its Rayon pool.
    /// </summary>
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte ParallelFor(
        delegate* unmanaged[Cdecl]<nint, uint, void> callback, nint state, uint count)
    {
        ParallelDispatchCalls++;
        for (uint index = 0; index < count; index++)
            callback(state, index);
        return ParallelDispatch.Ok;
    }

    /// <summary>The rows <see cref="CopyMirrorMethods"/> serves.</summary>
    internal static MirrorMethodEntry[] MirroredRows = [];

    /// <summary>Managed shape of the `(row, out data, out length)` trampoline.</summary>
    internal delegate byte ViewDelegate(IntPtr row, out IntPtr data, out IntPtr length);

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static uint MirrorMethodCount() => (uint)MirroredRows.Length;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static uint CopyMirrorMethods(MirrorMethodEntry* entries, uint max)
    {
        uint written = 0;
        foreach (MirrorMethodEntry row in MirroredRows)
        {
            if (written >= max)
                break;
            entries[written] = row;
            written++;
        }
        return written;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static uint MirrorEpoch() => MirrorEpochValue;

    /// <summary>Publish one mirrored method and bump the epoch, as the host does.</summary>
    internal static void PublishMirrorRow(string typeName, string method, IntPtr address)
    {
        MirrorMethodEntry row = default;
        row.TypeName = Marshal.StringToHGlobalAnsi(typeName);
        row.Method = Marshal.StringToHGlobalAnsi(method);
        row.Address = address;
        MirroredRows = [row];
        MirrorEpochValue++;
    }

    internal static void ResetCommands()
    {
        NextEntityId = 0;
        QueuedCreates = 0;
        LastCreateComponentCount = 0;
        QueuedDestroys = 0;
    }

    // Trampolines shaped like the heap-field accessor ABI, for the
    // MirrorMethods raw-call tests. Each echoes fixed values, so a signature
    // mismatch between helper and trampoline shows up as wrong numbers rather
    // than being masked.

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte AccessorView(IntPtr row, IntPtr* data, IntPtr* length)
    {
        *data = row + 32;
        *length = 5;
        return 0;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte AccessorViewSecond(IntPtr row, IntPtr* data, IntPtr* length)
    {
        *data = row + 16;
        *length = 9;
        return 0;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte AccessorResize(IntPtr row, IntPtr count) => count == 7 ? (byte)0 : (byte)1;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte AccessorItem(IntPtr row, IntPtr index, IntPtr* data, IntPtr* length)
    {
        *data = row + 8;
        *length = index + 1;
        return 2;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte AccessorSetItem(
        IntPtr row, IntPtr index, IntPtr utf8, IntPtr length) =>
        length == 3 && index == (IntPtr)4 ? (byte)0 : (byte)1;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte AccessorUtf8Write(IntPtr row, IntPtr utf8, IntPtr length) =>
        utf8 != IntPtr.Zero && length == 3 ? (byte)0 : (byte)1;

    /// <summary>Element column the accessor benchmark's view trampoline reports.</summary>
    internal static float* AccessorElements;
    internal static int AccessorElementCount;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    internal static byte AccessorElementsView(IntPtr row, IntPtr* data, IntPtr* length)
    {
        *data = (IntPtr)AccessorElements;
        *length = AccessorElementCount;
        return 0;
    }

    private static NativeComponentChunk Chunk(
        void* data, NativeComponentTicks* ticks, int elementSize) => new()
        {
            ArchetypeLow = 7,
            ArchetypeHigh = 11,
            Data = (IntPtr)data,
            Length = Length,
            ElementSize = checked((uint)elementSize),
            Ticks = (IntPtr)ticks,
            ChangeTick = ChangeTick,
            ScopeToken = ScopeTokenValue,
        };

    /// <summary>
    /// Entity column chunk: the native ABI hands entity rows out through the
    /// const <c>Entities</c> slot and leaves the writable <c>Data</c> slot
    /// null, so the mock reproduces exactly that shape.
    /// </summary>
    private static NativeComponentChunk EntityChunk(void* data) => new()
    {
        ArchetypeLow = 7,
        ArchetypeHigh = 11,
        Entities = (IntPtr)data,
        Length = Length,
        ElementSize = checked((uint)sizeof(Entity)),
        Ticks = IntPtr.Zero,
        ChangeTick = ChangeTick,
        ScopeToken = ScopeTokenValue,
    };
}

// =============================================================================
// Test runner
// =============================================================================

internal static class Program
{
    private static int _passed;

    private static MethodInfo Method(string name) =>
        typeof(TestSystems).GetMethod(name)
        ?? throw new InvalidOperationException($"Missing test method {name}");

    private static void Assert(bool condition, string message)
    {
        if (!condition)
            throw new InvalidOperationException(message);
    }

    private static void Equal<T>(T actual, T expected, string message)
        where T : IEquatable<T>
    {
        if (!actual.Equals(expected))
            throw new InvalidOperationException(
                $"{message}: expected {expected}, got {actual}");
    }

    private static void Throws<T>(Action action, string message) where T : Exception
    {
        try
        {
            action();
        }
        catch (T)
        {
            return;
        }

        throw new InvalidOperationException(message);
    }

    /// <summary>Expect a refusal whose message names the reason.</summary>
    private static void Refused(Action action, string expected, string message)
    {
        try
        {
            action();
        }
        catch (Exception exception)
        {
            Assert(exception.Message.Contains(expected), $"{message}: {exception.Message}");
            return;
        }

        throw new InvalidOperationException($"{message}: nothing was refused");
    }

    private static void Test(string name, Action test)
    {
        test();
        _passed++;
        Console.WriteLine($"pass: {name}");
    }

    /// <summary>
    /// Compile one probe source into a loadable assembly, optionally with the
    /// AOT registry generator run over it, so a test can compare the two
    /// discovery postures over exactly the same declarations.
    /// </summary>
    private static Assembly CompileProbe(string source, string assemblyName, bool withGeneratedRegistry)
    {
        MetadataReference[] references = AppDomain.CurrentDomain.GetAssemblies()
            .Where(assembly => !assembly.IsDynamic && !string.IsNullOrEmpty(assembly.Location))
            .Select(assembly => MetadataReference.CreateFromFile(assembly.Location))
            .ToArray();
        CSharpCompilation compilation = CSharpCompilation.Create(
            assemblyName,
            [CSharpSyntaxTree.ParseText(source)],
            references,
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, allowUnsafe: true));
        if (withGeneratedRegistry)
        {
            GeneratorDriver driver = CSharpGeneratorDriver.Create(
                new PillCSharpRuntimeMacros.EcsAotRegistryGenerator().AsSourceGenerator());
            GeneratorRunResult result = driver.RunGenerators(compilation).GetRunResult().Results.Single();
            Diagnostic[] refusals = result.Diagnostics
                .Where(diagnostic => diagnostic.Severity == DiagnosticSeverity.Error)
                .ToArray();
            Assert(refusals.Length == 0, string.Join("; ", refusals.Select(refusal => refusal.ToString())));
            // Only the registry tree: the export forwarders the generator also
            // emits are for a NativeAOT publish, and they cannot be compiled
            // into a dynamically loaded probe.
            compilation = compilation.AddSyntaxTrees(
                result.GeneratedSources
                    .Where(entry => entry.HintName == "GeneratedAotRegistry.g.cs")
                    .Select(entry => entry.SyntaxTree));
        }
        using var stream = new MemoryStream();
        var emitted = compilation.Emit(stream);
        Assert(emitted.Success, string.Join("; ", emitted.Diagnostics
            .Where(diagnostic => diagnostic.Severity == DiagnosticSeverity.Error)
            .Select(diagnostic => diagnostic.ToString())));
        return Assembly.Load(stream.ToArray());
    }

    public static unsafe int Main(string[] args)
    {
        // Benchmark mode times the query path instead of asserting on it, so
        // the regression suite stays deterministic. Run it explicitly with
        // `dotnet run -c Release --project ... -- bench-query <iterations>`.
        if (args.Length > 0 && args[0] == "bench-query")
        {
            int iterations = args.Length > 1 ? int.Parse(args[1]) : 10_000;
            RunQueryBenchmark([10_000u, 100_000u]);
            RunAccessorBenchmark(iterations);
            return 0;
        }

        try
        {
            Test("current project discovers native and descriptor component systems", () =>
            {
                var systems = ProjectHost.DiscoverSystems(typeof(BallPhysicsSystem).Assembly);
                // The physics step, the three-query spline pass - the same
                // system shape the Rust project uses - and the systems that
                // fill the world up to the target counts, plus the four bench
                // systems BenchSystems.cs declares for the benchmarks.
                Equal(systems.Length, 10, "unexpected project system count");
                Assert(systems.Any(system => system.Name == "TracyLive.BallPhysicsSystem.Run"),
                    "ball physics system was not discovered");
                Assert(systems.Any(system => system.Name == "TracyLive.BallSpawnSystem.Run"),
                    "ball spawn system was not discovered");
                Assert(systems.Any(system => system.Name == "TracyLive.SplinePathSystem.Run"),
                    "spline path system was not discovered");
                Assert(systems.Any(system => system.Name == "TracyLive.SplineSampleSpawnSystem.Run"),
                    "spline sample spawn system was not discovered");
                Assert(systems.Any(system => system.Name == "TracyLive.SplineSpawnSystem.Run"),
                    "spline spawn system was not discovered");
            });

            Test("component manifest separates runtime mirrors from project components", () =>
            {
                var systems = ProjectHost.DiscoverSystems(typeof(BallPhysicsSystem).Assembly);
                using var json = System.Text.Json.JsonDocument.Parse(
                    ProjectManifestBuilder.Build(systems, typeof(BallPhysicsSystem).Assembly));
                var components = json.RootElement.EnumerateArray().ToArray();
                // The project's own structs (PhysicsState, SplineSample), its
                // resource (SimulationTime) and the three bench components
                // (BenchPosition, BenchVelocity, BenchHealth), plus the mirrors
                // of the modules the project loads: the renderer data's
                // components and its `MeshVertex` value type, the spline
                // extension's `Spline`, `Vector3f` and `OmoMO`, and the
                // pill_dummy_color extension's `Tint` and `TestStruct`.
                Equal(components.Length, 17, "unexpected manifest component count: " +
                    string.Join(" | ", components.Select(component =>
                        component.GetProperty("full_name").GetString())));
                var transform = components.Single(component =>
                    component.GetProperty("full_name").GetString() == "pill_master_renderer.component.TransformComponent");
                var renderable = components.Single(component =>
                    component.GetProperty("full_name").GetString() == "pill_master_renderer.component.MeshRendererComponent");
                var physics = components.Single(component =>
                    component.GetProperty("full_name").GetString() == "TracyLive.PhysicsState");
                var sample = components.Single(component =>
                    component.GetProperty("full_name").GetString() == "TracyLive.SplineSample");
                var spline = components.Single(component =>
                    component.GetProperty("full_name").GetString() == "pill_spline.Spline");
                // Generated mirrors bind by their generated identity, exactly as
                // an extension's do; the shared-schema path is for hand-written
                // `[EcsSharedComponent]` mirrors, which the renderer no longer has.
                Assert(!transform.GetProperty("shared").GetBoolean(),
                    "generated TransformComponent mirror must bind by identity, not as a shared schema");
                Assert(!renderable.GetProperty("shared").GetBoolean(),
                    "generated MeshRendererComponent mirror must bind by identity, not as a shared schema");
                Assert(!physics.GetProperty("shared").GetBoolean(),
                    "project-owned PhysicsState must be descriptor-registered");
                Assert(!sample.GetProperty("shared").GetBoolean(),
                    "project-owned SplineSample must be descriptor-registered");
                Equal(sample.GetProperty("size").GetInt32(), 4, "SplineSample size mismatch");
                Equal(sample.GetProperty("alignment").GetInt32(), 4,
                    "SplineSample alignment mismatch");
                Equal(sample.GetProperty("fields").GetArrayLength(), 1,
                    "SplineSample field schema mismatch");
                // The module mirror is declared in the project (not shared on
                // the managed side); the Rust host resolves it to the module's
                // native binding by stable identity instead.
                Assert(!spline.GetProperty("shared").GetBoolean(),
                    "module Spline mirror must be project-owned on the managed side");
                Equal(spline.GetProperty("size").GetInt32(), 200,
                    "Spline mirror size mismatch (16 * Vector3f + uint + float)");
            });

            Test("manifest includes project structs used only by commands", () =>
            {
                using var json = System.Text.Json.JsonDocument.Parse(
                    ProjectManifestBuilder.Build([], typeof(CommandOnlyComponent).Assembly));
                Assert(json.RootElement.EnumerateArray().Any(component =>
                        component.GetProperty("full_name").GetString() ==
                        typeof(CommandOnlyComponent).FullName),
                    "command-only project component was omitted from the manifest");
            });

            Test("manifest carries a field's presentation decorators", () =>
            {
                using var json = System.Text.Json.JsonDocument.Parse(
                    ProjectManifestBuilder.Build([], typeof(DecoratedProbe).Assembly));
                var probe = json.RootElement.EnumerateArray().Single(component =>
                    component.GetProperty("full_name").GetString() ==
                    typeof(DecoratedProbe).FullName);
                var fields = probe.GetProperty("fields").EnumerateArray().ToArray();
                var health = fields.Single(field =>
                    field.GetProperty("name").GetString() == "Health");
                var decorators = health.GetProperty("decorators")
                    .EnumerateArray().ToArray();
                // Declaration order, which is what the Inspector draws by.
                Equal(string.Join(",", decorators.Select(decorator =>
                        decorator.GetProperty("kind").GetString())),
                    "header,tooltip,range", "Health decorator order mismatch");
                Assert(decorators[0].GetProperty("text").GetString() == "Combat",
                    "the header text was not carried"); Equal(decorators[2].GetProperty("min").GetSingle(), 0f,
                    "the range minimum was not carried");
                Equal(decorators[2].GetProperty("max").GetSingle(), 100f,
                    "the range maximum was not carried");
                var ammo = fields.Single(field =>
                    field.GetProperty("name").GetString() == "Ammo");
                Equal(string.Join(",", ammo.GetProperty("decorators")
                        .EnumerateArray().Select(decorator =>
                            decorator.GetProperty("kind").GetString())),
                    "space,min", "Ammo decorator order mismatch");
                Equal(ammo.GetProperty("decorators")[0].GetProperty("number").GetSingle(), 10f,
                    "the gap height was not carried");
            });

            Test("manifest describes a handle field by its asset's Rust type", () =>
            {
                using var json = System.Text.Json.JsonDocument.Parse(
                    ProjectManifestBuilder.Build([], typeof(HandleProbe).Assembly));
                var probe = json.RootElement.EnumerateArray().Single(component =>
                    component.GetProperty("full_name").GetString() ==
                    typeof(HandleProbe).FullName);
                var mesh = probe.GetProperty("fields").EnumerateArray().Single(field =>
                    field.GetProperty("name").GetString() == "Mesh");
                Equal(mesh.GetProperty("primitive_type").GetString() ?? string.Empty,
                    "handle:probe::assets::mesh::ProbeMesh", "the handle's type was not carried");
                Equal(mesh.GetProperty("size").GetInt32(), 8, "a handle is two uints");
                Equal(mesh.GetProperty("fields").GetArrayLength(), 0,
                    "a handle is a leaf, not a nested struct");
            });

            Test("ball physics declares PhysicsState and TransformComponent writes", () =>
            {
                var method = typeof(BallPhysicsSystem).GetMethod(nameof(BallPhysicsSystem.Run))
                    ?? throw new InvalidOperationException("BallPhysicsSystem.Run is missing");
                var system = ProjectHost.CreateSystem(method);
                var componentAccesses = system.Accesses.Where(access => access.Kind == 0).ToArray();

                Equal(system.Accesses.Count(access => access.Kind == 0), 2, "unexpected ball access count");
                Assert(system.Accesses.All(access => access.Mode == 1),
                    "ball physics accesses must all be writable");
                Equal(componentAccesses[0].ComponentKey, Engine.ComponentKey(typeof(PhysicsState)),
                    "wrong PhysicsState key");
                Equal(componentAccesses[0].ComponentKeyHigh,
                    Engine.ComponentKeyHigh(typeof(PhysicsState)), "wrong PhysicsState high key");
                Equal(componentAccesses[1].ComponentKey, Engine.ComponentKey(typeof(TransformComponent)),
                    "wrong TransformComponent key");
                Equal(componentAccesses[1].ComponentKeyHigh,
                    Engine.ComponentKeyHigh(typeof(TransformComponent)), "wrong TransformComponent high key");
            });

            Test("managed Commands parameter is reflected into system metadata", () =>
            {
                var commandsOnly = ProjectHost.CreateSystem(Method(nameof(TestSystems.CommandsOnly)));
                Assert(commandsOnly.UsesCommands, "Commands-only system did not declare commands");
                Equal(commandsOnly.Accesses.Length, 0, "Commands-only system has component access");
                var mixed = ProjectHost.CreateSystem(Method(nameof(TestSystems.QueryAndCommands)));
                Assert(mixed.UsesCommands, "query plus Commands did not declare commands");
                Equal(mixed.Accesses.Length, 1, "query plus Commands lost query access");
            });

            Test("multiple query parameters merge into one scheduler access list", () =>
            {
                var system = ProjectHost.CreateSystem(
                    Method(nameof(TestSystems.ThreeQueriesAndCommands)));
                Assert(system.UsesCommands, "multi-query system lost its Commands flag");
                Equal(system.Queries.Length, 3, "query descriptors were not preserved");
                Equal(system.Accesses.Length, 3, "unexpected merged access count");
                Equal(system.Accesses[0],
                    new ManagedAccess(Engine.ComponentKey(typeof(TestPosition)),
                        Engine.ComponentKeyHigh(typeof(TestPosition)), 0, 0),
                    "wrong first-query read access");
                Equal(system.Accesses[1],
                    new ManagedAccess(Engine.ComponentKey(typeof(TestVelocity)),
                        Engine.ComponentKeyHigh(typeof(TestVelocity)), 1, 0),
                    "wrong second-query write access");
                Equal(system.Accesses[2],
                    new ManagedAccess(Engine.ComponentKey(typeof(TestHealth)),
                        Engine.ComponentKeyHigh(typeof(TestHealth)), 0, 0),
                    "wrong third-query read access");
            });

            Test("a component read by two queries merges into a single access entry", () =>
            {
                var system = ProjectHost.CreateSystem(
                    Method(nameof(TestSystems.SharedReadQueries)));
                Equal(system.Accesses.Length, 1, "shared reads must merge into one entry");
                Equal(system.Accesses[0].Mode, 0, "shared reads must stay read-only");
            });

            Test("a write shared across query parameters is rejected", () =>
                Throws<InvalidOperationException>(
                    () => ProjectHost.CreateSystem(Method(nameof(TestSystems.ConflictingQueries))),
                    "cross-query write must be rejected"));

            Test("a second Commands parameter is rejected", () =>
                Throws<InvalidOperationException>(
                    () => ProjectHost.CreateSystem(Method(nameof(TestSystems.TwoCommands))),
                    "a second Commands parameter must be rejected"));

            Test("a system beyond the parameter budget is rejected", () =>
                Throws<InvalidOperationException>(
                    () => ProjectHost.CreateSystem(Method(nameof(TestSystems.SevenParameters))),
                    "systems beyond the parameter budget must be rejected"));

            Test("ball spawn system queues the missing balls and nothing else", () =>
            {
                MockNativeWorld.ResetCommands();
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                var method = typeof(BallSpawnSystem).GetMethod(nameof(BallSpawnSystem.Run))
                    ?? throw new InvalidOperationException("BallSpawnSystem.Run is missing");
                // The mock world has no PhysicsState column, so the spawn system
                // sees an empty world and queues the full scene, which is also
                // what a fresh project build does.
                ProjectHost.CreateSystem(method).Run();
                Equal(MockNativeWorld.QueuedCreates, 5, "spawn create count mismatch");
                Equal(MockNativeWorld.LastCreateComponentCount, 3,
                    "each ball must contain PhysicsState, TransformComponent and MeshRendererComponent");
                Equal(MockNativeWorld.NextEntityId, 5UL, "spawn did not reserve unique entities");
            });

            Test("a table from another interop contract is refused before it is copied", () =>
            {
                EngineApi* stale = stackalloc EngineApi[1];
                *stale = MockNativeWorld.Api();
                stale->AbiVersion = LoaderInterop.InteropContractVersion - 1;
                Throws<InvalidOperationException>(
                    () => Engine.Bind(stale), "a table from another contract must be refused");
                EngineApi* resized = stackalloc EngineApi[1];
                *resized = MockNativeWorld.Api();
                resized->TableSize -= 8;
                Throws<InvalidOperationException>(
                    () => Engine.Bind(resized), "a table of another size must be refused");
            });

            Test("managed Commands reports stale entity generations", () =>
            {
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                Throws<InvalidOperationException>(
                    () => new Commands().DestroyEntity(new Entity(7, 99)),
                    "stale entity command should be rejected");
            });

            Test("a query system can queue despawn for its current entity", () =>
            {
                TestVelocity* velocities = stackalloc TestVelocity[1];
                Entity* entities = stackalloc Entity[1];
                velocities[0].X = 1;
                entities[0] = new Entity(55, 2);
                MockNativeWorld.Velocities = velocities;
                MockNativeWorld.Entities = entities;
                MockNativeWorld.Length = 1;
                MockNativeWorld.ResetCommands();
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                ProjectHost.CreateSystem(Method(nameof(TestSystems.DespawnSystem))).Run();
                Equal(MockNativeWorld.QueuedDestroys, 1,
                    "despawn system did not enqueue destruction");
            });

            Test("project and shared component layouts match their manifests", () =>
            {
                Equal(Marshal.SizeOf<PhysicsState>(), 28, "PhysicsState size mismatch");
                Equal(Marshal.OffsetOf<PhysicsState>(nameof(PhysicsState.DeltaTime)).ToInt32(), 0,
                    "PhysicsState.DeltaTime offset mismatch");
                Equal(Marshal.OffsetOf<PhysicsState>(nameof(PhysicsState.Radius)).ToInt32(), 20,
                    "PhysicsState.Radius offset mismatch");
                Equal(Marshal.OffsetOf<PhysicsState>(nameof(PhysicsState.Active)).ToInt32(), 24,
                    "PhysicsState.Active offset mismatch");

                Equal(Marshal.SizeOf<MeshRendererComponent>(), 16, "MeshRendererComponent size mismatch");
                Equal(Marshal.OffsetOf<MeshRendererComponent>(nameof(MeshRendererComponent.Material)).ToInt32(), 8,
                    "MeshRendererComponent.Material offset mismatch");
                Equal(Marshal.SizeOf<TransformComponent>(), 40, "TransformComponent size mismatch");
            });

            Test("padded sequential layouts agree with Marshal", () =>
            {
                // Pins the order of the align-vs-test steps: `B` follows a byte
                // field, so its offset exists only after the alignment step has
                // run for `B` itself.
                Equal(Marshal.SizeOf<PaddedProbe>(), 24, "the CLR sizes PaddedProbe at 24");
                Equal(NativeLayout.SizeOf(typeof(PaddedProbe)), Marshal.SizeOf<PaddedProbe>(),
                    "NativeLayout.SizeOf must agree with Marshal");
                foreach (string field in new[] { nameof(PaddedProbe.A), nameof(PaddedProbe.B),
                                                 nameof(PaddedProbe.C), nameof(PaddedProbe.D) })
                {
                    Equal(Marshal.OffsetOf<PaddedProbe>(field).ToInt32(),
                        NativeLayout.FieldOffset(typeof(PaddedProbe), field),
                        $"PaddedProbe.{field} offset must agree with Marshal");
                }
            });

            Test("explicit layouts answer offsets from their attributes and refuse a missing size", () =>
            {
                Equal(Marshal.SizeOf<ExplicitWithoutSizeProbe>(), 68,
                    "the CLR sizes the size-less explicit probe at 68");
                Equal(NativeLayout.FieldOffset(typeof(ExplicitWithoutSizeProbe),
                        nameof(ExplicitWithoutSizeProbe.A)), 64,
                    "an explicit field's offset comes from its FieldOffset attribute");
                // The size, by contrast, is unknowable without the declared
                // stride - answering 4 is the corruption this refuses. The
                // refusal fires at term creation, before anything registers,
                // and the nested-member case reaches it through the same walk.
                Throws<InvalidOperationException>(
                    () => NativeLayout.SizeOf(typeof(ExplicitWithoutSizeProbe)),
                    "an explicit layout with no declared size cannot be measured");
                Throws<InvalidOperationException>(
                    () => ProjectHost.CreateSystem(
                        Method(nameof(TestSystems.ExplicitLayoutMember))),
                    "a query term over a size-less explicit component must be refused");
            });

            Test("native change tracking ABI layout is stable", () =>
            {
                Equal(Marshal.SizeOf<NativeComponentTicks>(), 8,
                    "NativeComponentTicks size mismatch");
                Equal(Marshal.OffsetOf<NativeComponentTicks>(nameof(NativeComponentTicks.Changed))
                    .ToInt32(), 4, "NativeComponentTicks.Changed offset mismatch");
                Equal(Marshal.SizeOf<NativeComponentChunk>(), 56,
                    "NativeComponentChunk size mismatch");
                Equal(Marshal.OffsetOf<NativeComponentChunk>(nameof(NativeComponentChunk.Entities))
                    .ToInt32(), 24, "NativeComponentChunk.Entities offset mismatch");
                Equal(Marshal.OffsetOf<NativeComponentChunk>(nameof(NativeComponentChunk.Ticks))
                    .ToInt32(), 40, "NativeComponentChunk.Ticks offset mismatch");
                Equal(Marshal.OffsetOf<NativeComponentChunk>(nameof(NativeComponentChunk.ChangeTick))
                    .ToInt32(), 48, "NativeComponentChunk.ChangeTick offset mismatch");
                // The scope token rides in padding the struct already carried,
                // which is what makes the retention guard free; if this offset
                // ever moves past 52 the field stopped being free.
                Equal(Marshal.OffsetOf<NativeComponentChunk>(nameof(NativeComponentChunk.ScopeToken))
                    .ToInt32(), 52, "NativeComponentChunk.ScopeToken offset mismatch");
            });

            Test("a chunk issued to an earlier invocation is refused", () =>
            {
                // The host stamps every chunk with the invocation that asked
                // for it. Bumping the mock's token between the fetch and the
                // access is exactly what a retained chunk sees in a real
                // frame: the storage behind it may have moved, been freed, or
                // been unloaded with its module.
                TestPosition* positions = stackalloc TestPosition[1];
                NativeComponentTicks* ticks = stackalloc NativeComponentTicks[1];
                MockNativeWorld.Positions = positions;
                MockNativeWorld.PositionTicks = ticks;
                MockNativeWorld.Length = 1;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                MockNativeWorld.ScopeTokenValue = 1;
                var enumerator = new Query<Write<TestPosition>>().GetEnumerator();
                Assert(enumerator.MoveNext(), "the query must yield a row");

                MockNativeWorld.ScopeTokenValue = 2;
                bool refused = false;
                try
                {
                    enumerator.Current.Write<TestPosition>().X = 1.0f;
                }
                catch (InvalidOperationException)
                {
                    refused = true;
                }
#if DEBUG
                Assert(refused, "a chunk from a finished invocation must be refused");
#else
                // Compiled out of `Release` configurations on purpose: validating every
                // row access would cost the data plane the property that makes
                // it worth having. Asserting the absence keeps the posture
                // deliberate rather than accidental.
                Assert(!refused, "the retention guard must not survive into a `Release` configuration");
#endif

                // The guard must not fire on the ordinary path, which is the
                // half of the contract the rejection alone cannot prove.
                MockNativeWorld.ScopeTokenValue = 3;
                var current = new Query<Write<TestPosition>>().GetEnumerator();
                Assert(current.MoveNext(), "the query must yield a row");
                current.Current.Write<TestPosition>().X = 2.0f;
                Equal(positions[0].X, 2.0f, "a chunk used inside its own invocation must work");

                MockNativeWorld.ScopeTokenValue = 1;
                MockNativeWorld.Positions = null;
                MockNativeWorld.PositionTicks = null;
                MockNativeWorld.Length = 0;
            });

            Test("a component whose manifest size disagrees with the runtime is refused", () =>
            {
                // ComponentTypeMetadata<T> is where the manifest's predicted
                // layout meets the runtime's own size. A packed struct is the
                // reachable case: NativeLayout computes it with natural
                // alignment, so the column stride and the row write disagree.
                Throws<TypeInitializationException>(
                    () => Engine.ComponentSizeOf<PackedProbe>(),
                    "a packed component must be refused rather than strided wrongly");
            });

            Test("single-term query reports one write", () =>
            {
                var system = ProjectHost.CreateSystem(Method(nameof(TestSystems.SingleWriter)));
                Equal(system.Accesses.Length, 1, "unexpected access count");
                Equal(system.Accesses[0],
                    new ManagedAccess(Engine.ComponentKey(typeof(TestPosition)), Engine.ComponentKeyHigh(typeof(TestPosition)), 1, 0),
                    "wrong write access");
            });

            Test("composed query reports write then read", () =>
            {
                var system = ProjectHost.CreateSystem(Method(nameof(TestSystems.MixedAccess)));
                Equal(system.Accesses.Length, 2, "unexpected access count");
                Equal(system.Accesses[0],
                    new ManagedAccess(Engine.ComponentKey(typeof(TestPosition)), Engine.ComponentKeyHigh(typeof(TestPosition)), 1, 0),
                    "wrong writable access");
                Equal(system.Accesses[1],
                    new ManagedAccess(Engine.ComponentKey(typeof(TestVelocity)), Engine.ComponentKeyHigh(typeof(TestVelocity)), 0, 0),
                    "wrong read-only access");
            });

            Test("three composed terms report writes in declaration order", () =>
            {
                var system = ProjectHost.CreateSystem(Method(nameof(TestSystems.TripleWriter)));
                Equal(system.Accesses.Length, 3, "unexpected access count");
                Equal(system.Accesses[0].ComponentKey, Engine.ComponentKey(typeof(TestPosition)),
                    "wrong first key");
                Equal(system.Accesses[1].ComponentKey, Engine.ComponentKey(typeof(TestVelocity)),
                    "wrong second key");
                Equal(system.Accesses[2].ComponentKey, Engine.ComponentKey(typeof(TestHealth)),
                    "wrong third key");
                Assert(system.Accesses.All(access => access.Mode == 1),
                    "composed write terms must report only writes");
            });

            Test("entity and optional terms produce exact scheduler access", () =>
            {
                var system = ProjectHost.CreateSystem(Method(nameof(TestSystems.OptionalAndEntity)));
                Equal(system.Accesses.Length, 2, "EntityTerm must not create scheduler access");
                Equal(system.Accesses[0],
                    new ManagedAccess(Engine.ComponentKey(typeof(TestPosition)), Engine.ComponentKeyHigh(typeof(TestPosition)), 0, 0),
                    "wrong required read access");
                Equal(system.Accesses[1],
                    new ManagedAccess(Engine.ComponentKey(typeof(TestHealth)), Engine.ComponentKeyHigh(typeof(TestHealth)), 1, 0),
                    "wrong optional write access");
            });

            Test("query arities one through eight build closed descriptors", () =>
            {
                Type[] definitions =
                [
                    typeof(Query<>),
                    typeof(Query<,>),
                    typeof(Query<,,>),
                    typeof(Query<,,,>),
                    typeof(Query<,,,,>),
                    typeof(Query<,,,,,>),
                    typeof(Query<,,,,,,>),
                    typeof(Query<,,,,,,,>),
                ];
                Type[] terms =
                [
                    typeof(Read<TestPosition>),
                    typeof(Write<TestVelocity>),
                    typeof(OptionalRead<TestHealth>),
                    typeof(Write<TestComponent4>),
                    typeof(Read<TestComponent5>),
                    typeof(Write<TestComponent6>),
                    typeof(Read<TestComponent7>),
                    typeof(OptionalWrite<TestComponent8>),
                ];
                for (int arity = 1; arity <= definitions.Length; arity++)
                {
                    Type closed = definitions[arity - 1]
                        .MakeGenericType(terms.Take(arity).ToArray());
                    var query = (IQueryDescriptor)(Activator.CreateInstance(
                        closed, nonPublic: true)
                        ?? throw new InvalidOperationException(
                            $"Could not instantiate query arity {arity}."));
                    Equal(query.Descriptor.Terms.Count, arity,
                        $"query arity {arity} descriptor term count mismatch");
                }
            });

            Test("eight-term query exports every scheduler access in order", () =>
            {
                var system = ProjectHost.CreateSystem(Method(nameof(TestSystems.EightTerms)));
                Type[] componentTypes =
                [
                    typeof(TestPosition), typeof(TestVelocity), typeof(TestHealth),
                    typeof(TestComponent4), typeof(TestComponent5),
                    typeof(TestComponent6), typeof(TestComponent7),
                    typeof(TestComponent8),
                ];
                byte[] modes = [0, 1, 0, 1, 0, 1, 0, 1];
                Equal(system.Accesses.Length, componentTypes.Length,
                    "eight-term scheduler access count mismatch");
                for (int index = 0; index < componentTypes.Length; index++)
                {
                    Equal(system.Accesses[index], new ManagedAccess(
                            Engine.ComponentKey(componentTypes[index]),
                            Engine.ComponentKeyHigh(componentTypes[index]),
                            modes[index], 0),
                        $"wrong scheduler access at term {index}");
                }
            });

            Test("query rows and enumerators are stack-only", () =>
            {
                Assert(typeof(QueryRow).IsByRefLike, "QueryRow must remain a ref struct");
                Assert(typeof(QueryEnumerator).IsByRefLike,
                    "QueryEnumerator must remain a ref struct");
                Assert(typeof(QueryRow<Read<TestPosition>, None, None, None, None, None, None, None>).IsByRefLike,
                    "typed QueryRow must remain a ref struct");
                Assert(typeof(QueryEnumerator<Read<TestPosition>, None, None, None, None, None, None, None>).IsByRefLike,
                    "typed QueryEnumerator must remain a ref struct");
                Assert(typeof(OptionalReadRef<TestHealth>).IsByRefLike,
                    "OptionalReadRef must remain a ref struct");
                Assert(typeof(OptionalWriteRef<TestHealth>).IsByRefLike,
                    "OptionalWriteRef must remain a ref struct");
            });

            Test("composed query iterates required optional and entity terms", () =>
            {
                TestPosition* positions = stackalloc TestPosition[2];
                TestVelocity* velocities = stackalloc TestVelocity[2];
                Entity* entities = stackalloc Entity[2];
                NativeComponentTicks* positionTicks = stackalloc NativeComponentTicks[2];
                NativeComponentTicks* velocityTicks = stackalloc NativeComponentTicks[2];
                positions[0].X = 1;
                positions[1].X = 2;
                velocities[0].X = 10;
                velocities[1].X = 20;
                entities[0] = new Entity(100, 3);
                entities[1] = new Entity(200, 4);
                positionTicks[0] = positionTicks[1] = new NativeComponentTicks
                { Added = 1, Changed = 2 };
                velocityTicks[0] = velocityTicks[1] = new NativeComponentTicks
                { Added = 1, Changed = 2 };

                MockNativeWorld.Positions = positions;
                MockNativeWorld.Velocities = velocities;
                MockNativeWorld.Entities = entities;
                MockNativeWorld.PositionTicks = positionTicks;
                MockNativeWorld.VelocityTicks = velocityTicks;
                MockNativeWorld.Length = 2;
                MockNativeWorld.ChangeTick = 9;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                var query = new Query<
                    Write<TestPosition>, Read<TestVelocity>,
                    OptionalWrite<TestHealth>, EntityTerm>();
                var seen = 0;
                foreach (var row in query)
                {
                    if (row.Entity.Id == 100)
                    {
                        ref TestPosition position = ref row.Write<TestPosition>();
                        ref readonly TestVelocity velocity = ref row.Read<TestVelocity>();
                        position.X += velocity.X;
                    }
                    Assert(!row.OptionalWrite<TestHealth>().HasValue,
                        "missing optional component unexpectedly has a value");
                    Equal(row.Entity.Id, entities[seen].Id, "wrong entity joined to row");
                    seen++;
                }

                Equal(seen, 2, "wrong composed query row count");
                Equal(positions[0].X, 11.0f, "first writable row was not updated");
                Equal(positions[1].X, 2.0f, "unrequested writable row was modified");
                Equal(positionTicks[0].Changed, 9u, "written row was not marked changed");
                Equal(positionTicks[1].Changed, 2u, "unrequested row was marked changed");
                Equal(velocityTicks[0].Changed, 2u, "read-only row was marked changed");
                Equal(velocityTicks[1].Changed, 2u, "read-only row was marked changed");
            });

            Test("parallel rows visit every row once and match a sequential pass", () =>
            {
                const uint rows = 1200;
                TestPosition* positions =
                    (TestPosition*)NativeMemory.Alloc(rows * (nuint)sizeof(TestPosition));
                TestVelocity* velocities =
                    (TestVelocity*)NativeMemory.Alloc(rows * (nuint)sizeof(TestVelocity));
                NativeComponentTicks* positionTicks =
                    (NativeComponentTicks*)NativeMemory.Alloc(rows * (nuint)sizeof(NativeComponentTicks));
                for (var i = 0; i < rows; i++)
                {
                    positions[i] = new TestPosition { X = i, Y = 0 };
                    velocities[i] = new TestVelocity { X = 1, Y = 0 };
                    positionTicks[i] = default;
                }
                MockNativeWorld.Positions = positions;
                MockNativeWorld.Velocities = velocities;
                MockNativeWorld.PositionTicks = positionTicks;
                MockNativeWorld.Length = rows;
                MockNativeWorld.ChangeTick = 21;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                try
                {
                    var query = new Query<Write<TestPosition>, Read<TestVelocity>>();
                    var visited = 0;
                    var dispatchesBefore = MockNativeWorld.ParallelDispatchCalls;
                    ((QueryBase)query).GetEnumerator().ForEachSliceParallel(
                        (segment, start, count) =>
                        {
                            for (var row = start; row < start + count; row++)
                            {
                                ref TestPosition position = ref segment.Get<TestPosition>(0, row);
                                ref readonly TestVelocity velocity =
                                    ref segment.Get<TestVelocity>(1, row);
                                position.X += velocity.X;
                                segment.MarkChanged(0, row);
                                Interlocked.Increment(ref visited);
                            }
                        }, 256);

                    Equal(visited, (int)rows, "every row must be visited exactly once");
                    Equal(MockNativeWorld.ParallelDispatchCalls, dispatchesBefore + 1,
                        "a multi-slice pass must dispatch exactly once");
                    for (var i = 0; i < rows; i++)
                    {
                        Equal(positions[i].X, i + 1.0f, "parallel row value");
                        Equal(positionTicks[i].Changed, 21u, "written row was not marked changed");
                    }

                    // Sequential parity: the same body over Rows() (foreach)
                    // reaches the same storage, so the passes must agree.
                    foreach (var row in query)
                    {
                        ref TestPosition position = ref row.Write<TestPosition>();
                        position.X += 1.0f;
                    }
                    for (var i = 0; i < rows; i++)
                        Equal(positions[i].X, i + 2.0f, "sequential pass parity");
                }
                finally
                {
                    MockNativeWorld.Positions = null;
                    MockNativeWorld.Velocities = null;
                    MockNativeWorld.PositionTicks = null;
                    MockNativeWorld.Length = 0;
                    NativeMemory.Free(positions);
                    NativeMemory.Free(velocities);
                    NativeMemory.Free(positionTicks);
                }
            });

            Test("parallel rows stamp required writes and skip absent optionals", () =>
            {
                const int rows = 600;
                TestPosition* positions = stackalloc TestPosition[rows];
                NativeComponentTicks* positionTicks = stackalloc NativeComponentTicks[rows];
                for (var i = 0; i < rows; i++)
                {
                    positions[i] = default;
                    positionTicks[i] = default;
                }
                MockNativeWorld.Positions = positions;
                MockNativeWorld.PositionTicks = positionTicks;
                MockNativeWorld.Healths = null;
                MockNativeWorld.Length = rows;
                MockNativeWorld.ChangeTick = 77;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                try
                {
                    var query = new Query<Write<TestPosition>, OptionalWrite<TestHealth>>();
                    var anyPresent = false;
                    var visited = 0;
                    ((QueryBase)query).GetEnumerator().ForEachSliceParallel(
                        (segment, start, count) =>
                        {
                            for (var row = start; row < start + count; row++)
                            {
                                ref TestPosition position = ref segment.Get<TestPosition>(0, row);
                                position.X += 1.0f;
                                segment.MarkChanged(0, row);
                                OptionalWriteRef<TestHealth> optional =
                                    segment.GetOptionalWrite<TestHealth>(1, row);
                                if (optional.HasValue)
                                    anyPresent = true;
                                Interlocked.Increment(ref visited);
                            }
                        }, 256);

                    Equal(visited, rows, "every row must be visited exactly once");
                    Assert(!anyPresent, "an absent optional reported a value");
                    for (var i = 0; i < rows; i++)
                        Equal(positionTicks[i].Changed, 77u, "written row was not marked changed");
                    for (var i = 0; i < rows; i++)
                        Equal(positions[i].X, 1.0f, "parallel row value");
                }
                finally
                {
                    MockNativeWorld.Positions = null;
                    MockNativeWorld.PositionTicks = null;
                    MockNativeWorld.Length = 0;
                }
            });

            Test("parallel slice failure rethrows the first exception after other items run", () =>
            {
                const uint rows = 1200;
                TestPosition* positions =
                    (TestPosition*)NativeMemory.Alloc(rows * (nuint)sizeof(TestPosition));
                NativeComponentTicks* positionTicks =
                    (NativeComponentTicks*)NativeMemory.Alloc(rows * (nuint)sizeof(NativeComponentTicks));
                for (var i = 0; i < rows; i++)
                {
                    positions[i] = default;
                    positionTicks[i] = default;
                }
                MockNativeWorld.Positions = positions;
                MockNativeWorld.PositionTicks = positionTicks;
                MockNativeWorld.Length = rows;
                MockNativeWorld.ChangeTick = 5;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                try
                {
                    var query = new Query<Write<TestPosition>>();
                    var visited = 0;
                    var thrown = false;
                    try
                    {
                        ((QueryBase)query).GetEnumerator().ForEachSliceParallel(
                            (segment, start, count) =>
                            {
                                for (var row = start; row < start + count; row++)
                                {
                                    Interlocked.Increment(ref visited);
                                    if (row == 700)
                                        throw new InvalidOperationException("parallel body exploded");
                                    ref TestPosition position = ref segment.Get<TestPosition>(0, row);
                                    position.X += 1.0f;
                                }
                            }, 256);
                    }
                    catch (InvalidOperationException error)
                    {
                        thrown = true;
                        Equal(error.Message, "parallel body exploded",
                            "the original exception must surface unchanged");
                    }
                    Assert(thrown, "the failing pass must rethrow after the join");
                    Assert(visited > 512 && visited < (int)rows,
                        "later slices still ran; the failing slice stopped at its throwing row");
                    Equal(positions[0].X, 1.0f, "the first slice ran");
                    Equal(positions[(int)rows - 1].X, 1.0f, "slices after the failure still ran");

                    // The process survives: a fresh pass over the same query
                    // covers every row again.
                    var recovery = 0;
                    ((QueryBase)query).GetEnumerator().ForEachSliceParallel(
                        (segment, start, count) =>
                        {
                            for (var row = start; row < start + count; row++)
                            {
                                _ = segment.Get<TestPosition>(0, row).X;
                                recovery++;
                            }
                        }, 256);
                    Equal(recovery, (int)rows, "a pass after a failed one still covers every row");
                }
                finally
                {
                    MockNativeWorld.Positions = null;
                    MockNativeWorld.PositionTicks = null;
                    MockNativeWorld.Length = 0;
                    NativeMemory.Free(positions);
                    NativeMemory.Free(positionTicks);
                }
            });

            Test("parallel rows refuse off-scope and nested passes, and inline single slices", () =>
            {
                TestPosition* positions = stackalloc TestPosition[2];
                NativeComponentTicks* positionTicks = stackalloc NativeComponentTicks[2];
                positions[0] = default;
                positions[1] = default;
                positionTicks[0] = default;
                positionTicks[1] = default;
                MockNativeWorld.Positions = positions;
                MockNativeWorld.PositionTicks = positionTicks;
                MockNativeWorld.Length = 2;
                MockNativeWorld.ChangeTick = 3;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                try
                {
                    var query = new Query<Write<TestPosition>>();

                    // A single work item runs inline: the host slot is never
                    // called.
                    var dispatchesBefore = MockNativeWorld.ParallelDispatchCalls;
                    var visited = 0;
                    ((QueryBase)query).GetEnumerator().ForEachSliceParallel(
                        (segment, start, count) =>
                        {
                            for (var row = start; row < start + count; row++)
                            {
                                segment.Get<TestPosition>(0, row).X += 1.0f;
                                visited++;
                            }
                        }, 1_000_000);
                    Equal(visited, 2, "the inline slice must cover every row");
                    Equal(MockNativeWorld.ParallelDispatchCalls, dispatchesBefore,
                        "a single slice must not dispatch through the host");
                    Equal(positions[0].X, 1.0f, "the inline slice wrote the row");

                    // Nesting is refused before any row of the inner pass runs.
                    var nestedMessage = "";
                    try
                    {
                        ((QueryBase)query).GetEnumerator().ForEachSliceParallel(
                            (segment, start, count) =>
                                ((QueryBase)query).GetEnumerator().ForEachSliceParallel(
                                    (_, _, _) => { }));
                    }
                    catch (InvalidOperationException error)
                    {
                        nestedMessage = error.Message;
                    }
                    Assert(nestedMessage.Contains("cannot start another parallel pass"),
                        "a nested pass must be refused with the nesting message");

                    // Off-scope entry is refused before any row runs.
                    var attemptedRows = 0;
                    MockNativeWorld.ScopeTokenValue = 0;
                    var scopeMessage = "";
                    try
                    {
                        ((QueryBase)query).GetEnumerator().ForEachSliceParallel(
                            (_, _, _) => attemptedRows++);
                    }
                    catch (InvalidOperationException error)
                    {
                        scopeMessage = error.Message;
                    }
                    MockNativeWorld.ScopeTokenValue = 1;
                    Assert(scopeMessage.Contains("requires a scheduled [System]"),
                        "an off-scope pass must be refused with the scope message");
                    Equal(attemptedRows, 0, "no row may run for a refused pass");
                }
                finally
                {
                    MockNativeWorld.Positions = null;
                    MockNativeWorld.PositionTicks = null;
                    MockNativeWorld.Length = 0;
                }
            });

            Test("generated ForEachParallel binds named refs, optionals, and entities", () =>
            {
                const int rows = 300;
                TestPosition* positions = stackalloc TestPosition[rows];
                TestHealth* healths = stackalloc TestHealth[rows];
                Entity* entities = stackalloc Entity[rows];
                NativeComponentTicks* healthTicks = stackalloc NativeComponentTicks[rows];
                for (var i = 0; i < rows; i++)
                {
                    positions[i] = new TestPosition { X = i, Y = 0 };
                    healths[i] = new TestHealth { Value = 1 };
                    entities[i] = new Entity((ulong)(100 + i), 0);
                    healthTicks[i] = default;
                }
                MockNativeWorld.Positions = positions;
                MockNativeWorld.Healths = healths;
                MockNativeWorld.Entities = entities;
                MockNativeWorld.HealthTicks = healthTicks;
                MockNativeWorld.Length = rows;
                MockNativeWorld.ChangeTick = 42;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                try
                {
                    var query = new Query<EntityTerm, Read<TestPosition>, OptionalWrite<TestHealth>>();
                    var visited = 0;
                    var idSum = 0UL;
                    var absent = false;
                    query.ForEachParallel(64,
                        (Entity entity, in TestPosition position, OptionalWriteRef<TestHealth> health) =>
                        {
                            if (!health.HasValue)
                            {
                                absent = true;
                                return;
                            }
                            idSum += entity.Id;
                            health.Value.Value += position.X;
                            visited++;
                        });
                    Equal(visited, rows, "the generated pass must visit every row once");
                    Assert(!absent, "every fixture row carries the optional component");
                    Equal(idSum, 74850UL, "the entity term must deliver each row's entity");
                    for (var i = 0; i < rows; i++)
                    {
                        Equal(healths[i].Value, 1.0f + i, "optional writes must reach the row");
                        Equal(healthTicks[i].Changed, 42u, "optional writes must stamp the tick");
                    }

                    // The parameterless overload travels the same path with the
                    // runtime's default slice size; 300 rows is one slice, so
                    // this also covers the inline pass.
                    var visitedDefault = 0;
                    query.ForEachParallel(
                        (Entity entity, in TestPosition position, OptionalWriteRef<TestHealth> health) =>
                            visitedDefault++);
                    Equal(visitedDefault, rows, "the default overload must visit every row once");
                }
                finally
                {
                    MockNativeWorld.Positions = null;
                    MockNativeWorld.Healths = null;
                    MockNativeWorld.Entities = null;
                    MockNativeWorld.HealthTicks = null;
                    MockNativeWorld.Length = 0;
                }
            });

            Test("typed rows alias native columns instead of copying them", () =>
            {
                TestPosition* positions = stackalloc TestPosition[1];
                TestVelocity* velocities = stackalloc TestVelocity[1];
                NativeComponentTicks* positionTicks = stackalloc NativeComponentTicks[1];
                positions[0].X = 5;
                velocities[0].X = 7;
                positionTicks[0] = default;
                MockNativeWorld.Positions = positions;
                MockNativeWorld.Velocities = velocities;
                MockNativeWorld.PositionTicks = positionTicks;
                MockNativeWorld.Length = 1;
                MockNativeWorld.ChangeTick = 3;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                var query = new Query<Write<TestPosition>, Read<TestVelocity>>();
                foreach (var row in query)
                {
                    ref TestPosition position = ref row.Write<TestPosition>();
                    ref readonly TestVelocity velocity = ref row.Read<TestVelocity>();
                    Assert(Unsafe.AreSame(ref position, ref positions[0]),
                        "writable accessor did not alias the native row");
                    Assert(Unsafe.AreSame(ref Unsafe.AsRef(in velocity), ref velocities[0]),
                        "read-only accessor did not alias the native row");
                    position.X += velocity.X;
                }

                Equal(positions[0].X, 12.0f, "aliased write did not reach native storage");
                MockNativeWorld.Positions = null;
                MockNativeWorld.Velocities = null;
                MockNativeWorld.PositionTicks = null;
            });

            Test("typed rows reject access the query did not declare", () =>
            {
                TestPosition* positions = stackalloc TestPosition[1];
                TestHealth* health = stackalloc TestHealth[1];
                NativeComponentTicks* positionTicks = stackalloc NativeComponentTicks[1];
                positions[0].X = 1;
                health[0].Value = 2;
                positionTicks[0] = default;
                MockNativeWorld.Positions = positions;
                MockNativeWorld.Healths = health;
                MockNativeWorld.PositionTicks = positionTicks;
                MockNativeWorld.Length = 1;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                var query = new Query<Write<TestPosition>, OptionalRead<TestHealth>>();
                foreach (var row in query)
                {
                    // Positive control: the declared access resolves and writes.
                    ref TestPosition position = ref row.Write<TestPosition>();
                    position.X += 1;

                    bool rejected = false;
                    try { row.Read<TestPosition>(); }
                    catch (InvalidOperationException) { rejected = true; }
                    Assert(rejected, "read access to a write-only term must be rejected");

                    rejected = false;
                    try { row.OptionalRead<TestPosition>(); }
                    catch (InvalidOperationException) { rejected = true; }
                    Assert(rejected, "optional read access to a required term must be rejected");

                    rejected = false;
                    try { row.Read<TestHealth>(); }
                    catch (InvalidOperationException) { rejected = true; }
                    Assert(rejected, "required read access to an optional term must be rejected");

                    rejected = false;
                    try { row.OptionalWrite<TestHealth>(); }
                    catch (InvalidOperationException) { rejected = true; }
                    Assert(rejected, "optional write access to an optional read term must be rejected");

                    rejected = false;
                    try { row.Read<TestComponent4>(); }
                    catch (InvalidOperationException) { rejected = true; }
                    Assert(rejected, "access to an undeclared component must be rejected");

                    rejected = false;
                    try { _ = row.Entity; }
                    catch (InvalidOperationException) { rejected = true; }
                    Assert(rejected, "EntityTerm access without the term must be rejected");
                }

                Equal(positions[0].X, 2.0f, "positive control did not write");
                MockNativeWorld.Positions = null;
                MockNativeWorld.Healths = null;
                MockNativeWorld.PositionTicks = null;
            });

            Test("optional writes mark only rows whose value is requested", () =>
            {
                TestHealth* health = stackalloc TestHealth[2];
                Entity* entities = stackalloc Entity[2];
                NativeComponentTicks* ticks = stackalloc NativeComponentTicks[2];
                health[0].Value = 10;
                health[1].Value = 20;
                entities[0] = new Entity(100, 3);
                entities[1] = new Entity(200, 4);
                ticks[0] = ticks[1] = new NativeComponentTicks { Added = 1, Changed = 2 };

                MockNativeWorld.Healths = health;
                MockNativeWorld.HealthTicks = ticks;
                MockNativeWorld.Entities = entities;
                MockNativeWorld.Length = 2;
                MockNativeWorld.ChangeTick = 12;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                var query = new Query<OptionalWrite<TestHealth>, EntityTerm>();
                foreach (var row in query)
                {
                    var optional = row.OptionalWrite<TestHealth>();
                    Assert(optional.HasValue, "present optional component was not found");
                    if (row.Entity.Id == 100)
                        optional.Value.Value += 5;
                }

                Equal(health[0].Value, 15.0f, "optional writable value was not updated");
                Equal(health[1].Value, 20.0f, "unrequested optional value was modified");
                Equal(ticks[0].Changed, 12u, "optional written row was not marked changed");
                Equal(ticks[1].Changed, 2u, "HasValue incorrectly marked the optional row");
                MockNativeWorld.Healths = null;
                MockNativeWorld.HealthTicks = null;
            });

            Test("optional-read-only query yields present and missing components", () =>
            {
                TestHealth* health = stackalloc TestHealth[1];
                Entity* entities = stackalloc Entity[1];
                health[0].Value = 42;
                entities[0] = new Entity(300, 5);
                MockNativeWorld.Healths = health;
                MockNativeWorld.Entities = entities;
                MockNativeWorld.Length = 1;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                var presentRows = 0;
                foreach (var row in new Query<OptionalRead<TestHealth>>())
                {
                    var optional = row.OptionalRead<TestHealth>();
                    Assert(optional.HasValue, "present optional read was not found");
                    Equal(optional.Value.Value, 42.0f, "optional read returned the wrong value");
                    presentRows++;
                }
                Equal(presentRows, 1, "optional-only query did not use the entity driver");

                MockNativeWorld.Healths = null;
                var missingRows = 0;
                foreach (var row in new Query<OptionalRead<TestHealth>>())
                {
                    Assert(!row.OptionalRead<TestHealth>().HasValue,
                        "missing optional read unexpectedly had a value");
                    missingRows++;
                }
                Equal(missingRows, 1,
                    "optional-only query skipped entities without the component");
            });

            Test("entity-only query iterates native entity chunks", () =>
            {
                Entity* entities = stackalloc Entity[2];
                entities[0] = new Entity(400, 6);
                entities[1] = new Entity(500, 7);
                MockNativeWorld.Entities = entities;
                MockNativeWorld.Length = 2;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                ulong idSum = 0;
                var rows = 0;
                foreach (var row in new Query<EntityTerm>())
                {
                    idSum += row.Entity.Id;
                    rows++;
                }
                Equal(rows, 2, "entity-only query row count mismatch");
                Equal(idSum, 900UL, "entity-only query returned the wrong entities");
            });

            Test("warmed typed row access performs no managed allocations", () =>
            {
                TestVelocity* velocities = stackalloc TestVelocity[1];
                Entity* entities = stackalloc Entity[1];
                velocities[0].X = 3;
                entities[0] = new Entity(600, 8);
                MockNativeWorld.Velocities = velocities;
                MockNativeWorld.Entities = entities;
                MockNativeWorld.Length = 1;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                long allocated = -1;
                float sum = 0;
                foreach (var row in new Query<Read<TestVelocity>>())
                {
                    _ = row.Read<TestVelocity>().X; // Initialize generic metadata and JIT paths.
                    long before = GC.GetAllocatedBytesForCurrentThread();
                    for (int iteration = 0; iteration < 1_000; iteration++)
                        sum += row.Read<TestVelocity>().X;
                    allocated = GC.GetAllocatedBytesForCurrentThread() - before;
                }
                Equal(allocated, 0L, "typed row access allocated after warm-up");
                Equal(sum, 3_000.0f, "typed row access loop was not executed");
            });

            Test("enumerating a query allocates nothing", () =>
            {
                // A system enumerates its queries every frame, so starting an
                // enumeration - joining the columns of each chunk - must stay
                // off the managed heap, not only the per-row accessors.
                TestVelocity* velocities = stackalloc TestVelocity[1];
                Entity* entities = stackalloc Entity[1];
                velocities[0].X = 3;
                entities[0] = new Entity(601, 8);
                MockNativeWorld.Velocities = velocities;
                MockNativeWorld.Entities = entities;
                MockNativeWorld.Length = 1;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                var query = new Query<Read<TestVelocity>>();
                static float Walk(Query<Read<TestVelocity>> query)
                {
                    float total = 0;
                    foreach (var row in query)
                        total += row.Read<TestVelocity>().X;
                    return total;
                }
                Walk(query); // Initialize generic metadata and JIT paths.
                long before = GC.GetAllocatedBytesForCurrentThread();
                float sum = 0;
                for (int frame = 0; frame < 1_000; frame++)
                    sum += Walk(query);
                long allocated = GC.GetAllocatedBytesForCurrentThread() - before;
                Equal(allocated, 0L, "enumerating a query allocated");
                Equal(sum, 3_000.0f, "the query loop was not executed");
            });

            Test("compiled runner supplies its query parameter", () =>
            {
                TestSystems.WasRun = false;
                ProjectHost.CreateSystem(Method(nameof(TestSystems.Runner))).Run();
                Assert(TestSystems.WasRun, "compiled runner did not invoke the method");
            });

            Test("compiled runner supplies every query parameter", () =>
            {
                TestPosition* positions = stackalloc TestPosition[1];
                TestVelocity* velocities = stackalloc TestVelocity[1];
                MockNativeWorld.Positions = positions;
                MockNativeWorld.Velocities = velocities;
                MockNativeWorld.Length = 1;
                TestSystems.QueriesVisited = 0;
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                ProjectHost.CreateSystem(Method(nameof(TestSystems.TwoQueries))).Run();
                Equal(TestSystems.QueriesVisited, 2,
                    "the runner did not iterate both query parameters");
            });

            Test("component keys are stable and type-specific", () =>
            {
                Equal(Engine.ComponentKey(typeof(TransformComponent)),
                    Engine.ComponentKey(typeof(TransformComponent)), "TransformComponent key is unstable");
                Assert(Engine.ComponentKey(typeof(PhysicsState)) !=
                       Engine.ComponentKey(typeof(TransformComponent)),
                    "different current project components produced the same key");
                Assert(Engine.ComponentKey(typeof(TransformComponent)) !=
                       Engine.ComponentKey(typeof(MeshRendererComponent)),
                    "different current project components produced the same key");
            });

            Test("duplicate write/read component is rejected", () =>
                Throws<InvalidOperationException>(
                    () => ProjectHost.CreateSystem(Method(nameof(TestSystems.DuplicateReadWrite))),
                    "duplicate write/read component should be rejected"));

            Test("duplicate component in a three-term query is rejected", () =>
                Throws<InvalidOperationException>(
                    () => ProjectHost.CreateSystem(Method(nameof(TestSystems.DuplicateTriple))),
                    "duplicate component in a three-term query should be rejected"));

            Test("duplicate entity term is rejected", () =>
                Throws<InvalidOperationException>(
                    () => ProjectHost.CreateSystem(Method(nameof(TestSystems.DuplicateEntity))),
                    "duplicate EntityTerm should be rejected"));

            Test("zero-parameter system is rejected", () =>
                Throws<InvalidOperationException>(
                    () => ProjectHost.CreateSystem(Method(nameof(TestSystems.NoParameters))),
                    "zero-parameter system should be rejected"));

            Test("non-void system is rejected", () =>
                Throws<InvalidOperationException>(
                    () => ProjectHost.CreateSystem(Method(nameof(TestSystems.NonVoid))),
                    "non-void system should be rejected"));

            Test("unsupported parameter is rejected", () =>
                Throws<InvalidOperationException>(
                    () => ProjectHost.CreateSystem(Method(nameof(TestSystems.Unsupported))),
                    "unsupported parameter should be rejected"));

            Test("invalid managed component layout is rejected before registration", () =>
            {
                var system = ProjectHost.CreateSystem(Method(nameof(TestSystems.InvalidLayout)));
                Throws<InvalidOperationException>(
                    () => ProjectManifestBuilder.Build([system]),
                    "bool fields must be rejected from native component manifests");
            });

            Test("loader interop runs a discovered system and clears its error slot", () =>
            {
                var projectAssembly = typeof(BallPhysicsSystem).Assembly;
                Environment.SetEnvironmentVariable(
                    "ECS_CSHARP_PROJECT_DIR", Path.GetDirectoryName(projectAssembly.Location));
                Environment.SetEnvironmentVariable(
                    "ECS_CSHARP_PROJECT_ASSEMBLY", Path.GetFileName(projectAssembly.Location));
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                // The Rust host consumes these exports through function
                // pointers, so the regression tests do exactly the same.
                var init = (delegate* unmanaged<nint, byte>)&LoaderInterop.Init;
                var systemCount = (delegate* unmanaged<uint>)&LoaderInterop.SystemCount;
                var runSystem = (delegate* unmanaged<uint, byte>)&LoaderInterop.RunSystem;
                var errorLength =
                    (delegate* unmanaged<uint, uint>)&LoaderInterop.SystemErrorMessageLength;

                Equal(init((IntPtr)(&api)), 1, "loader init failed");
                // The project's six systems and its four bench systems.
                Equal(systemCount(), 10u, "unexpected managed system count");
                Equal(runSystem(1), 1, "healthy system reported failure");
                Equal(errorLength(1), 0u,
                    "healthy system carries a stale error message");
            });

            Test("loader interop reports failure for an invalid system index", () =>
            {
                var runSystem = (delegate* unmanaged<uint, byte>)&LoaderInterop.RunSystem;
                Equal(runSystem(99), 0,
                    "invalid managed system index reported success");
            });

            Test("Resolve rebinds after the host republishes the table", () =>
            {
                MockNativeWorld.MirroredRows = [];
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                // Step 1: the host publishes one trampoline and resolves it.
                IntPtr first = (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr*, IntPtr*, byte>)
                    &MockNativeWorld.AccessorView;
                MockNativeWorld.PublishMirrorRow("TracyLive.Probe", "entries_view", first);
                MockNativeWorld.ViewDelegate resolved =
                    MirrorMethods.Resolve<MockNativeWorld.ViewDelegate>(
                        "TracyLive.Probe", "entries_view");
                resolved((IntPtr)32, out IntPtr firstData, out IntPtr firstLength);
                Equal(firstData, (IntPtr)64, "the first trampoline's data");
                Equal(firstLength, (IntPtr)5, "the first trampoline's length");

                // Step 2: the host republishes with a fresh address and bumps
                // its epoch. The cached delegate belongs to the previous bind,
                // so the next resolve has to reach the new trampoline without
                // waiting for an assembly swap.
                IntPtr second = (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr*, IntPtr*, byte>)
                    &MockNativeWorld.AccessorViewSecond;
                MockNativeWorld.PublishMirrorRow("TracyLive.Probe", "entries_view", second);
                resolved = MirrorMethods.Resolve<MockNativeWorld.ViewDelegate>(
                    "TracyLive.Probe", "entries_view");
                resolved((IntPtr)32, out IntPtr secondData, out IntPtr secondLength);
                Equal(secondData, (IntPtr)48, "the republished trampoline's data");
                Equal(secondLength, (IntPtr)9, "the republished trampoline's length");
            });

            Test("concurrent Resolve keeps the cache consistent", () =>
            {
                // A bound host with a stable epoch: the staleness check must
                // not wipe the table while the threads churn it.
                MockNativeWorld.MirroredRows = [];
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                IntPtr address = (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr*, IntPtr*, byte>)
                    &MockNativeWorld.AccessorView;
                MirrorMethods.Reset();
                MirrorMethods.Register("TracyLive.Probe", "entries_view", address);

                Exception? failure = null;
                Thread[] threads = new Thread[8];
                for (int index = 0; index < threads.Length; index++)
                {
                    threads[index] = new Thread(() =>
                    {
                        try
                        {
                            for (int iteration = 0; iteration < 2_000; iteration++)
                            {
                                MockNativeWorld.ViewDelegate resolved =
                                    MirrorMethods.Resolve<MockNativeWorld.ViewDelegate>(
                                        "TracyLive.Probe", "entries_view");
                                if (resolved is null)
                                    throw new InvalidOperationException("resolve returned null");
                                // Re-register periodically so the address table
                                // churns while other threads read and insert.
                                if ((iteration & 0xFF) == 0)
                                    MirrorMethods.Register("TracyLive.Probe", "entries_view", address);
                            }
                        }
                        catch (Exception error)
                        {
                            failure = error;
                        }
                    });
                    threads[index].Start();
                }
                foreach (Thread thread in threads)
                    thread.Join();
                if (failure is not null)
                    throw new InvalidOperationException($"concurrent resolve failed: {failure}");
            });

            Test("a mirror refresh never clears the table under a live call", () =>
            {
                // The GC unload of a retiring context reaches the refresh path
                // on a runtime-chosen thread. A refresh must not leave a
                // registered method unresolvable while it runs, or a correct
                // system reports "no method registered" for a method that is
                // there.
                MockNativeWorld.MirroredRows = [];
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                IntPtr address = (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr*, IntPtr*, byte>)
                    &MockNativeWorld.AccessorView;
                MockNativeWorld.PublishMirrorRow("TracyLive.Probe", "entries_view", address);

                int spurious = 0;
                Exception? failure = null;
                bool stop = false;
                Thread reader = new Thread(() =>
                {
                    while (!Volatile.Read(ref stop))
                    {
                        try
                        {
                            MirrorMethods.Address("TracyLive.Probe", "entries_view");
                        }
                        catch (InvalidOperationException)
                        {
                            Interlocked.Increment(ref spurious);
                        }
                        catch (Exception error)
                        {
                            failure = error;
                        }
                    }
                });
                reader.Start();
                for (int iteration = 0; iteration < 500; iteration++)
                {
                    MockNativeWorld.PublishMirrorRow("TracyLive.Probe", "entries_view", address);
                    Engine.RefreshMirrorMethodsIfStale();
                }
                Volatile.Write(ref stop, true);
                reader.Join();

                if (failure is not null)
                    throw new InvalidOperationException($"the reader failed: {failure}");
                Equal(spurious, 0,
                    "a refresh must not make a registered method unresolvable");
            });

            Test("an unloaded context drops its delegates and keeps the table", () =>
            {
                // What a collectible context's Unloading handler does: the
                // delegate types it created go, the addresses the host
                // published stay, and the generation a live accessor resolved
                // against does not move under it.
                MockNativeWorld.MirroredRows = [];
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                IntPtr address = (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr*, IntPtr*, byte>)
                    &MockNativeWorld.AccessorView;
                MockNativeWorld.PublishMirrorRow("TracyLive.Probe", "entries_view", address);
                MirrorMethods.Resolve<MockNativeWorld.ViewDelegate>(
                    "TracyLive.Probe", "entries_view");
                int generation = MirrorMethods.Generation;

                MirrorMethods.DropDelegateCache();

                Equal(MirrorMethods.Generation, generation,
                    "an unload must not bump the generation a live call resolved against");
                Equal(MirrorMethods.Address("TracyLive.Probe", "entries_view").Address, address,
                    "an unload must not clear the host's address table");
                MockNativeWorld.ViewDelegate after =
                    MirrorMethods.Resolve<MockNativeWorld.ViewDelegate>(
                        "TracyLive.Probe", "entries_view");
                after((IntPtr)32, out IntPtr data, out IntPtr length);
                Equal(data, (IntPtr)64, "the table still resolves the published trampoline");
                Equal(length, (IntPtr)5, "the table still resolves the published trampoline");
            });

            Test("accessor addresses resolve and re-resolve across a host rebind", () =>
            {
                // Step 1: a fresh bind resolves the address a generated
                // accessor would cache for its trampoline.
                MirrorMethods.Reset();
                IntPtr first = (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr*, IntPtr*, byte>)
                    &MockNativeWorld.AccessorView;
                MirrorMethods.Register("TracyLive.Probe", "entries_view", first);
                int generation = MirrorMethods.Generation;
                Equal(MirrorMethods.Address("TracyLive.Probe", "entries_view").Address, first,
                    "a registered address did not resolve");

                // Step 2: an op the module never published is refused with the
                // registration message instead of handing out a null pointer.
                Throws<InvalidOperationException>(
                    () => MirrorMethods.Address("TracyLive.Probe", "missing_view"),
                    "an unregistered accessor must be refused");

                // Step 3: rebinding bumps the generation, which is what makes
                // generated caches re-resolve; the new bind's address wins.
                MirrorMethods.Reset();
                IntPtr second = (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr*, IntPtr*, byte>)
                    &MockNativeWorld.AccessorViewSecond;
                MirrorMethods.Register("TracyLive.Probe", "entries_view", second);
                Equal(MirrorMethods.Generation, generation + 1,
                    "a rebind must bump the generation");
                Equal(MirrorMethods.Address("TracyLive.Probe", "entries_view").Address, second,
                    "the rebind's address did not replace the previous one");
                Assert(second != first, "the two test trampolines must differ");
            });

            Test("raw accessor invokes call the registered trampolines", () =>
            {
                // The five helpers cover every heap-field trampoline shape;
                // the argument order of each is part of the ABI, so the
                // trampolines echo the arguments back.
                MirrorMethods.Reset();
                MirrorMethods.Register("TracyLive.Probe", "entries_view",
                    (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr*, IntPtr*, byte>)
                        &MockNativeWorld.AccessorView);
                MirrorMethods.Register("TracyLive.Probe", "entries_resize",
                    (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr, byte>)
                        &MockNativeWorld.AccessorResize);
                MirrorMethods.Register("TracyLive.Probe", "entries_item",
                    (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr, IntPtr*, IntPtr*, byte>)
                        &MockNativeWorld.AccessorItem);
                MirrorMethods.Register("TracyLive.Probe", "entries_set_item",
                    (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr, IntPtr, IntPtr, byte>)
                        &MockNativeWorld.AccessorSetItem);
                MirrorMethods.Register("TracyLive.Probe", "entries_push",
                    (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr, IntPtr, byte>)
                        &MockNativeWorld.AccessorUtf8Write);

                RowPointer row = new RowPointer((IntPtr)32);
                byte[] payload = new byte[] { 1, 2, 3 };

                byte status = MirrorMethods.InvokeView(
                    MirrorMethods.Address("TracyLive.Probe", "entries_view"),
                    row, out BufferView view);
                Equal(status, (byte)0, "view status");
                Equal(view.Data, (IntPtr)64, "view data");
                Equal(view.Length, 5, "view length");

                byte resized = MirrorMethods.InvokeResize(
                    MirrorMethods.Address("TracyLive.Probe", "entries_resize"), row, 7);
                Equal(resized, (byte)0, "resize status with the matching count");
                byte refused = MirrorMethods.InvokeResize(
                    MirrorMethods.Address("TracyLive.Probe", "entries_resize"), row, 8);
                Equal(refused, (byte)1, "resize status with a mismatched count");

                byte item = MirrorMethods.InvokeItem(
                    MirrorMethods.Address("TracyLive.Probe", "entries_item"),
                    row, 4, out BufferView itemView);
                Equal(item, (byte)2, "item status");
                Equal(itemView.Data, (IntPtr)40, "item data");
                Equal(itemView.Length, 5, "item length");

                byte setItem = MirrorMethods.InvokeSetItem(
                    MirrorMethods.Address("TracyLive.Probe", "entries_set_item"),
                    row, 4, payload);
                Equal(setItem, (byte)0, "set_item status");

                byte written = MirrorMethods.InvokeUtf8Write(
                    MirrorMethods.Address("TracyLive.Probe", "entries_push"), row, payload);
                Equal(written, (byte)0, "utf8 write status");
            });

            Test("a mirrored call packs every slot the Rust side reads", () =>
            {
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                MirrorMethods.Reset();
                MirrorMethods.Register("Test.Probe", "probe",
                    (IntPtr)(delegate* unmanaged[Cdecl]<byte*, byte*, byte>)&MockNativeWorld.MirrorProbe);

                var call = MirrorCall.Begin("Test.Probe", "probe");
                call.Push(7u);
                call.PushString("pill");
                call.PushSpan<float>([1.5f, 2.5f]);
                call.PushField(0, 3u);
                call.PushField(4, 9u);
                call.EndSlot();
                call.PushOptional<uint>(42u);
                call.PushOptional<uint>(null);
                call.PushResource<AssetManager>(QueryAccess.Write);
                call.PushLoader(AssetLoader.FromBytes([5, 6, 7]));
                call.Invoke();
                Equal(call.Result<uint>(), 11u, "the result travels back");

                byte[] slots = MockNativeWorld.ProbedSlots;
                Equal(BitConverter.ToUInt32(slots, 0), 7u, "a primitive at offset 0");
                Equal(BitConverter.ToUInt32(slots, 16 + 8), 4u, "a string's byte length at 8");
                Equal(BitConverter.ToUInt32(slots, 32 + 8), 2u, "a span's element count at 8");
                Equal(BitConverter.ToUInt32(slots, 48), 3u, "a tuple's first field");
                Equal(BitConverter.ToUInt32(slots, 48 + 4), 9u, "a tuple's second field at its offset");
                Equal(BitConverter.ToUInt32(slots, 64), 42u, "a present option's value");
                Equal(slots[64 + 15], (byte)1, "a present option's flag at 15");
                Equal(slots[80 + 15], (byte)0, "an absent option's flag");
                Equal((IntPtr)BitConverter.ToInt64(slots, 96), MockNativeWorld.NativeResourceAddress,
                    "a resource slot carries the address the host handed out");
                Equal(MockNativeWorld.LastNativeResourceMode, (byte)1, "a write was requested");
                Equal(BitConverter.ToUInt32(slots, 112 + 8), 3u, "a bytes loader's length");
                Equal(slots[112 + 12], (byte)1, "a bytes loader's kind at 12");
            });

            Test("a mirrored call of primitives allocates nothing", () =>
            {
                // Generated value-type methods run per row, per frame (the
                // spline sampling in project_cs), so the call path must not
                // touch the managed heap once the thread's frame exists.
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                MirrorMethods.Reset();
                MirrorMethods.Register("Test.Probe", "increment",
                    (IntPtr)(delegate* unmanaged[Cdecl]<byte*, byte*, byte>)&MockNativeWorld.MirrorIncrement);
                static uint CallOnce(uint value)
                {
                    var call = MirrorCall.Begin("Test.Probe", "increment");
                    call.Push(value);
                    call.Push(0.5f);
                    call.Invoke();
                    return call.Result<uint>();
                }
                CallOnce(1);
                long before = GC.GetAllocatedBytesForCurrentThread();
                uint total = 0;
                for (uint index = 0; index < 1000; index++)
                    total += CallOnce(index);
                long allocated = GC.GetAllocatedBytesForCurrentThread() - before;
                Assert(total > 0, "the calls ran");
                Equal(allocated, 0L, "bytes allocated by 1000 mirrored calls");
            });

            Test("a failed mirrored call throws with the Rust error's message", () =>
            {
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                MirrorMethods.Reset();
                MirrorMethods.Register("Test.Probe", "fails",
                    (IntPtr)(delegate* unmanaged[Cdecl]<byte*, byte*, byte>)&MockNativeWorld.MirrorFails);
                EngineException? caught = null;
                try
                {
                    var call = MirrorCall.Begin("Test.Probe", "fails");
                    call.Invoke();
                }
                catch (EngineException exception)
                {
                    caught = exception;
                }
                Assert(caught is not null, "the failure must surface as an EngineException");
                Assert(caught!.Message.Contains("the OBJ has no triangles"), caught.Message);
                Assert(caught.Message.Contains("Test.Probe::fails"), caught.Message);
            });

            Test("an undeclared native resource access is refused by name", () =>
            {
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                MirrorMethods.Reset();
                MirrorMethods.Register("Test.Probe", "probe",
                    (IntPtr)(delegate* unmanaged[Cdecl]<byte*, byte*, byte>)&MockNativeWorld.MirrorProbe);
                MockNativeWorld.NativeResourceStatus = 2;
                try
                {
                    Throws<InvalidOperationException>(() =>
                    {
                        var call = MirrorCall.Begin("Test.Probe", "probe");
                        call.PushResource<AssetManager>(QueryAccess.Write);
                    }, "an undeclared access must be refused");
                }
                finally
                {
                    MockNativeWorld.NativeResourceStatus = 0;
                }
            });

            Test("every query status produces its own message", () =>
            {
                Refused(() => Engine.ValidateStatus((byte)QueryStatus.NotRegistered, "TestPosition", QueryAccess.Read),
                    "is not registered by the Rust host", "an unknown component must be named");
                Refused(() => Engine.ValidateStatus((byte)QueryStatus.NoInvocation, "TestPosition", QueryAccess.Read),
                    "outside its scheduled system call", "an out-of-scope query must be named");
                Refused(() => Engine.ValidateStatus((byte)QueryStatus.AccessNotDeclared, "TestPosition", QueryAccess.Write),
                    "did not declare this write access", "an undeclared access must be named");
                Refused(() => Engine.ValidateStatus((byte)QueryStatus.NullOutput, "TestPosition", QueryAccess.Read),
                    "was given no result buffer", "a null output must be named");
            });

            Test("every command status produces its own message", () =>
            {
                Engine.ValidateCommandStatus((byte)CommandStatus.Ok, "probe");
                Refused(() => Engine.ValidateCommandStatus((byte)CommandStatus.NotRegistered, "probe"),
                    "the component is not registered", "an unknown component must be named");
                Refused(() => Engine.ValidateCommandStatus((byte)CommandStatus.NoInvocation, "probe"),
                    "outside an active startup", "an out-of-scope command must be named");
                Refused(() => Engine.ValidateCommandStatus((byte)CommandStatus.CommandsNotDeclared, "probe"),
                    "did not declare a Commands parameter", "an undeclared Commands must be named");
                Refused(() => Engine.ValidateCommandStatus((byte)CommandStatus.StaleEntity, "probe"),
                    "stale", "a stale handle must be named");
                Refused(() => Engine.ValidateCommandStatus((byte)CommandStatus.InvalidComponentList, "probe"),
                    "ABI layout is invalid", "an invalid component list must be named");
                Refused(() => Engine.ValidateCommandStatus(9, "probe"),
                    "native status 9", "an unknown status must keep its number");
            });

            Test("every resource view status produces its own message", () =>
            {
                Assert(ResourceAccess.Failure<SimulationTime>((byte)ResourceViewStatus.NotRegistered, QueryAccess.Read)
                        .Message.Contains("not registered with the engine"),
                    "an unknown resource must be named");
                Assert(ResourceAccess.Failure<SimulationTime>((byte)ResourceViewStatus.AccessNotDeclared, QueryAccess.Write)
                        .Message.Contains("did not declare Write access"),
                    "an undeclared access must be named");
                Assert(ResourceAccess.Failure<SimulationTime>((byte)ResourceViewStatus.NoInvocation, QueryAccess.Read)
                        .Message.Contains("No ECS system is running"),
                    "an out-of-scope view must be named");
                Assert(ResourceAccess.Failure<SimulationTime>((byte)ResourceViewStatus.NoValue, QueryAccess.Read)
                        .Message.Contains("holds no value"),
                    "a missing value must be named");
                Assert(ResourceAccess.Failure<SimulationTime>(9, QueryAccess.Read)
                        .Message.Contains("could not be reached (status 9)"),
                    "an unknown status must keep its number");
            });

            Test("every mirrored resource status produces its own message", () =>
            {
                Assert(MirrorCall.NativeResourceFailure("SimulationTime", nameof(SimulationTime), QueryAccess.Read,
                            (byte)NativeResourceStatus.NoValue).Message.Contains("holds no SimulationTime resource"),
                    "a missing value must be named");
                Assert(MirrorCall.NativeResourceFailure("SimulationTime", nameof(SimulationTime), QueryAccess.Write,
                            (byte)NativeResourceStatus.AccessNotDeclared).Message.Contains("did not declare Write access"),
                    "an undeclared access must be named");
                Assert(MirrorCall.NativeResourceFailure("SimulationTime", nameof(SimulationTime), QueryAccess.Read,
                            (byte)NativeResourceStatus.NoInvocation).Message.Contains("inside an [System] or [Startup] call"),
                    "an out-of-scope resource must be named");
                Assert(MirrorCall.NativeResourceFailure("SimulationTime", nameof(SimulationTime), QueryAccess.Read, 9)
                        .Message.Contains("could not be reached (status 9)"),
                    "an unknown status must keep its number");
            });

            Test("the AOT registry refuses a startup the reflection path refuses", () =>
            {
                const string source = """
                    using TracyLive;
                    namespace Probe;
                    internal static class ProbeStartups
                    {
                        [Startup]
                        public static void Supported(Commands commands, Res<SimulationTime> read, ResMut<SimulationTime> write) { }

                        [Startup]
                        public static void Unsupported(string value) { }
                    }
                    """;
                var references = AppDomain.CurrentDomain.GetAssemblies()
                    .Where(assembly => !assembly.IsDynamic && !string.IsNullOrEmpty(assembly.Location))
                    .Select(assembly => MetadataReference.CreateFromFile(assembly.Location))
                    .ToArray();
                CSharpCompilation compilation = CSharpCompilation.Create(
                    "ProbeProject",
                    [CSharpSyntaxTree.ParseText(source)],
                    references,
                    new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, allowUnsafe: true));
                GeneratorDriver driver = CSharpGeneratorDriver.Create(
                    new PillCSharpRuntimeMacros.EcsAotRegistryGenerator().AsSourceGenerator());
                driver = driver.RunGenerators(compilation);
                GeneratorRunResult result = driver.GetRunResult().Results.Single();
                Diagnostic[] refusals = result.Diagnostics
                    .Where(diagnostic => diagnostic.Id == "PCS0002")
                    .ToArray();
                Equal(refusals.Length, 1, "exactly the unsupported startup must be refused");
                Assert(refusals[0].GetMessage().Contains("value"), refusals[0].GetMessage());
                string generated = result.GeneratedSources
                    .Single(source => source.HintName == "GeneratedAotRegistry.g.cs")
                    .SourceText.ToString();
                Assert(generated.Contains("Probe.ProbeStartups.Supported"),
                    "the supported startup must be registered");
                Assert(!generated.Contains("Probe.ProbeStartups.Unsupported"),
                    "the refused startup must not be registered");
            });

            // One declaration that sets every [System] property, compiled from
            // source so the reflection path and the AOT generator read the
            // same source. It sits beside `using System;` on purpose: the
            // attribute lookup must keep finding TracyLive.SystemAttribute.
            const string probeDeclarations = """
                using System;
                using TracyLive;

                namespace Probe;

                internal static class Declarations
                {
                    [System(Name = "spin_cube", Order = 500, RunPhase = RunPhase.PostProject, RunInEditMode = true,
                        RunIf = new[] { nameof(Rules.HasPlayer) }, RunIfType = typeof(Rules))]
                    public static void Scheduled(Commands commands) { }

                    [System]
                    public static void Plain(Commands commands) { }

                    internal static class Rules
                    {
                        public static bool HasPlayer() => true;
                    }
                }
                """;

            Test("a System declaration reports every schedule value through reflection", () =>
            {
                Assembly assembly = CompileProbe(
                    probeDeclarations, "ProbeDeclarationsReflection", withGeneratedRegistry: false);
                ManagedSystem[] systems = ProjectHost.DiscoverSystems(assembly);
                Equal(systems.Length, 2, "both declared systems must be discovered");
                ManagedSystem scheduled = systems.Single(system => system.Name == "spin_cube");
                Equal(scheduled.Order, 500, "the declared order must reach ManagedSystem");
                Assert(scheduled.Phase == RunPhase.PostProject,
                    "the declared phase must reach ManagedSystem");
                Assert(scheduled.RunInEditMode,
                    "the declared edit-mode flag must reach ManagedSystem");
                Equal(scheduled.Conditions.Length, 1, "the declared condition must reach ManagedSystem");
                Equal(scheduled.Conditions[0].Name, "HasPlayer", "the condition keeps its name");
                Assert(scheduled.Conditions[0].DeclaringType.FullName == "Probe.Declarations+Rules",
                    "RunIfType names the condition's declaring type");
                ManagedSystem plain = systems.Single(system => system.Name == "Probe.Declarations.Plain");
                Assert(plain.Order == 0 && plain.Phase == RunPhase.Project && !plain.RunInEditMode &&
                    plain.Conditions.Length == 0,
                    "an undeclared schedule keeps the attribute's defaults");
            });

            Test("reflection and the AOT generator agree on every declared value", () =>
            {
                Assembly reflectionProbe = CompileProbe(
                    probeDeclarations, "ProbeDeclarationsReflection", withGeneratedRegistry: false);
                Assembly generatedProbe = CompileProbe(
                    probeDeclarations, "ProbeDeclarationsGenerated", withGeneratedRegistry: true);
                ManagedSystem[] reflected = ProjectHost.DiscoverSystems(reflectionProbe);
                Type registry = generatedProbe.GetType("TracyLive.Loader.GeneratedAotRegistry")
                    ?? throw new InvalidOperationException("the generator must emit GeneratedAotRegistry");
                FieldInfo systemsField = registry.GetField("Systems", BindingFlags.NonPublic | BindingFlags.Static)
                    ?? throw new InvalidOperationException("GeneratedAotRegistry must hold Systems");
                var generated = (IReadOnlyList<AotSystemRegistration>)systemsField.GetValue(null)!;
                Equal(generated.Count, reflected.Length, "both postures discover the same systems");
                for (int index = 0; index < reflected.Length; index++)
                {
                    ManagedSystem reflection = reflected[index];
                    AotSystemRegistration registration = generated[index];
                    Equal(registration.Name, reflection.Name, $"system {index} name");
                    Equal(registration.Order, reflection.Order, $"system {index} order");
                    Assert(registration.RunPhase == reflection.Phase, $"system {index} phase");
                    Assert(registration.RunInEditMode == reflection.RunInEditMode,
                        $"system {index} edit-mode flag");
                    Equal(registration.Conditions.Length, reflection.Conditions.Length,
                        $"system {index} condition count");
                    for (int condition = 0; condition < reflection.Conditions.Length; condition++)
                    {
                        Equal(registration.Conditions[condition].Name, reflection.Conditions[condition].Name,
                            $"system {index} condition {condition} name");
                        Assert(registration.Conditions[condition].DeclaringType.FullName ==
                            reflection.Conditions[condition].DeclaringType.FullName,
                            $"system {index} condition {condition} declaring type");
                    }
                }
            });

            // A gated system over a shared counter: one true and one false
            // condition must skip the body without failing the frame, and two
            // true conditions must run it.
            const string conditionProbe = """
                using TracyLive;

                namespace Probe;

                public struct Marker
                {
                    public int Value;
                }

                internal static class Gates
                {
                    internal static bool First;
                    internal static bool Second;

                    public static bool FirstOpen() => First;
                    public static bool SecondOpen() => Second;
                    public static bool HasMarker(Query<Read<Marker>> query)
                    {
                        _ = query;
                        return true;
                    }
                }

                internal static class ConditionRunState
                {
                    internal static int Runs;
                }

                public static class GatedSystem
                {
                    [System(RunIf = new[] { nameof(Gates.FirstOpen), nameof(Gates.SecondOpen) },
                        RunIfType = typeof(Gates))]
                    public static void Run(Commands commands)
                    {
                        _ = commands;
                        ConditionRunState.Runs++;
                    }

                    [System(RunIf = new[] { nameof(Gates.HasMarker) }, RunIfType = typeof(Gates))]
                    public static void ReadsMarker(Commands commands)
                    {
                        _ = commands;
                    }
                }
                """;

            // One probe per refusal the loader must make, each naming the
            // condition or the reason so the message can be asserted.
            const string conditionRefusals = """
                using TracyLive;

                namespace Probe;

                public struct RefusalMarker
                {
                    public int Value;
                }

                internal static class BadGates
                {
                    public static int Number() => 1;
                    public static bool Writable(ResMut<SimulationTime> time)
                    {
                        _ = time;
                        return true;
                    }
                    public static bool TakesCommands(Commands commands)
                    {
                        _ = commands;
                        return true;
                    }
                    public static bool Writes(Query<Write<RefusalMarker>> query)
                    {
                        _ = query;
                        return true;
                    }
                }

                public static class UnknownGateSystem
                {
                    [System(RunIf = new[] { "Missing" }, RunIfType = typeof(BadGates))]
                    public static void Run(Commands commands)
                    {
                        _ = commands;
                    }
                }

                public static class NonBoolGateSystem
                {
                    [System(RunIf = new[] { nameof(BadGates.Number) }, RunIfType = typeof(BadGates))]
                    public static void Run(Commands commands)
                    {
                        _ = commands;
                    }
                }

                public static class MutResourceGateSystem
                {
                    [System(RunIf = new[] { nameof(BadGates.Writable) }, RunIfType = typeof(BadGates))]
                    public static void Run(Commands commands)
                    {
                        _ = commands;
                    }
                }

                public static class CommandsGateSystem
                {
                    [System(RunIf = new[] { nameof(BadGates.TakesCommands) }, RunIfType = typeof(BadGates))]
                    public static void Run(Commands commands)
                    {
                        _ = commands;
                    }
                }

                public static class WriteQueryGateSystem
                {
                    [System(RunIf = new[] { nameof(BadGates.Writes) }, RunIfType = typeof(BadGates))]
                    public static void Run(Commands commands)
                    {
                        _ = commands;
                    }
                }
                """;

            Test("a false condition skips the system and a true one runs it", () =>
            {
                Assembly assembly = CompileProbe(
                    conditionProbe, "ConditionProbeRun", withGeneratedRegistry: false);
                ManagedSystem system = ProjectHost.DiscoverSystems(assembly)
                    .Single(entry => entry.Name == "Probe.GatedSystem.Run");
                const BindingFlags ProbeField = BindingFlags.NonPublic | BindingFlags.Static;
                FieldInfo first = assembly.GetType("Probe.Gates")!.GetField("First", ProbeField)!;
                FieldInfo second = assembly.GetType("Probe.Gates")!.GetField("Second", ProbeField)!;
                FieldInfo runs = assembly.GetType("Probe.ConditionRunState")!.GetField("Runs", ProbeField)!;

                first.SetValue(null, true);
                second.SetValue(null, false);
                system.Run();
                Equal(Convert.ToInt32(runs.GetValue(null)), 0,
                    "a false condition must skip the body without failing the frame");

                second.SetValue(null, true);
                system.Run();
                Equal(Convert.ToInt32(runs.GetValue(null)), 1,
                    "both conditions hold, so the body must run");
            });

            Test("a condition's read joins the system's declared access", () =>
            {
                Assembly assembly = CompileProbe(
                    conditionProbe, "ConditionProbeAccess", withGeneratedRegistry: false);
                ManagedSystem gated = ProjectHost.DiscoverSystems(assembly)
                    .Single(system => system.Name == "Probe.GatedSystem.ReadsMarker");
                Equal(gated.Accesses.Length, 1,
                    "the condition's component read must be one access row");
                Equal(gated.Accesses[0].Mode, (byte)0, "a condition read must stay a read");
            });

            Test("a condition the loader cannot evaluate is refused by name", () =>
            {
                Assembly assembly = CompileProbe(
                    conditionRefusals, "ConditionProbeRefusals", withGeneratedRegistry: false);
                MethodInfo System(string type) => assembly.GetType(type)!.GetMethod("Run")!;
                Refused(() => ProjectHost.CreateSystem(System("Probe.UnknownGateSystem")),
                    "Missing", "an unknown condition must be named");
                Refused(() => ProjectHost.CreateSystem(System("Probe.NonBoolGateSystem")),
                    "must return bool", "a non-bool condition must be refused");
                Refused(() => ProjectHost.CreateSystem(System("Probe.MutResourceGateSystem")),
                    "read-only", "a writable resource condition must be refused");
                Refused(() => ProjectHost.CreateSystem(System("Probe.CommandsGateSystem")),
                    "cannot take Commands", "a Commands condition must be refused");
                Refused(() => ProjectHost.CreateSystem(System("Probe.WriteQueryGateSystem")),
                    "query writes", "a writing query condition must be refused");
            });

            Test("an object is moved once and dropped once", () =>
            {
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                MirrorMethods.Reset();
                MirrorMethods.Register("Test.Object", "make",
                    (IntPtr)(delegate* unmanaged[Cdecl]<byte*, byte*, byte>)&MockNativeWorld.MirrorMakeObject);
                MirrorMethods.Register("Test.Object", "__type",
                    (IntPtr)(delegate* unmanaged[Cdecl]<byte*, byte*, byte>)&MockNativeWorld.MirrorDropObject);
                MirrorMethods.Register("Test.Object", "consume",
                    (IntPtr)(delegate* unmanaged[Cdecl]<byte*, byte*, byte>)&MockNativeWorld.MirrorConsumeObject);

                ProbeObject Make()
                {
                    var call = MirrorCall.Begin("Test.Object", "make");
                    call.Invoke();
                    return new ProbeObject(call.ResultObject(0));
                }

                int droppedBefore = MockNativeWorld.ObjectsDropped;
                ProbeObject moved = Make();
                var consume = MirrorCall.Begin("Test.Object", "consume");
                consume.PushMoved(moved);
                consume.Invoke();
                Assert(!moved.IsAlive, "a moved object owns nothing");
                Assert(MockNativeWorld.LastConsumed != IntPtr.Zero, "the box reached the call");
                Throws<ObjectDisposedException>(() =>
                {
                    var again = MirrorCall.Begin("Test.Object", "consume");
                    again.PushMoved(moved);
                }, "a moved object cannot be moved again");
                moved.Dispose();
                Equal(MockNativeWorld.ObjectsDropped, droppedBefore, "a moved object is not dropped by C#");

                ProbeObject disposed = Make();
                disposed.Dispose();
                disposed.Dispose();
                Equal(MockNativeWorld.ObjectsDropped, droppedBefore + 1, "dispose drops exactly once");
            });

            Test("an object that outlived a module reload is leaked, not dropped", () =>
            {
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);
                MirrorMethods.Reset();
                MirrorMethods.Register("Test.Object", "make",
                    (IntPtr)(delegate* unmanaged[Cdecl]<byte*, byte*, byte>)&MockNativeWorld.MirrorMakeObject);
                var call = MirrorCall.Begin("Test.Object", "make");
                call.Invoke();
                var stale = new ProbeObject(call.ResultObject(0));

                // A rebind is what an extension reload looks like to the runtime.
                MirrorMethods.Reset();
                MirrorMethods.Register("Test.Object", "__type",
                    (IntPtr)(delegate* unmanaged[Cdecl]<byte*, byte*, byte>)&MockNativeWorld.MirrorDropObject);
                int droppedBefore = MockNativeWorld.ObjectsDropped;
                Throws<InvalidOperationException>(() =>
                {
                    var use = MirrorCall.Begin("Test.Object", "__type");
                    use.PushBorrowed(stale);
                }, "a stale object refuses to be used");
                stale.Dispose();
                Equal(MockNativeWorld.ObjectsDropped, droppedBefore, "a stale object's drop must not run");
            });

            Test("a native resource marker is refused as a byte resource", () =>
            {
                Throws<InvalidOperationException>(
                    () => ResourceAccess.Borrow<AssetManager>(QueryAccess.Read),
                    "a Rust resource has no C# value");
                Equal(ResourceNames.Of(typeof(AssetManager)), "pill_engine::asset::AssetManager",
                    "the marker is named by the Rust shared name");
            });

            Test("a build that lands during a load is examined on the next poll", () =>
            {
                // The poll samples the assembly's write time and then loads the
                // bytes it finds. A build renamed over the file while those
                // bytes load has a later stamp and has not been examined, so a
                // load must remember the stamp it was given - a fresh sample at
                // the end of the load would skip the new build until the file
                // changed again.
                EngineApi api = MockNativeWorld.Api();
                Engine.Bind(&api);

                string directory = Path.Combine(
                    Path.GetTempPath(), $"pill_write_time_{Guid.NewGuid():N}");
                Directory.CreateDirectory(directory);
                try
                {
                    string path = Path.Combine(directory, "project_cs.dll");
                    File.Copy(typeof(BallPhysicsSystem).Assembly.Location, path);
                    DateTime first = new(2024, 1, 1, 0, 0, 0, DateTimeKind.Utc);
                    File.SetLastWriteTimeUtc(path, first);

                    var host = new ProjectHost(path);
                    Equal((byte)host.Load(isReload: false, writeUtc: first),
                        (byte)PollStatus.Reloaded, "the first version loads");
                    Assert(host.SystemCount > 0, "the copied project's systems are discovered");
                    Equal(host.LastExaminedWriteUtc, first,
                        "a load remembers the stamp it examined");

                    // A second build lands while that load runs: the file's
                    // stamp has moved on, but the load read the bytes the poll
                    // sampled `first` for.
                    DateTime second = first.AddMinutes(1);
                    File.SetLastWriteTimeUtc(path, second);
                    Equal((byte)host.Load(isReload: false, writeUtc: first),
                        (byte)PollStatus.Reloaded, "the racing load completes");
                    Equal(host.LastExaminedWriteUtc, first,
                        "a build that landed mid-load is not recorded as examined");

                    // So the next poll still has a version to examine, and both
                    // assemblies ran.
                    host.RequestImmediatePoll();
                    Equal((byte)host.PollReload(), (byte)PollStatus.Reloaded,
                        "the build that landed during the load is examined next poll");
                    Equal(host.LastExaminedWriteUtc, second,
                        "the newer build is the examined version now");
                }
                finally
                {
                    Directory.Delete(directory, recursive: true);
                }
            });

            Console.WriteLine($"C# ECS runtime tests passed: {_passed}");
            return 0;
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(exception);
            return 1;
        }
    }

    // =========================================================================
    // Benchmark mode
    // =========================================================================

    /// <summary>Accumulator the benchmark passes publish into, so the JIT
    /// cannot discard the work as dead code.</summary>
    private static float _benchSink;

    /// <summary>Columns the raw-cursor ceiling loop walks (pointer locals cannot
    /// be captured by a delegate, so the loop reads them from fields).</summary>
    private static unsafe TestPosition* _benchPositions;
    private static unsafe TestVelocity* _benchVelocities;
    private static unsafe NativeComponentTicks* _benchPositionTicks;
    private static uint _benchCount;

    /// <summary>
    /// Loop overhead alone: entity-only iteration touches no component column,
    /// so this isolates MoveNext plus the per-row row construction.
    /// </summary>
    private static void IterateLoopOnly()
    {
        float sum = 0;
        foreach (var row in new Query<EntityTerm>())
            sum += row.Entity.Id;
        _benchSink = sum;
    }

    /// <summary>MoveNext alone, with no Current access, to price the loop state machine.</summary>
    private static void IterateMoveNextOnly()
    {
        float sum = 0;
        var enumerator = new Query<EntityTerm>().GetEnumerator();
        while (enumerator.MoveNext())
            sum += 1;
        _benchSink = sum;
    }

    /// <summary>MoveNext plus Current (no accessor call), to separate row construction from access.</summary>
    private static void IterateMoveNextCurrent()
    {
        float sum = 0;
        var enumerator = new Query<EntityTerm>().GetEnumerator();
        while (enumerator.MoveNext())
            sum += enumerator.Current.Entity.Id;
        _benchSink = sum;
    }

    /// <summary>One accessor per row, to split its cost from the loop's.</summary>
    private static void IterateSingleRead()
    {
        float sum = 0;
        foreach (var row in new Query<Read<TestVelocity>>())
            sum += row.Read<TestVelocity>().X;
        _benchSink = sum;
    }

    /// <summary>
    /// Three read terms per row - the C# counterpart of the Rust
    /// `query_multi_component` bench (`(&Position, &Velocity, &Health)`).
    /// </summary>
    private static void IterateReadTriple()
    {
        float sum = 0;
        foreach (var row in new Query<
            Read<TestPosition>, Read<TestVelocity>, Read<TestHealth>>())
        {
            sum += row.Read<TestPosition>().X
                + row.Read<TestVelocity>().X
                + row.Read<TestHealth>().Value;
        }
        _benchSink = sum;
    }

    /// <summary>
    /// The same component work as the write pass with none of the query
    /// machinery: a raw cursor loop over the native columns. This is the floor
    /// the query API is measured against - Rust's iterator compiles to
    /// something very close to it.
    /// </summary>
    private static unsafe void IterateRawPointerPair()
    {
        float sum = 0;
        TestPosition* positions = _benchPositions;
        TestVelocity* velocities = _benchVelocities;
        for (uint index = 0; index < _benchCount; index++)
        {
            positions[index].X += velocities[index].X;
            positions[index].Y += velocities[index].Y;
            sum += positions[index].X;
        }
        _benchSink = sum;
    }

    /// <summary>One joined column as the row sees it: the row holds a ref to
    /// this and reads `Data` per access, exactly like `QueryRow` does.</summary>
    private struct SimColumn
    {
        public unsafe void* Data;
    }

    /// <summary>
    /// Simulation of the ref-row design: the row is built ONCE and the index
    /// advances per row, so the per-row cost is the index store plus whatever
    /// the accessors load. This is the floor the Rust-style `foreach` row loop
    /// could reach; the switch/match chains are omitted because the JIT folds
    /// them for closed shapes.
    /// </summary>
    private ref struct SimRow
    {
        public ref SimColumn C0;
        public ref SimColumn C1;
        public ref SimColumn C2;
        public int Index;

        public SimRow(ref SimColumn c0, ref SimColumn c1, ref SimColumn c2)
        {
            C0 = ref c0;
            C1 = ref c1;
            C2 = ref c2;
            Index = 0;
        }
    }

    /// <summary>Ref-row read pair: index store plus two accessor loads.</summary>
    private static unsafe void IterateStoredRowReadPair()
    {
        SimColumn c0 = new SimColumn { Data = _benchPositions };
        SimColumn c1 = new SimColumn { Data = _benchVelocities };
        SimColumn c2 = new SimColumn { Data = _benchPositionTicks };
        var row = new SimRow(ref c0, ref c1, ref c2);
        int count = (int)_benchCount;
        float sum = 0;
        for (int index = 0; index < count; index++)
        {
            row.Index = index;
            ref readonly TestPosition position =
                ref ((TestPosition*)row.C0.Data)[row.Index];
            ref readonly TestVelocity velocity =
                ref ((TestVelocity*)row.C1.Data)[row.Index];
            sum += position.X + velocity.X;
        }
        _benchSink = sum;
    }

    /// <summary>Ref-row write pair: the read shape plus the change-tick store.</summary>
    private static unsafe void IterateStoredRowWritePair()
    {
        SimColumn c0 = new SimColumn { Data = _benchPositions };
        SimColumn c1 = new SimColumn { Data = _benchVelocities };
        SimColumn c2 = new SimColumn { Data = _benchPositionTicks };
        var row = new SimRow(ref c0, ref c1, ref c2);
        int count = (int)_benchCount;
        float sum = 0;
        for (int index = 0; index < count; index++)
        {
            row.Index = index;
            ref TestPosition position = ref ((TestPosition*)row.C0.Data)[row.Index];
            ref readonly TestVelocity velocity =
                ref ((TestVelocity*)row.C1.Data)[row.Index];
            position.X += velocity.X;
            position.Y += velocity.Y;
            ((NativeComponentTicks*)row.C2.Data)[row.Index].Changed = 9;
            sum += position.X;
        }
        _benchSink = sum;
    }

    /// <summary>
    /// The read pair as a slice-API inner loop: two spans in locals, index
    /// loop with the length comparison the JIT elides bounds checks for. This
    /// is the shape a `Slices()`/span iteration path would compile to.
    /// </summary>
    private static unsafe void IterateSpanReadPair()
    {
        ReadOnlySpan<TestPosition> positions = new(_benchPositions, (int)_benchCount);
        ReadOnlySpan<TestVelocity> velocities = new(_benchVelocities, (int)_benchCount);
        float sum = 0;
        for (int index = 0; index < positions.Length; index++)
            sum += positions[index].X + velocities[index].X;
        _benchSink = sum;
    }

    /// <summary>
    /// The writable pass as a slice-API inner loop, including the per-row
    /// change-tick stamp the row path performs.
    /// </summary>
    private static unsafe void IterateSpanWritePair()
    {
        Span<TestPosition> positions = new(_benchPositions, (int)_benchCount);
        ReadOnlySpan<TestVelocity> velocities = new(_benchVelocities, (int)_benchCount);
        NativeComponentTicks* ticks = _benchPositionTicks;
        float sum = 0;
        for (int index = 0; index < positions.Length; index++)
        {
            positions[index].X += velocities[index].X;
            positions[index].Y += velocities[index].Y;
            ticks[index].Changed = 9;
            sum += positions[index].X;
        }
        _benchSink = sum;
    }

    /// <summary>
    /// Time the two query shapes the Rust `query_iteration` criterion bench
    /// covers - a read-only pair and a writable pair - over mock chunks of
    /// the same population, and report nanoseconds per row. Each scenario
    /// runs a fixed row budget, and every measurement is best-of-five so
    /// scheduler noise cannot inflate it. Build in Release to compare against
    /// the criterion numbers; Debug inflates them.
    /// </summary>
    private static unsafe void RunQueryBenchmark(uint[] counts)
    {
        const int rowBudget = 2_000_000;
        foreach (uint count in counts)
        {
            int passes = Math.Max(3, rowBudget / (int)count);
            // Step 1: native columns the mock chunks report, filled with the
            // same deterministic values the Rust benchmark world uses.
            TestPosition* positions = (TestPosition*)NativeMemory.Alloc(
                (nuint)count * (nuint)sizeof(TestPosition));
            TestVelocity* velocities = (TestVelocity*)NativeMemory.Alloc(
                (nuint)count * (nuint)sizeof(TestVelocity));
            TestHealth* healths = (TestHealth*)NativeMemory.Alloc(
                (nuint)count * (nuint)sizeof(TestHealth));
            Entity* entities = (Entity*)NativeMemory.Alloc(
                (nuint)count * (nuint)sizeof(Entity));
            NativeComponentTicks* positionTicks = (NativeComponentTicks*)NativeMemory.Alloc(
                (nuint)count * (nuint)sizeof(NativeComponentTicks));
            NativeComponentTicks* velocityTicks = (NativeComponentTicks*)NativeMemory.Alloc(
                (nuint)count * (nuint)sizeof(NativeComponentTicks));
            NativeComponentTicks* healthTicks = (NativeComponentTicks*)NativeMemory.Alloc(
                (nuint)count * (nuint)sizeof(NativeComponentTicks));
            for (uint index = 0; index < count; index++)
            {
                positions[index] = new TestPosition { X = index, Y = index * 2 };
                velocities[index] = new TestVelocity { X = 0.1f, Y = 0.2f };
                healths[index] = new TestHealth { Value = index % 100 };
                entities[index] = new Entity(index, 1);
                positionTicks[index] = velocityTicks[index] = healthTicks[index] =
                    new NativeComponentTicks { Added = 1, Changed = 2 };
            }
            MockNativeWorld.Positions = positions;
            MockNativeWorld.Velocities = velocities;
            MockNativeWorld.Healths = healths;
            MockNativeWorld.Entities = entities;
            MockNativeWorld.PositionTicks = positionTicks;
            MockNativeWorld.VelocityTicks = velocityTicks;
            MockNativeWorld.HealthTicks = healthTicks;
            MockNativeWorld.Length = count;
            MockNativeWorld.ChangeTick = 9;
            EngineApi api = MockNativeWorld.Api();
            Engine.Bind(&api);
            _benchPositions = positions;
            _benchVelocities = velocities;
            _benchPositionTicks = positionTicks;
            _benchCount = count;

            // Step 2: warm the JIT so the first timed pass is not the first
            // execution of the generic enumerator.
            for (int warmup = 0; warmup < 10; warmup++)
            {
                IterateMoveNextOnly();
                IterateMoveNextCurrent();
                IterateLoopOnly();
                IterateSingleRead();
                IterateReadTriple();
                IterateReadPair();
                IterateWritePair();
                IterateNamedRowsWritePair();
                IterateRawPointerPair();
                IterateSpanReadPair();
                IterateSpanWritePair();
                IterateStoredRowReadPair();
                IterateStoredRowWritePair();
            }

            double readSeconds = BestSeconds(5, passes, IterateReadPair);
            double writeSeconds = BestSeconds(5, passes, IterateWritePair);
            double namedSeconds = BestSeconds(5, passes, IterateNamedRowsWritePair);
            double legacySeconds = BestSeconds(5, passes, IterateLegacyWritePair);
            double loopSeconds = BestSeconds(5, passes, IterateLoopOnly);
            double moveNextSeconds = BestSeconds(5, passes, IterateMoveNextOnly);
            double moveNextCurrentSeconds = BestSeconds(5, passes, IterateMoveNextCurrent);
            double oneReadSeconds = BestSeconds(5, passes, IterateSingleRead);
            double tripleSeconds = BestSeconds(5, passes, IterateReadTriple);
            double ceilingSeconds = BestSeconds(5, passes, IterateRawPointerPair);
            double spanReadSeconds = BestSeconds(5, passes, IterateSpanReadPair);
            double spanWriteSeconds = BestSeconds(5, passes, IterateSpanWritePair);
            double storedReadSeconds = BestSeconds(5, passes, IterateStoredRowReadPair);
            double storedWriteSeconds = BestSeconds(5, passes, IterateStoredRowWritePair);
            double rows = (double)passes * count;
            Console.WriteLine(
                $"[bench] query rows={count} iters={passes} " +
                $"move_next_ns_per_row={moveNextSeconds * 1e9 / rows:F2} " +
                $"move_next_current_ns_per_row={moveNextCurrentSeconds * 1e9 / rows:F2} " +
                $"loop_only_ns_per_row={loopSeconds * 1e9 / rows:F2} " +
                $"one_read_ns_per_row={oneReadSeconds * 1e9 / rows:F2} " +
                $"read_triple_ns_per_row={tripleSeconds * 1e9 / rows:F2} " +
                $"read_pair_ns_per_row={readSeconds * 1e9 / rows:F2} " +
                $"write_pair_ns_per_row={writeSeconds * 1e9 / rows:F2} " +
                $"named_rows_write_ns_per_row={namedSeconds * 1e9 / rows:F2} " +
                $"raw_pointer_ns_per_row={ceilingSeconds * 1e9 / rows:F2} " +
                $"span_read_pair_ns_per_row={spanReadSeconds * 1e9 / rows:F2} " +
                $"span_write_tick_ns_per_row={spanWriteSeconds * 1e9 / rows:F2} " +
                $"ref_row_read_ns_per_row={storedReadSeconds * 1e9 / rows:F2} " +
                $"ref_row_write_ns_per_row={storedWriteSeconds * 1e9 / rows:F2} " +
                $"legacy_find_pair_ns_per_row={legacySeconds * 1e9 / rows:F2} " +
                $"checksum={_benchSink:F1}");

            // Step 3: release the benchmark columns.
            NativeMemory.Free(positions);
            NativeMemory.Free(velocities);
            NativeMemory.Free(healths);
            NativeMemory.Free(entities);
            NativeMemory.Free(positionTicks);
            NativeMemory.Free(velocityTicks);
            NativeMemory.Free(healthTicks);
            MockNativeWorld.Positions = null;
            MockNativeWorld.Velocities = null;
            MockNativeWorld.Healths = null;
            MockNativeWorld.Entities = null;
            MockNativeWorld.PositionTicks = null;
            MockNativeWorld.VelocityTicks = null;
            MockNativeWorld.HealthTicks = null;
            _benchPositions = null;
            _benchVelocities = null;
            _benchPositionTicks = null;
            _benchCount = 0;
        }
    }

    /// <summary>
    /// One read-only pass over the mock chunks - the C# shape of the Rust
    /// `Query::<(&Position, &Velocity)>` bench. Velocity and Health stand in
    /// for the Rust pair because the mock only maps Position as writable.
    /// </summary>
    private static void IterateReadPair()
    {
        float sum = 0;
        foreach (var row in new Query<Read<TestVelocity>, Read<TestHealth>>())
        {
            ref readonly TestVelocity velocity = ref row.Read<TestVelocity>();
            ref readonly TestHealth health = ref row.Read<TestHealth>();
            sum += velocity.X + health.Value;
        }
        _benchSink = sum;
    }

    /// <summary>One writable pass - the C# shape of the Rust `(&mut Position, &Velocity)` bench.</summary>
    private static void IterateWritePair()
    {
        float sum = 0;
        foreach (var row in new Query<Write<TestPosition>, Read<TestVelocity>>())
        {
            ref TestPosition position = ref row.Write<TestPosition>();
            ref readonly TestVelocity velocity = ref row.Read<TestVelocity>();
            position.X += velocity.X;
            position.Y += velocity.Y;
            sum += position.X;
        }
        _benchSink = sum;
    }

    /// <summary>
    /// The same pass through the source generator's named rows
    /// (`query.Rows()` with per-term properties), binding each term once -
    /// the idiomatic use, and the shape project_cs ships.
    /// </summary>
    private static void IterateNamedRowsWritePair()
    {
        float sum = 0;
        foreach (var row in new Query<Write<TestPosition>, Read<TestVelocity>>().Rows())
        {
            ref TestPosition position = ref row.TestPosition;
            ref readonly TestVelocity velocity = ref row.TestVelocity;
            position.X += velocity.X;
            position.Y += velocity.Y;
            sum += position.X;
        }
        _benchSink = sum;
    }

    /// <summary>
    /// The same writable pass through the pre-P0 shape-erased row, whose
    /// accessors scan the query's columns per row (`Find&lt;T&gt;`). This is the
    /// "before" the typed rows measured against, kept so the lookup cost the
    /// typed path removes stays quantified.
    /// </summary>
    private static void IterateLegacyWritePair()
    {
        float sum = 0;
        var query = new Query<Write<TestPosition>, Read<TestVelocity>>();
        var enumerator = ((QueryBase)query).GetEnumerator();
        while (enumerator.MoveNext())
        {
            QueryRow row = enumerator.Current;
            ref TestPosition position = ref row.Write<TestPosition>();
            ref readonly TestVelocity velocity = ref row.Read<TestVelocity>();
            position.X += velocity.X;
            position.Y += velocity.Y;
            sum += position.X;
        }
        _benchSink = sum;
    }

    /// <summary>Run <paramref name="pass"/> <paramref name="iterations"/> times and return the wall seconds.</summary>
    private static double MeasureSeconds(int iterations, Action pass)
    {
        var watch = new System.Diagnostics.Stopwatch();
        watch.Start();
        for (int iteration = 0; iteration < iterations; iteration++)
            pass();
        watch.Stop();
        return watch.Elapsed.TotalSeconds;
    }

    /// <summary>Best of <paramref name="repetitions"/> timed runs, to damp scheduler noise.</summary>
    private static double BestSeconds(int repetitions, int iterations, Action pass)
    {
        double best = double.MaxValue;
        for (int repetition = 0; repetition < repetitions; repetition++)
            best = Math.Min(best, MeasureSeconds(iterations, pass));
        return best;
    }

    /// <summary>
    /// Time the generated-accessor steady state: one generation compare, one
    /// cached raw view call, then the span walk a `{P}` property performs.
    /// Call and walk are timed separately so the boundary price and the
    /// element price do not hide each other.
    /// </summary>
    private static unsafe void RunAccessorBenchmark(int iterations)
    {
        const int elementCount = 64;
        float* elements = (float*)NativeMemory.Alloc((nuint)elementCount * sizeof(float));
        for (int index = 0; index < elementCount; index++)
            elements[index] = index;
        MockNativeWorld.AccessorElements = elements;
        MockNativeWorld.AccessorElementCount = elementCount;
        MirrorMethods.Reset();
        MirrorMethods.Register("Bench.Probe", "elements_view",
            (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, IntPtr*, IntPtr*, byte>)
                &MockNativeWorld.AccessorElementsView);

        // Mirror what a generated member caches between binds, then time the
        // call alone and the call plus the element walk.
        MirrorTrampoline address = MirrorMethods.Address("Bench.Probe", "elements_view");
        int boundGeneration = MirrorMethods.Generation;
        float callSink = 0;
        void CallOnly()
        {
            if (boundGeneration != MirrorMethods.Generation)
            {
                address = MirrorMethods.Address("Bench.Probe", "elements_view");
                boundGeneration = MirrorMethods.Generation;
            }
            MirrorMethods.InvokeView(address, new RowPointer((IntPtr)1), out BufferView view);
            callSink += view.Length;
        }

        float walkSink = 0;
        void CallAndWalk()
        {
            if (boundGeneration != MirrorMethods.Generation)
            {
                address = MirrorMethods.Address("Bench.Probe", "elements_view");
                boundGeneration = MirrorMethods.Generation;
            }
            MirrorMethods.InvokeView(address, new RowPointer((IntPtr)1), out BufferView view);
            ReadOnlySpan<float> span = ComponentViews.AsReadOnlySpan<float>(view);
            float sum = 0;
            for (int element = 0; element < span.Length; element++)
                sum += span[element];
            walkSink = sum;
        }

        // The idiomatic bounds-check-free walk: foreach over the span. Rust's
        // `iter().sum()` compiles to this shape, so it is the fair per-element
        // comparison.
        void CallAndWalkForeach()
        {
            if (boundGeneration != MirrorMethods.Generation)
            {
                address = MirrorMethods.Address("Bench.Probe", "elements_view");
                boundGeneration = MirrorMethods.Generation;
            }
            MirrorMethods.InvokeView(address, new RowPointer((IntPtr)1), out BufferView view);
            ReadOnlySpan<float> span = ComponentViews.AsReadOnlySpan<float>(view);
            float sum = 0;
            foreach (ref readonly float element in span)
                sum += element;
            walkSink = sum;
        }

        double callSeconds = BestSeconds(5, iterations, CallOnly);
        double fullSeconds = BestSeconds(5, iterations, CallAndWalk);
        double foreachSeconds = BestSeconds(5, iterations, CallAndWalkForeach);

        // The mock's trampolines are managed methods reached through an
        // unmanaged function pointer, so every call pays a managed ->
        // unmanaged -> managed round trip that real native trampolines do not.
        // Time that tax with the cheapest possible FFI call so it can be
        // subtracted from the accessor number.
        float taxSink = 0;
        void TaxLoop()
        {
            for (int iteration = 0; iteration < iterations; iteration++)
                taxSink += Engine.EntityCount();
        }
        double mockFfiSeconds = BestSeconds(5, 1, TaxLoop);
        double mockFfiNanoseconds = mockFfiSeconds * 1e9 / iterations;
        double callNanoseconds = callSeconds * 1e9 / iterations;
        double fullNanoseconds = fullSeconds * 1e9 / iterations;
        Console.WriteLine(
            $"[bench] accessor calls={iterations} elements={elementCount} " +
            $"call_ns={callNanoseconds:F1} full_ns={fullNanoseconds:F1} " +
            $"walk_ns={fullNanoseconds - callNanoseconds:F1} " +
            $"walk_element_ns={(fullNanoseconds - callNanoseconds) / elementCount:F2} " +
            $"foreach_ns={foreachSeconds * 1e9 / iterations:F1} " +
            $"foreach_element_ns={(foreachSeconds * 1e9 / iterations - callNanoseconds) / elementCount:F2} " +
            $"mock_ffi_tax_ns={mockFfiNanoseconds:F1} " +
            $"call_ns_minus_mock={callNanoseconds - mockFfiNanoseconds:F1} " +
            $"checksum={callSink + walkSink + taxSink + _benchSink:F1}");
        NativeMemory.Free(elements);
        MockNativeWorld.AccessorElements = null;
    }
}

/// <summary>A wrapped Rust value for the mirror-call tests.</summary>
internal sealed class ProbeObject : RustObject
{
    internal ProbeObject(RustObjectHandle handle) : base(handle, "Test.Object")
    {
    }
}
