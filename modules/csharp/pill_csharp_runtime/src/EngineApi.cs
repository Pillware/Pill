// Native ABI declarations shared by the Rust host and managed runtime.
//
// Responsibilities:
// - Mirrors the Rust function table and component-chunk result layout.
// - Documents the exact calling convention used across the native boundary.
//
// Design:
// - These structs contain no behavior. Their sequential field order and sizes
//   are part of the ABI and must change in lockstep with pill_csharp_bridge/src/abi.rs.

using System.Runtime.InteropServices;

namespace TracyLive;

/// <summary>
/// Mirror of the Rust <c>EngineApi</c> struct: a table of native function
/// pointers into the engine. All pointers use the C calling convention.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public unsafe struct EngineApi
{
    /// <summary>
    /// The host's <see cref="Loader.LoaderInterop.InteropContractVersion"/>, checked
    /// before any slot below is copied.
    /// </summary>
    public uint AbiVersion;

    /// <summary>
    /// The host's size of this table in bytes; a table whose slots changed
    /// without a version bump fails this check instead of being misread.
    /// </summary>
    public uint TableSize;

    /// <summary>
    /// Write the entity count from the currently scheduled world.
    /// Status <c>0</c> wrote the count, <c>3</c> means no system is scheduled,
    /// and <c>5</c> means the caller passed no output buffer.
    /// </summary>
    public delegate* unmanaged[Cdecl]<uint*, byte> EntityCount;

    /// <summary>Request one native archetype column by component ID and mode.</summary>
    public delegate* unmanaged[Cdecl]<ulong, ulong, byte, uint, NativeComponentChunk*, byte> GetComponentChunk;

    /// <summary>Request one component's chunk within an already-known archetype.</summary>
    /// <remarks>Mode <c>2</c> requests the archetype's entity column.</remarks>
    public delegate* unmanaged[Cdecl]<ulong, ulong, ulong, ulong, byte, NativeComponentChunk*, byte> GetArchetypeChunk;

    /// <summary>Request one native archetype's entity-handle column.</summary>
    public delegate* unmanaged[Cdecl]<uint, NativeComponentChunk*, byte> GetEntityChunk;

    /// <summary>Reserve a generation-checked handle for deferred creation.</summary>
    public delegate* unmanaged[Cdecl]<Entity*, byte> ReserveEntity;

    /// <summary>Queue creation with an array of pinned component blobs.</summary>
    public delegate* unmanaged[Cdecl]<Entity*, NativeComponentBlob*, uint, byte> QueueCreate;

    /// <summary>Queue destruction of a currently live entity.</summary>
    public delegate* unmanaged[Cdecl]<Entity*, byte> QueueDestroy;

    /// <summary>Queue adding one native or runtime-defined component.</summary>
    public delegate* unmanaged[Cdecl]<Entity*, ulong, ulong, byte*, uint, byte> QueueAddComponent;

    /// <summary>Queue removing one component selected by stable identity.</summary>
    public delegate* unmanaged[Cdecl]<Entity*, ulong, ulong, byte> QueueRemoveComponent;

    /// <summary>Return how many mirrored Rust methods are registered.</summary>
    public delegate* unmanaged[Cdecl]<uint> MirrorMethodCount;

    /// <summary>Copy the mirrored-method rows into a caller-owned buffer.</summary>
    public delegate* unmanaged[Cdecl]<MirrorMethodEntry*, uint, uint> CopyMirrorMethods;

    /// <summary>
    /// Token of the managed invocation active on the calling thread, or zero
    /// when no scheduled system is running on it.
    /// </summary>
    public delegate* unmanaged[Cdecl]<uint> CurrentScopeToken;

    /// <summary>
    /// Epoch of the mirrored-method table: bumped every time the host
    /// republishes it, so managed code can notice a rebind without waiting for
    /// the assembly swap that would normally carry one.
    /// </summary>
    public delegate* unmanaged[Cdecl]<uint> MirrorEpoch;

    /// <summary>
    /// Fill a <see cref="NativeResourceView"/> for one resource the active
    /// system declared.
    /// </summary>
    /// <remarks>
    /// Arguments are the stable identity's low and high halves, the requested
    /// mode (<c>0</c> read, <c>1</c> write) and the output view. Status
    /// <c>0</c> filled it, <c>1</c> the identity names no registered resource,
    /// <c>2</c> the system did not declare this access, <c>3</c> no system is
    /// running on this thread, <c>4</c> the world holds no value yet, and
    /// <c>5</c> the caller passed no output buffer.
    /// </remarks>
    public delegate* unmanaged[Cdecl]<ulong, ulong, byte, NativeResourceView*, byte> GetResourceView;

    /// <summary>Emit a managed event through the native tracing subscriber.</summary>
    public delegate* unmanaged[Cdecl]<byte, byte*, uint, byte*, uint, void> CSharpLog;

    /// <summary>Begin a dynamic managed profiling zone and return its token.</summary>
    public delegate* unmanaged[Cdecl]<byte*, uint, ulong> CSharpZoneBegin;

    /// <summary>End a dynamic managed profiling zone.</summary>
    public delegate* unmanaged[Cdecl]<ulong, void> CSharpZoneEnd;

    /// <summary>
    /// Whether a key, by its <see cref="KeyCode"/> value, is held this frame.
    /// Returns `1` while held and `0` otherwise.
    /// </summary>
    public delegate* unmanaged[Cdecl]<byte, byte> InputKeyHeld;

    /// <summary>
    /// Whether a mouse button, by its <see cref="MouseButton"/> value, is held
    /// this frame; shaped like <see cref="InputKeyHeld"/>.
    /// </summary>
    public delegate* unmanaged[Cdecl]<byte, byte> InputMouseButtonHeld;

    /// <summary>
    /// Write the mouse motion since the previous frame, in physical pixels, to
    /// the two outputs; returns `1` when written.
    /// </summary>
    public delegate* unmanaged[Cdecl]<float*, float*, byte> InputMouseDelta;

    /// <summary>
    /// The frame's clamped delta in seconds; `0` with no clock.
    /// </summary>
    public delegate* unmanaged[Cdecl]<float> TimeDeltaSeconds;

    /// <summary>Seconds since the engine started; `0` with no clock.</summary>
    public delegate* unmanaged[Cdecl]<float> TimeElapsedSeconds;

    /// <summary>
    /// Run the managed trampoline for every index in <c>0..count</c> on the
    /// host's shared thread pool; returns after every invocation finished.
    /// Status <c>0</c> ran every index, <c>3</c> no managed system is
    /// scheduled on the calling thread, <c>4</c> called from inside a
    /// parallel callback, <c>5</c> an internal dispatch failure.
    /// <paramref name="state"/> is opaque and travels back to the callback
    /// untouched. Appended after <see cref="TimeElapsedSeconds"/> for the
    /// same reason every earlier addition was: a new slot goes at the end.
    /// </summary>
    public delegate* unmanaged[Cdecl]<delegate* unmanaged[Cdecl]<nint, uint, void>, nint, uint, byte> ParallelFor;

    /// <summary>
    /// Take the last error (<c>0</c>) or the returned string (<c>1</c>) a
    /// mirrored call left on this thread: writes a pointer and a length the
    /// caller copies before its next call. Status <c>0</c> handed text out,
    /// <c>1</c> the channel was empty.
    /// </summary>
    public delegate* unmanaged[Cdecl]<byte, byte**, uint*, byte> TakeMirrorText;

    /// <summary>
    /// Write the address of a Rust-owned resource, by its shared name's
    /// identity halves, under the requested mode (<c>0</c> read, <c>1</c>
    /// write). Status <c>0</c> wrote it, <c>1</c> the world holds no value,
    /// <c>2</c> the access was not declared, <c>3</c> no invocation is
    /// running.
    /// </summary>
    public delegate* unmanaged[Cdecl]<ulong, ulong, byte, void**, byte> GetNativeResource;

    /// <summary>
    /// Read a file below <c>res</c> through the mounted asset store for
    /// <see cref="AssetLoader.Load"/>; status <c>1</c> leaves the reason in
    /// the last-error channel.
    /// </summary>
    public delegate* unmanaged[Cdecl]<byte*, uint, byte**, uint*, byte> AssetLoaderRead;

    /// <summary>
    /// Write a live entity's stable id as its low and high halves; status
    /// <c>0</c> wrote it, <c>1</c> the entity is not live, <c>5</c> an output
    /// half was missing.
    /// </summary>
    public delegate* unmanaged[Cdecl]<Entity*, ulong*, ulong*, byte> EntityStableId;
}

/// <summary>
/// Borrowed view of one resource's bytes. Valid only for the managed system
/// invocation that asked for it.
/// </summary>
/// <remarks>
/// A resource is one value, so this carries far less than
/// <see cref="NativeComponentChunk"/>: no archetype identity, no row count and
/// no per-row tick column, only the bytes, their width, and the invocation the
/// view was issued to.
/// </remarks>
[StructLayout(LayoutKind.Sequential)]
public struct NativeResourceView
{
    /// <summary>Pointer to the resource's first byte in engine storage.</summary>
    internal IntPtr Data;

    /// <summary>Width of the stored value, checked against the managed struct.</summary>
    internal uint Length;

    /// <summary>Token of the managed invocation this view was issued to.</summary>
    internal uint ScopeToken;
}

/// <summary>
/// One mirrored Rust method the managed runtime can call through its exported
/// C-ABI trampoline. Field order and widths match the Rust host's
/// <c>MirrorMethodEntry</c>.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public struct MirrorMethodEntry
{
    /// <summary>Fully-qualified Rust type name, e.g. <c>pill_spline::OmoMO</c>.</summary>
    internal IntPtr TypeName;

    /// <summary>Rust method name, e.g. <c>get_sum</c>.</summary>
    internal IntPtr Method;

    /// <summary>Address of the exported <c>#[no_mangle]</c> C-ABI trampoline.</summary>
    internal IntPtr Address;
}

/// <summary>Pinned component value passed synchronously to native commands.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct NativeComponentBlob
{
    internal ulong ComponentKey;
    internal ulong ComponentKeyHigh;
    internal IntPtr Data;
    internal uint Size;
}

/// <summary>
/// Borrowed view of one native component column and its archetype identity.
/// The pointer is valid only for the active managed system invocation.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public struct NativeComponentChunk
{
    /// <summary>Low 64 bits of the native archetype identifier.</summary>
    internal ulong ArchetypeLow;

    /// <summary>High 64 bits of the native archetype identifier.</summary>
    internal ulong ArchetypeHigh;

    /// <summary>Pointer to the first component in the contiguous native column.</summary>
    /// <remarks>Null for entity columns, whose rows arrive in <see cref="Entities"/>.</remarks>
    internal IntPtr Data;

    /// <summary>Pointer to the first entity of an archetype's entity column.</summary>
    /// <remarks>
    /// Entity rows are const on the native side and arrive only through this
    /// slot, separate from the writable <see cref="Data"/>; the query join
    /// reads this one for entity terms.
    /// </remarks>
    internal IntPtr Entities;

    /// <summary>Number of component values in the column.</summary>
    internal uint Length;

    /// <summary>Native size of one component, used for ABI validation.</summary>
    internal uint ElementSize;

    /// <summary>Parallel per-row change metadata for component columns.</summary>
    internal IntPtr Ticks;

    /// <summary>World change tick current at managed system execution.</summary>
    internal uint ChangeTick;

    /// <summary>
    /// Token of the managed invocation this chunk was issued to.
    /// </summary>
    /// <remarks>
    /// Compared against the live token before the chunk is dereferenced in a
    /// debug build, so a chunk kept past the call that produced it is a named
    /// error instead of a read of storage that has since moved. Declared last
    /// to match the Rust struct, where it lands in tail padding the layout
    /// already carried and therefore costs no bytes.
    /// </remarks>
    internal uint ScopeToken;
}

/// <summary>ABI mirror of Rust's per-component change-detection metadata.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct NativeComponentTicks
{
    internal uint Added;
    internal uint Changed;
}
