// The engine facade: the managed entry points into the native world.
//
// Responsibilities:
// - Binds the native engine function table supplied by the Rust host.
// - Serves the calls a system makes: entity counts and identities, chunk and
//   resource resolution under the scheduler's declared access, and the
//   deferred commands queued for the end of the call.
// - Refreshes the mirrored-method table when the host republishes it, and owns
//   the stable component identity both sides hash a name into.
//
// Design:
// - Every call checks the native status against its slot's named vocabulary
//   and turns a refusal into an exception that names the reason.
// - The table is copied once at bind time; a slot the host does not provide
//   fails closed rather than being dereferenced.
// - Queries and rows live in their own files (QueryTerms.cs, QueryRows.cs);
//   this file holds only what a call into the engine needs.

using System.Diagnostics;
using System.Diagnostics.CodeAnalysis;
using System.Globalization;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;

namespace TracyLive;

// =============================================================================
// Native Engine Facade
// =============================================================================

/// <summary>Safe facade over the type-erased native ECS query API.</summary>
public static unsafe class Engine
{
    private static EngineApi _api;
    private static uint _mirrorEpoch;

    /// <summary>Bind the native function table for all subsequent queries.</summary>
    /// <exception cref="InvalidOperationException">
    /// The table was built for a different interop contract than this runtime.
    /// </exception>
    internal static void Bind(EngineApi* api)
    {
        // Check the header before copying: every slot after it is only
        // meaningful if both sides agree on the contract that laid them out.
        if (api->AbiVersion != Loader.LoaderInterop.InteropContractVersion
            || api->TableSize != (uint)sizeof(EngineApi))
        {
            throw new InvalidOperationException(
                $"the host's engine API table (contract {api->AbiVersion}, {api->TableSize} bytes) " +
                $"does not match this runtime (contract {Loader.LoaderInterop.InteropContractVersion}, " +
                $"{sizeof(EngineApi)} bytes); rebuild csharp_runtime and the host from the same checkout");
        }
        _api = *api;
        ReloadMirrorMethods();
    }

    /// <summary>Bind an unmanaged pointer received by the exported loader API.</summary>
    /// <remarks>
    /// This is the sharpest edge on the safe surface: anything holding a
    /// plausible <see cref="IntPtr"/> can bind an arbitrary function table.
    /// That is deliberate: hot-reloadable project code is trusted because it
    /// is the user's own build output, and the loader handshake has no safer
    /// channel. <c>AllowUnsafeBlocks</c> guards the keyword, not memory
    /// safety.
    /// </remarks>
    internal static void Bind(IntPtr api) => Bind((EngineApi*)api);

    /// <summary>
    /// Token of the managed invocation running on this thread, or zero outside
    /// one.
    /// </summary>
    internal static uint CurrentScopeToken =>
        _api.CurrentScopeToken == null ? 0u : _api.CurrentScopeToken();

    /// <summary>
    /// Run the host's parallel dispatch slot: one callback invocation per work
    /// item, on the host's shared thread pool, returning after every item
    /// finished.
    /// </summary>
    /// <remarks>
    /// Statuses are interpreted by <see cref="ParallelDispatch"/>; a null slot
    /// would mean the host predates this runtime's contract, which
    /// <see cref="Bind(EngineApi*)"/> already refuses at binding time.
    /// </remarks>
    internal static byte ParallelFor(
        delegate* unmanaged[Cdecl]<nint, uint, void> callback, nint state, uint count) =>
        _api.ParallelFor == null
            ? ParallelDispatch.DispatchFailed
            : _api.ParallelFor(callback, state, count);

    /// <summary>
    /// Reject a chunk that was issued to an earlier managed invocation.
    /// </summary>
    /// <remarks>
    /// Debug builds only, which is the same posture Unity takes with its job
    /// safety system: the checks run while you are developing and compile out
    /// of the build you ship, because validating on every row access would
    /// cost the data plane the property that makes it worth having.
    ///
    /// A mismatch means a chunk outlived the call that produced it. The
    /// storage it points at may have been moved by an archetype migration,
    /// freed, or unloaded with its module.
    /// </remarks>
    [Conditional("DEBUG")]
    internal static void ValidateChunkScope(uint issuedToScope, string component)
    {
        uint current = CurrentScopeToken;
        if (issuedToScope == current)
            return;
        throw new InvalidOperationException(
            $"The component chunk for {component} was issued to " +
            (issuedToScope == 0 ? "no managed invocation" : $"invocation {issuedToScope}") +
            (current == 0
                ? ", and no ECS system is running on this thread now."
                : $", but invocation {current} is running now.") +
            " Chunks are valid only inside the [System] call that produced them; " +
            "copy the value out, or re-run the query.");
    }

    /// <summary>
    /// Ask the host for one resource's bytes under the requested access.
    /// </summary>
    /// <remarks>
    /// Thin on purpose: the status byte is turned into a message by
    /// <c>ResourceAccess</c>, which knows the resource type and can name it.
    /// </remarks>
    internal static byte GetResourceView(
        StableComponentId id, byte mode, NativeResourceView* output)
    {
        if (_api.GetResourceView == null)
            return 1;
        return _api.GetResourceView(id.Low, id.High, mode, output);
    }

    internal static void Log(byte level, string target, string message)
    {
        if (_api.CSharpLog == null)
            return;
        byte[] targetBytes = Encoding.UTF8.GetBytes(target);
        byte[] messageBytes = Encoding.UTF8.GetBytes(message);
        fixed (byte* targetPointer = targetBytes)
        fixed (byte* messagePointer = messageBytes)
            _api.CSharpLog(level, targetPointer, (uint)targetBytes.Length,
                messagePointer, (uint)messageBytes.Length);
    }

    internal static ulong BeginZone(string name)
    {
        if (_api.CSharpZoneBegin == null)
            return 0;
        byte[] bytes = Encoding.UTF8.GetBytes(name);
        fixed (byte* pointer = bytes)
            return _api.CSharpZoneBegin(pointer, (uint)bytes.Length);
    }

    internal static void EndZone(ulong token)
    {
        if (token != 0 && _api.CSharpZoneEnd != null)
            _api.CSharpZoneEnd(token);
    }

    /// <summary>
    /// Reject a resource view that was issued to an earlier managed invocation.
    /// </summary>
    /// <remarks>
    /// Debug builds only, like <see cref="ValidateChunkScope"/>. A resource
    /// reference cannot normally go stale - every access re-asks the host - but
    /// a system that stashes one in a local and reads it from a lambda that
    /// runs later would, and this is what names that rather than reading
    /// storage the reload has since moved.
    /// </remarks>
    [Conditional("DEBUG")]
    internal static void ValidateResourceScope(uint issuedToScope, string resource)
    {
        uint current = CurrentScopeToken;
        if (issuedToScope == current)
            return;
        throw new InvalidOperationException(
            $"The view of resource {resource} was issued to " +
            (issuedToScope == 0 ? "no managed invocation" : $"invocation {issuedToScope}") +
            (current == 0
                ? ", and no ECS system is running on this thread now."
                : $", but invocation {current} is running now.") +
            " A resource reference is valid only inside the [System] call that " +
            "produced it; read it again, or copy the value out.");
    }

    /// <summary>
    /// Native size of one component row, after the manifest and the runtime
    /// have been proved to agree.
    /// </summary>
    /// <remarks>
    /// Exists so the agreement check is reachable from a test: resolving
    /// <c>ComponentTypeMetadata&lt;T&gt;</c> is what runs it, and a query or a
    /// deferred command is otherwise the only way to get there.
    /// </remarks>
    internal static int ComponentSizeOf<T>() where T : unmanaged =>
        ComponentTypeMetadata<T>.Size;

    /// <summary>Return the active native world's entity count.</summary>
    /// <exception cref="InvalidOperationException">
    /// No managed system is scheduled on the native side.
    /// </exception>
    public static uint EntityCount()
    {
        uint count;
        EntityCountStatus status = (EntityCountStatus)_api.EntityCount(&count);
        if (status == EntityCountStatus.Ok)
            return count;
        if (status == EntityCountStatus.NoInvocation)
            throw new InvalidOperationException(
                "EntityCount is only available while a system is scheduled.");
        throw new InvalidOperationException($"EntityCount failed with native status {(byte)status}.");
    }

    /// <summary>
    /// Read a live entity's stable id, the identity a saved reference uses.
    /// </summary>
    /// <returns>
    /// <c>false</c> when the entity is not live - destroyed, or a handle from
    /// another world - which is not an error: a reference to a destroyed entity
    /// is data the caller has to handle, not a broken call.
    /// </returns>
    /// <exception cref="InvalidOperationException">
    /// No managed system is scheduled on the native side.
    /// </exception>
    /// <remarks>
    /// The handle a query hands out names a slot that is recycled, so it cannot
    /// be stored in anything meant to outlive the run. This reads the stable id
    /// instead.
    /// </remarks>
    public static bool TryGetStableId(Entity entity, out StableEntityId id)
    {
        ulong low;
        ulong high;
        Entity target = entity;
        StableIdStatus status = (StableIdStatus)_api.EntityStableId(&target, &low, &high);
        if (status == StableIdStatus.Ok)
        {
            id = new StableEntityId(low, high);
            return true;
        }
        if (status == StableIdStatus.NotLive)
        {
            id = default;
            return false;
        }
        if (status == StableIdStatus.NoInvocation)
            throw new InvalidOperationException(
                "TryGetStableId is only available while a system is scheduled.");
        throw new InvalidOperationException($"TryGetStableId failed with native status {(byte)status}.");
    }

    /// <summary>
    /// Re-copy the host's mirrored-method table when the host has republished
    /// it since the last copy.
    ///
    /// The assembly swap that normally carries a rebind waits on a queued
    /// project build - seconds at best, and never if that build fails - while
    /// the trampoline addresses from the previous bind already point into the
    /// retiring module generation. Checking the host's epoch before resolving
    /// a mirrored method closes that window on the calling thread.
    /// </summary>
    internal static void RefreshMirrorMethodsIfStale()
    {
        // A host that never bound this runtime has no table to refresh, and no
        // call into the native table is possible either.
        if (_api.MirrorEpoch == null)
            return;
        if (_api.MirrorEpoch() == _mirrorEpoch)
            return;
        ReloadMirrorMethods();
    }

    /// <summary>
    /// Re-copy the host's mirrored-method table into <see cref="MirrorMethods"/>
    /// and drop the resolved delegate cache.
    ///
    /// Called at bind time and again after each managed assembly swap: an
    /// extension reload maps the module at a fresh base and may add or
    /// remove mirrored methods, so the addresses the generated structs call
    /// must be re-read from the host, and each collectible context defines its
    /// own delegate types.
    /// </summary>
    internal static void ReloadMirrorMethods()
    {
        // The epoch is read *before* the rows: a publish that lands while they
        // are being copied then leaves a newer epoch behind, so the next
        // refresh copies again instead of trusting a half-updated view.
        if (_api.MirrorEpoch != null)
            _mirrorEpoch = _api.MirrorEpoch();
        uint count = _api.MirrorMethodCount();
        // Built aside and published once: a call in flight during the refresh
        // keeps resolving against the previous bind's complete table instead of
        // a cleared one, and the generation moves once per bind rather than
        // once per row.
        Dictionary<(string TypeName, string Method), IntPtr> table = MirrorMethods.NewTable();
        if (count != 0)
        {
            MirrorMethodEntry[] buffer = new MirrorMethodEntry[count];
            uint written;
            fixed (MirrorMethodEntry* pointer = buffer)
                written = _api.CopyMirrorMethods(pointer, count);
            for (int index = 0; index < written; index++)
            {
                string typeName = Marshal.PtrToStringAnsi(buffer[index].TypeName) ?? string.Empty;
                string method = Marshal.PtrToStringAnsi(buffer[index].Method) ?? string.Empty;
                MirrorMethods.Register(table, typeName, method, buffer[index].Address);
            }
        }
        MirrorMethods.Publish(table);
    }

    /// <summary>Build the stable component ID shared with the Rust adapter.</summary>
    internal static ulong ComponentKey(Type type) => ComponentStableId(type).Low;
    internal static ulong ComponentKeyHigh(Type type) => ComponentStableId(type).High;
    internal static StableComponentId ComponentStableId(Type type) =>
        StableIdOf(type.FullName ?? type.Name);

    /// <summary>
    /// The stable 128-bit identity of one declared name.
    /// </summary>
    /// <remarks>
    /// A component's name is always its full type name; a resource may declare
    /// its own, so the hash is reachable from a string as well as a type. Both
    /// go through here, so the two can never drift apart. The two offsets and
    /// the mixing function are a contract with the host's
    /// `pill_csharp_bridge/src/components.rs` (`stable_component_id`), which
    /// carries the full derivation chain.
    /// </remarks>
    internal static StableComponentId StableIdOf(string name) =>
        new StableComponentId(HashName(name, 0xcbf29ce484222325),
            HashName(name, 0x84222325cbf29ce4));

    internal static bool TryGetChunk(
        QueryTermDescriptor term,
        uint index,
        out NativeComponentChunk chunk)
    {
        NativeComponentChunk result;
        byte status = _api.GetComponentChunk(
            term.ComponentKey, term.ComponentKeyHigh, (byte)term.Access, index, &result);
        chunk = result;
        if (status == 0)
            return false;
        ValidateStatus(status, term.ComponentType?.FullName ?? "<unknown>", term.Access);
        if (chunk.ElementSize != term.ComponentSize)
            throw new InvalidOperationException(
                $"Component {term.ComponentType!.FullName} has size {term.ComponentSize} in C# but " +
                $"{chunk.ElementSize} in Rust. The component layouts must match exactly.");
        if (term.Access == QueryAccess.Write && chunk.Ticks == IntPtr.Zero)
            throw new InvalidOperationException(
                $"Writable component {term.ComponentType!.FullName} has no native change-tick column.");
        return true;
    }

    /// <summary>
    /// Resolve one term's column inside an archetype a driver chunk already
    /// identified.
    /// </summary>
    /// <remarks>
    /// The enumerator calls this once per term per archetype instead of
    /// scanning chunk indices until the archetypes match, so the number of
    /// native calls no longer grows with the archetype count. Entity terms
    /// request the archetype's entity column through the same slot.
    /// </remarks>
    internal static bool TryGetArchetypeChunk(
        QueryTermDescriptor term,
        ulong archetypeLow,
        ulong archetypeHigh,
        out NativeComponentChunk chunk)
    {
        string name = term.ComponentType?.FullName ?? "entity";
        NativeComponentChunk result;
        byte mode = term.IsEntity ? (byte)2 : (byte)term.Access;
        byte status = _api.GetArchetypeChunk(
            archetypeLow,
            archetypeHigh,
            term.ComponentKey,
            term.ComponentKeyHigh,
            mode,
            &result);
        chunk = result;
        if (status == 0)
            return false;
        ValidateStatus(status, name, term.Access);
        if (chunk.ElementSize != term.ComponentSize)
            throw new InvalidOperationException(
                $"Component {name} has size {term.ComponentSize} in C# but " +
                $"{chunk.ElementSize} in Rust. The component layouts must match exactly.");
        if (term.Access == QueryAccess.Write && chunk.Ticks == IntPtr.Zero)
            throw new InvalidOperationException(
                $"Writable component {name} has no native change-tick column.");
        return true;
    }

    internal static bool TryGetEntityChunk(uint index, out NativeComponentChunk chunk)
    {
        NativeComponentChunk result;
        byte status = _api.GetEntityChunk(index, &result);
        chunk = result;
        if (status == 0)
            return false;
        ValidateStatus(status, nameof(Entity), QueryAccess.Read);
        if (chunk.ElementSize != Marshal.SizeOf<Entity>())
            throw new InvalidOperationException(
                $"Entity has size {Marshal.SizeOf<Entity>()} in C# but {chunk.ElementSize} in Rust.");
        return true;
    }

    internal static void ValidateStatus(byte status, string name, QueryAccess access)
    {
        switch ((QueryStatus)status)
        {
            case QueryStatus.Chunk:
                return;
            // Status 5 reports a caller bug on this side: every managed call
            // site passes a real result buffer, so a null one reached the
            // native table by some other route.
            case QueryStatus.NullOutput:
                throw new ArgumentException(
                    $"The native query for {name} was given no result buffer.", nameof(status));
            case QueryStatus.NotRegistered:
                throw new InvalidOperationException(
                    $"Component {name} is not registered by the Rust host.");
            case QueryStatus.NoInvocation:
                throw new InvalidOperationException(
                    "An ECS query was used outside its scheduled system call.");
            default:
                throw new InvalidOperationException(
                    $"The current system did not declare this {access.ToString().ToLowerInvariant()} access to {name}.");
        }
    }

    internal static Entity ReserveEntity()
    {
        Entity entity;
        ValidateCommandStatus(_api.ReserveEntity(&entity), "reserve an entity");
        return entity;
    }

    internal static void QueueCreate(Entity entity, Span<NativeComponentBlob> blobs)
    {
        fixed (NativeComponentBlob* pointer = blobs)
            ValidateCommandStatus(
                _api.QueueCreate(&entity, pointer, checked((uint)blobs.Length)),
                "create an entity");
    }

    internal static void QueueDestroy(Entity entity) =>
        ValidateCommandStatus(_api.QueueDestroy(&entity), "destroy an entity");

    internal static void QueueAdd<T>(Entity entity, T value) where T : unmanaged
    {
        StableComponentId id = ComponentTypeMetadata<T>.StableId;
        ValidateCommandStatus(
            _api.QueueAddComponent(
                &entity, id.Low, id.High, (byte*)&value,
                checked((uint)ComponentTypeMetadata<T>.Size)),
            $"add component {typeof(T).FullName}");
    }

    internal static void QueueRemove<T>(Entity entity) where T : unmanaged
    {
        StableComponentId id = ComponentTypeMetadata<T>.StableId;
        ValidateCommandStatus(
            _api.QueueRemoveComponent(&entity, id.Low, id.High),
            $"remove component {typeof(T).FullName}");
    }

    internal static void ValidateCommandStatus(byte status, string operation)
    {
        if ((CommandStatus)status == CommandStatus.Ok)
            return;
        string reason = ((CommandStatus)status) switch
        {
            CommandStatus.NotRegistered => "the component is not registered",
            CommandStatus.NoInvocation =>
                "Commands was used outside an active startup or scheduled system",
            CommandStatus.CommandsNotDeclared =>
                "the current system did not declare a Commands parameter",
            CommandStatus.StaleEntity =>
                "the entity handle is stale, invalid, or was not reserved by this invocation",
            CommandStatus.InvalidComponentList => "the component list or ABI layout is invalid",
            _ => $"native status {status}",
        };
        throw new InvalidOperationException($"Could not {operation}: {reason}.");
    }

    private static ulong HashName(string name, ulong offset)
    {
        const ulong prime = 0x100000001b3;
        ulong hash = offset;
        // Hashed through a stack buffer: a key is derived wherever a type or a
        // resource name is resolved, and a type name always fits.
        int length = Encoding.UTF8.GetByteCount(name);
        Span<byte> bytes = length <= 512 ? stackalloc byte[length] : new byte[length];
        Encoding.UTF8.GetBytes(name, bytes);
        foreach (byte value in bytes)
        {
            hash ^= value;
            hash = unchecked(hash * prime);
        }
        return hash;
    }

    // =========================================================================
    // Mirror ABI
    // =========================================================================

    /// <summary>
    /// Take the last error (<paramref name="kind"/> 0) or the returned string
    /// (1) the mirrored call that just ran on this thread left behind.
    /// </summary>
    internal static string? TakeMirrorText(byte kind)
    {
        if (_api.TakeMirrorText == null)
            return null;
        byte* data;
        uint length;
        if (_api.TakeMirrorText(kind, &data, &length) != 0)
            return null;
        return length == 0 ? string.Empty : Encoding.UTF8.GetString(data, checked((int)length));
    }

    /// <summary>
    /// Ask the host for the address of a Rust-owned resource under the
    /// running invocation's declared access.
    /// </summary>
    /// <remarks>The status byte is turned into a message by <see cref="MirrorCall"/>, which can name the type.</remarks>
    internal static byte GetNativeResource(StableComponentId id, byte mode, void** output)
    {
        if (_api.GetNativeResource == null)
            return 3;
        return _api.GetNativeResource(id.Low, id.High, mode, output);
    }

    /// <summary>Read a file below <c>res</c> through the engine's mounted asset store.</summary>
    /// <exception cref="EngineException">The file cannot be read.</exception>
    internal static byte[] ReadAsset(string path)
    {
        if (_api.AssetLoaderRead == null)
            throw new InvalidOperationException("The host offers no asset store to this runtime.");
        byte[] pathBytes = Encoding.UTF8.GetBytes(path);
        byte* data;
        uint length;
        byte status;
        fixed (byte* pathPointer = pathBytes)
            status = _api.AssetLoaderRead(pathPointer, (uint)pathBytes.Length, &data, &length);
        if (status != 0)
            throw new EngineException(
                $"Could not read asset \"{path}\": {TakeMirrorText(0) ?? $"native status {status}"}");
        return new ReadOnlySpan<byte>(data, checked((int)length)).ToArray();
    }

    /// <summary>
    /// The engine clock, as the frame being processed sees it; a managed
    /// system reads the same values a native one does.
    /// </summary>
    public static class Time
    {
        /// <summary>The frame's clamped delta in seconds; 0 outside a frame.</summary>
        public static float DeltaSeconds =>
            _api.TimeDeltaSeconds == null ? 0.0f : _api.TimeDeltaSeconds();

        /// <summary>Seconds since the engine started; 0 outside a frame.</summary>
        public static float ElapsedSeconds =>
            _api.TimeElapsedSeconds == null ? 0.0f : _api.TimeElapsedSeconds();
    }

    /// <summary>
    /// The input state of the frame being processed. Headless runs never
    /// receive events, so every query reads as released there.
    /// </summary>
    public static class Input
    {
        /// <summary>Whether <paramref name="key"/> is held this frame.</summary>
        public static bool KeyHeld(KeyCode key) =>
            _api.InputKeyHeld != null && _api.InputKeyHeld((byte)key) != 0;

        /// <summary>Whether <paramref name="button"/> is held this frame.</summary>
        public static bool MouseButtonHeld(MouseButton button) =>
            _api.InputMouseButtonHeld != null && _api.InputMouseButtonHeld((byte)button) != 0;

        /// <summary>
        /// Mouse motion since the previous frame, in physical pixels; keeps
        /// reporting at the window border, so it is the value for mouse-look.
        /// </summary>
        public static (float X, float Y) MouseDelta
        {
            get
            {
                if (_api.InputMouseDelta == null)
                    return (0.0f, 0.0f);
                float x, y;
                _api.InputMouseDelta(&x, &y);
                return (x, y);
            }
        }
    }
}

/// <summary>
/// The 128-bit stable identity of one managed component or resource, stored as
/// the same low/high halves the C ABI carries. The host's
/// `pill_csharp_bridge/src/components.rs` documents the full derivation chain.
/// </summary>
internal readonly record struct StableComponentId(ulong Low, ulong High);

// =============================================================================
// Deferred Entity Commands
// =============================================================================

/// <summary>
/// Stateless system parameter for deferred structural mutations. Native code
/// accepts calls only during the startup/system invocation that supplied it.
/// </summary>
public readonly struct Commands
{
    /// <summary>Begin describing a new entity.</summary>
    public DeferredEntityBuilder CreateEntity() => new();

    /// <summary>Queue adding one component after the current system phase.</summary>
    public void AddComponent<T>(Entity entity, T component) where T : unmanaged =>
        Engine.QueueAdd(entity, component);

    /// <summary>Queue removing one component after the current system phase.</summary>
    public void RemoveComponent<T>(Entity entity) where T : unmanaged =>
        Engine.QueueRemove<T>(entity);

    /// <summary>Queue entity destruction after the current system phase.</summary>
    public void DestroyEntity(Entity entity) => Engine.QueueDestroy(entity);
}

/// <summary>Managed component value retained until a creation command is queued.</summary>
internal readonly record struct DeferredComponentValue(
    StableComponentId Id, Type Type, byte[] Bytes);

/// <summary>Fluent, allocation-backed builder for one deferred entity.</summary>
public sealed class DeferredEntityBuilder
{
    private readonly List<DeferredComponentValue> _components = [];
    private readonly HashSet<UInt128> _ids = [];
    private bool _built;

    /// <summary>Add an unmanaged component value to this new entity.</summary>
    public DeferredEntityBuilder With<T>(T component) where T : unmanaged
    {
        if (_built)
            throw new InvalidOperationException("A deferred entity builder can only be built once.");
        StableComponentId id = ComponentTypeMetadata<T>.StableId;
        UInt128 key = ((UInt128)id.High << 64) | id.Low;
        if (!_ids.Add(key))
            throw new InvalidOperationException(
                $"Entity creation contains component {typeof(T).FullName} more than once.");
        byte[] bytes = new byte[ComponentTypeMetadata<T>.Size];
        MemoryMarshal.Write(bytes, in component);
        _components.Add(new DeferredComponentValue(id, typeof(T), bytes));
        return this;
    }

    /// <summary>Reserve the entity handle and queue its atomic creation.</summary>
    public Entity Build()
    {
        if (_built)
            throw new InvalidOperationException("A deferred entity builder can only be built once.");
        _built = true;
        Entity entity = Engine.ReserveEntity();
        var handles = new GCHandle[_components.Count];
        Span<NativeComponentBlob> blobs = _components.Count <= 64
            ? stackalloc NativeComponentBlob[_components.Count]
            : new NativeComponentBlob[_components.Count];
        try
        {
            for (int index = 0; index < _components.Count; index++)
            {
                DeferredComponentValue component = _components[index];
                handles[index] = GCHandle.Alloc(component.Bytes, GCHandleType.Pinned);
                blobs[index] = new NativeComponentBlob
                {
                    ComponentKey = component.Id.Low,
                    ComponentKeyHigh = component.Id.High,
                    Data = handles[index].AddrOfPinnedObject(),
                    Size = checked((uint)component.Bytes.Length),
                };
            }
            Engine.QueueCreate(entity, blobs);
            return entity;
        }
        finally
        {
            foreach (GCHandle handle in handles)
                if (handle.IsAllocated)
                    handle.Free();
        }
    }
}
