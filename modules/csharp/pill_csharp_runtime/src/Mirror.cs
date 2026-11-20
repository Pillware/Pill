// The managed half of the mirror ABI: calling a mirrored Rust function, and the
// types its generated C# members are written in.
//
// Responsibilities:
// - Pack a mirrored call's arguments into 16-byte slots, call its trampoline
//   through the one C-ABI signature every trampoline shares, and read its
//   result (MirrorCall).
// - Own Rust values C# holds by box - a mesh being built, a shader builder -
//   with move semantics and a drop that never reaches into a reloaded module
//   (RustObject).
// - Give generated code the engine-level vocabulary it names: Handle<T>,
//   AssetLoader, the AssetManager resource and its typed operations.
//
// Design:
// - The slot encodings are the contract with `pill_engine::mirror` on the Rust
//   side, documented there; nothing here knows any extension's types. Every
//   extension's surface is generated from its own Rust attributes into its
//   `generated/*.g.cs`, and calls back through this file.
// - Generated members are safe C#: every pointer is produced and consumed in
//   here, so the reloadable project assembly needs no AllowUnsafeBlocks, and
//   one function-pointer type covers every trampoline, so NativeAOT generates
//   nothing per signature.
// - A call's argument data is copied into a per-thread native arena before the
//   call and released after it, so nothing managed needs pinning across the
//   boundary and a span argument can come from anywhere.

using System.Collections.Concurrent;
using System.Numerics;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;

namespace TracyLive;

// =============================================================================
// Errors
// =============================================================================

/// <summary>
/// A mirrored Rust function failed: it returned an <c>Err</c> or panicked.
/// </summary>
/// <remarks>The message is the Rust error's own text.</remarks>
public sealed class EngineException : Exception
{
    /// <summary>Wrap one failure message.</summary>
    public EngineException(string message) : base(message)
    {
    }
}

// =============================================================================
// Resources
// =============================================================================

/// <summary>
/// Marks a generated struct as the C# name of a Rust-owned resource: systems
/// declare <c>Res&lt;T&gt;</c> or <c>ResMut&lt;T&gt;</c> of it to reach the
/// resource through mirrored functions, and the scheduler orders them against
/// every Rust system that uses it.
/// </summary>
/// <remarks>
/// The resource's value stays in Rust - <c>.Value</c> on such a parameter is
/// refused - and the marker is never registered as a component or a managed
/// resource.
/// </remarks>
[AttributeUsage(AttributeTargets.Struct)]
public sealed class NativeResourceAttribute : Attribute
{
    /// <summary>Declare the Rust resource's shared name.</summary>
    public NativeResourceAttribute(string sharedName) => SharedName = sharedName;

    /// <summary>The name the Rust type gives <c>Resource::shared_name</c>.</summary>
    public string SharedName { get; }
}

/// <summary>
/// The engine's asset store. Declare <c>Res&lt;AssetManager&gt;</c> or
/// <c>ResMut&lt;AssetManager&gt;</c> to add, find and remove assets with the
/// <see cref="AssetManagerExtensions"/> methods.
/// </summary>
[NativeResource("pill_engine::AssetManager")]
public readonly struct AssetManager
{
}

// =============================================================================
// Handles
// =============================================================================

/// <summary>
/// A typed reference to one asset in the <see cref="AssetManager"/>: the
/// layout of Rust's <c>Handle&lt;T&gt;</c>, a slot index and a generation.
/// </summary>
/// <remarks>
/// A handle kept across the asset's removal no longer resolves: removing an
/// asset bumps its slot's generation.
/// </remarks>
[StructLayout(LayoutKind.Sequential)]
public readonly struct Handle<T> : IEquatable<Handle<T>>
{
    /// <summary>The asset's slot.</summary>
    public readonly uint Index;

    /// <summary>The slot's generation when the handle was issued.</summary>
    public readonly uint Generation;

    /// <summary>A handle from its two halves.</summary>
    public Handle(uint index, uint generation)
    {
        Index = index;
        Generation = generation;
    }

    /// <summary>The handle that names no asset - Rust's <c>Handle::INVALID</c>.</summary>
    public static Handle<T> Invalid => new(uint.MaxValue, uint.MaxValue);

    /// <summary>Whether this is not <see cref="Invalid"/>.</summary>
    public bool IsValid => Index != uint.MaxValue || Generation != uint.MaxValue;

    /// <inheritdoc/>
    public bool Equals(Handle<T> other) => Index == other.Index && Generation == other.Generation;

    /// <inheritdoc/>
    public override bool Equals(object? obj) => obj is Handle<T> other && Equals(other);

    /// <inheritdoc/>
    public override int GetHashCode() => HashCode.Combine(Index, Generation);

    /// <summary>Whether two handles name the same slot and generation.</summary>
    public static bool operator ==(Handle<T> left, Handle<T> right) => left.Equals(right);

    /// <summary>Whether two handles differ.</summary>
    public static bool operator !=(Handle<T> left, Handle<T> right) => !left.Equals(right);

    /// <inheritdoc/>
    public override string ToString() => $"Handle<{typeof(T).Name}>({Index}, gen {Generation})";
}

// =============================================================================
// Asset Loader
// =============================================================================

/// <summary>
/// Where an asset's bytes come from: a file below the project's <c>res</c>, or
/// bytes already in memory - Rust's <c>AssetLoader</c>.
/// </summary>
public sealed class AssetLoader
{
    private AssetLoader(string? path, byte[]? bytes)
    {
        PathBelowRes = path;
        Bytes = bytes;
    }

    /// <summary>The path below <c>res</c>, for a path loader.</summary>
    internal string? PathBelowRes { get; }

    /// <summary>The bytes, for a bytes loader.</summary>
    internal byte[]? Bytes { get; }

    /// <summary>A file below the project's <c>res</c>, found through the mounted packs.</summary>
    public static AssetLoader Path(string path) => new(path ?? throw new ArgumentNullException(nameof(path)), null);

    /// <summary>Bytes already in memory.</summary>
    public static AssetLoader FromBytes(byte[] bytes) => new(null, bytes ?? throw new ArgumentNullException(nameof(bytes)));

    /// <summary>
    /// Read the bytes: the file through the engine's asset store, or the bytes
    /// this loader holds.
    /// </summary>
    /// <exception cref="EngineException">The file cannot be read.</exception>
    public byte[] Load() => Bytes ?? Engine.ReadAsset(PathBelowRes!);
}

// =============================================================================
// Rust Objects
// =============================================================================

/// <summary>The address of a boxed Rust value a mirrored call returned.</summary>
/// <remarks>Opaque: only a mirrored call produces one, and only a generated class consumes it.</remarks>
public readonly struct RustObjectHandle
{
    internal readonly nint Pointer;
    internal RustObjectHandle(nint pointer) => Pointer = pointer;
}

/// <summary>A generated class wrapping a Rust value C# holds by box.</summary>
public interface IRustObject<TSelf> where TSelf : RustObject, IRustObject<TSelf>
{
    /// <summary>The Rust type's qualified name, which its functions are filed under.</summary>
    static abstract string RustTypeName { get; }

    /// <summary>Wrap a box a mirrored call returned.</summary>
    static abstract TSelf Wrap(RustObjectHandle handle);
}

/// <summary>A wrapped Rust type that is an asset: it can live in the <see cref="AssetManager"/>.</summary>
public interface IRustAsset<TSelf> : IRustObject<TSelf> where TSelf : RustObject, IRustAsset<TSelf>
{
}

/// <summary>A wrapped asset type imported from a source file through its <c>.meta</c> file.</summary>
public interface IRustImportedAsset<TSelf> : IRustAsset<TSelf> where TSelf : RustObject, IRustImportedAsset<TSelf>
{
}

/// <summary>A wrapped asset type whose file in <c>res</c> is the asset itself.</summary>
public interface IRustStandaloneAsset<TSelf> : IRustAsset<TSelf> where TSelf : RustObject, IRustStandaloneAsset<TSelf>
{
}

/// <summary>
/// A Rust value C# holds by box: created by a mirrored function that returns
/// it, moved into a function that takes it by value, borrowed by its methods,
/// and dropped when disposed or finalized.
/// </summary>
/// <remarks>
/// <para>
/// Moving is final: after a mirrored call takes the object by value - a
/// builder's <c>WithX</c>, <c>assets.AddNamed(name, mesh)</c> - this instance
/// is empty and every further use throws <see cref="ObjectDisposedException"/>.
/// </para>
/// <para>
/// Objects are meant to be short-lived: built and handed to the engine in one
/// go. One created before an extension reload must not run the retired
/// module's drop code, so after a reload it throws on use, and is leaked rather
/// than dropped (reported once per type).
/// </para>
/// </remarks>
public abstract class RustObject : IDisposable
{
    private static readonly ConcurrentDictionary<string, bool> ReportedLeaks = new();

    private nint _pointer;
    private readonly int _generation;

    /// <summary>Take ownership of a box a mirrored call returned.</summary>
    protected RustObject(RustObjectHandle handle, string rustTypeName)
    {
        if (handle.Pointer == 0)
            throw new ArgumentException("A mirrored call returned no object.", nameof(handle));
        _pointer = handle.Pointer;
        _generation = MirrorMethods.Generation;
        RustTypeName = rustTypeName;
    }

    /// <summary>The Rust type's qualified name.</summary>
    public string RustTypeName { get; }

    /// <summary>Whether this object still owns its Rust value.</summary>
    public bool IsAlive => Volatile.Read(ref _pointer) != 0;

    /// <summary>The box's address, for a call that borrows the value.</summary>
    internal nint Borrow()
    {
        nint pointer = Volatile.Read(ref _pointer);
        if (pointer == 0)
            throw Moved();
        ThrowIfStale();
        return pointer;
    }

    /// <summary>Give the box to a call that takes the value; this object is empty after.</summary>
    internal nint Take()
    {
        ThrowIfStale();
        nint pointer = Interlocked.Exchange(ref _pointer, 0);
        if (pointer == 0)
            throw Moved();
        GC.SuppressFinalize(this);
        return pointer;
    }

    /// <summary>
    /// Hand this object's box to another wrapper; this object is empty after.
    /// </summary>
    /// <remarks>
    /// How a generated constructor adopts the value its static <c>New</c>
    /// built: <c>: this(New(...).TakeHandle())</c>.
    /// </remarks>
    protected RustObjectHandle TakeHandle() => new(Take());

    /// <summary>Drop the Rust value now.</summary>
    public void Dispose()
    {
        Release();
        GC.SuppressFinalize(this);
    }

    /// <summary>Drop the Rust value the object was never disposed of.</summary>
    ~RustObject() => Release();

    private ObjectDisposedException Moved() => new(
        GetType().Name,
        $"This {GetType().Name} was moved into a Rust call or disposed; it no longer owns a value.");

    private void ThrowIfStale()
    {
        if (_generation != MirrorMethods.Generation)
            throw new InvalidOperationException(
                $"This {GetType().Name} was created before an engine module reloaded, so its " +
                "Rust code may no longer be loaded. Build it again after the reload.");
    }

    private void Release()
    {
        nint pointer = Interlocked.Exchange(ref _pointer, 0);
        if (pointer == 0)
            return;
        if (_generation != MirrorMethods.Generation)
        {
            // The module that allocated the value may be gone: leaking it is
            // the only safe answer.
            if (ReportedLeaks.TryAdd(RustTypeName, true))
                Engine.Log(3, "csharp.mirror",
                    $"leaked a {RustTypeName} created before a module reload; its drop code may be unloaded");
            return;
        }
        try
        {
            var call = MirrorCall.Begin(RustTypeName, MirrorCall.TypeRow);
            call.PushPointer(pointer);
            call.Invoke();
        }
        catch (Exception exception)
        {
            // A finalizer must not throw; a failed drop is reported instead.
            Engine.Log(3, "csharp.mirror", $"dropping a {RustTypeName} failed: {exception.Message}");
        }
    }
}

// =============================================================================
// Calls
// =============================================================================

/// <summary>
/// One call of a mirrored Rust function, as generated members make it: begin,
/// push the receiver and each argument, invoke, read the result.
/// </summary>
/// <remarks>
/// Not for hand-written code: the encodings must match the Rust signature
/// exactly, which only the generator knows. A <c>ref struct</c> so a call
/// cannot outlive the frame that began it.
/// </remarks>
public unsafe ref struct MirrorCall
{
    /// <summary>The descriptor row naming a type; an object's is its drop.</summary>
    internal const string TypeRow = "__type";

    private const int SlotSize = 16;

    private readonly MirrorFrame _frame;
    private readonly nint _address;
    private readonly string _typeName;
    private readonly string _method;
    private int _slot;

    private MirrorCall(MirrorFrame frame, nint address, string typeName, string method)
    {
        _frame = frame;
        _address = address;
        _typeName = typeName;
        _method = method;
        _slot = 0;
    }

    /// <summary>Begin a call of one mirrored function.</summary>
    /// <param name="typeName">The Rust qualified name the function is filed under.</param>
    /// <param name="method">The Rust function's name.</param>
    /// <param name="returnSize">Bytes the result needs, when more than the default buffer.</param>
    public static MirrorCall Begin(string typeName, string method, int returnSize = 0)
    {
        Engine.RefreshMirrorMethodsIfStale();
        nint address = MirrorMethods.Address(typeName, method).Address;
        return new MirrorCall(MirrorFrame.Current(returnSize), address, typeName, method);
    }

    private byte* NextSlot()
    {
        if (_slot >= MirrorFrame.MaxSlots)
            throw new InvalidOperationException(
                $"{_typeName}::{_method} takes more than {MirrorFrame.MaxSlots} arguments.");
        byte* slot = _frame.Arguments + _slot * SlotSize;
        new Span<byte>(slot, SlotSize).Clear();
        _slot++;
        return slot;
    }

    // ---- Arguments -------------------------------------------------------

    /// <summary>A plain value written into its slot: a primitive, an enum, a handle, a vector.</summary>
    public void Push<T>(T value) where T : unmanaged
    {
        if (sizeof(T) > SlotSize)
            throw new InvalidOperationException($"{typeof(T)} does not fit a mirror slot.");
        Unsafe.WriteUnaligned(NextSlot(), value);
    }

    /// <summary>A field of a tuple slot, at its offset; <see cref="EndSlot"/> moves on.</summary>
    public void PushField<T>(int offset, T value) where T : unmanaged
    {
        if (_slot >= MirrorFrame.MaxSlots || offset + sizeof(T) > SlotSize)
            throw new InvalidOperationException($"A tuple field of {_typeName}::{_method} does not fit its slot.");
        byte* slot = _frame.Arguments + _slot * SlotSize;
        if (offset == 0)
            new Span<byte>(slot, SlotSize).Clear();
        Unsafe.WriteUnaligned(slot + offset, value);
    }

    /// <summary>Finish a tuple slot written by <see cref="PushField{T}"/>.</summary>
    public void EndSlot() => _slot++;

    /// <summary>A string, as UTF-8.</summary>
    public void PushString(string? value)
    {
        byte* slot = NextSlot();
        WriteUtf8(slot, value ?? string.Empty);
    }

    /// <summary>A run of plain elements, copied.</summary>
    public void PushSpan<T>(ReadOnlySpan<T> values) where T : unmanaged
    {
        byte* slot = NextSlot();
        int bytes = checked(values.Length * sizeof(T));
        byte* data = _frame.Allocate(bytes);
        MemoryMarshal.AsBytes(values).CopyTo(new Span<byte>(data, bytes));
        WriteSpan(slot, data, values.Length);
    }

    /// <summary>A run of strings, each as UTF-8.</summary>
    public void PushStrings(ReadOnlySpan<string> values)
    {
        byte* slot = NextSlot();
        byte* elements = _frame.Allocate(checked(values.Length * SlotSize));
        for (int index = 0; index < values.Length; index++)
            WriteUtf8(elements + index * SlotSize, values[index] ?? string.Empty);
        WriteSpan(slot, elements, values.Length);
    }

    /// <summary>A run of objects, each moved into the call.</summary>
    public void PushObjects<T>(ReadOnlySpan<T> values) where T : RustObject
    {
        foreach (T value in values)
        {
            if (value is null)
                throw new ArgumentNullException(nameof(values), $"{_typeName}::{_method} was given a null element.");
            if (!value.IsAlive)
                throw new ObjectDisposedException(value.GetType().Name,
                    $"An element passed to {_typeName}::{_method} was already moved or disposed.");
        }
        byte* slot = NextSlot();
        byte* elements = _frame.Allocate(checked(values.Length * sizeof(nint)));
        for (int index = 0; index < values.Length; index++)
            ((nint*)elements)[index] = values[index].Take();
        WriteSpan(slot, elements, values.Length);
    }

    /// <summary>An object the call borrows (<c>&amp;T</c> / <c>&amp;mut T</c>).</summary>
    public void PushBorrowed(RustObject value)
    {
        ArgumentNullException.ThrowIfNull(value);
        nint pointer = value.Borrow();
        Unsafe.WriteUnaligned(NextSlot(), pointer);
    }

    /// <summary>An object the call takes by value; the object is empty afterwards.</summary>
    public void PushMoved(RustObject value)
    {
        ArgumentNullException.ThrowIfNull(value);
        nint pointer = value.Take();
        Unsafe.WriteUnaligned(NextSlot(), pointer);
    }

    /// <summary>A raw box address (a drop).</summary>
    internal void PushPointer(nint pointer) => Unsafe.WriteUnaligned(NextSlot(), pointer);

    /// <summary>An optional plain value: the value, and a presence byte at 15.</summary>
    public void PushOptional<T>(T? value) where T : unmanaged
    {
        if (sizeof(T) > 15)
            throw new InvalidOperationException($"An optional {typeof(T)} does not fit a mirror slot.");
        byte* slot = NextSlot();
        if (value is T present)
        {
            Unsafe.WriteUnaligned(slot, present);
            slot[15] = 1;
        }
    }

    /// <summary>An optional object, moved into the call when present.</summary>
    public void PushOptionalObject(RustObject? value)
    {
        byte* slot = NextSlot();
        if (value is not null)
        {
            Unsafe.WriteUnaligned(slot, value.Take());
            slot[15] = 1;
        }
    }

    /// <summary>An asset loader: data pointer at 0, length at 8, kind at 12.</summary>
    public void PushLoader(AssetLoader loader)
    {
        ArgumentNullException.ThrowIfNull(loader);
        byte* slot = NextSlot();
        if (loader.Bytes is byte[] bytes)
        {
            byte* data = _frame.Allocate(bytes.Length);
            bytes.CopyTo(new Span<byte>(data, bytes.Length));
            WriteSpan(slot, data, bytes.Length);
            slot[12] = 1;
        }
        else
        {
            WriteUtf8(slot, loader.PathBelowRes ?? string.Empty);
            slot[12] = 0;
        }
    }

    /// <summary>
    /// The address of a Rust-owned resource, under the access the running
    /// system declared for it.
    /// </summary>
    public void PushResource<T>(QueryAccess access) where T : unmanaged
    {
        StableComponentId id = ResourceTypeMetadata<T>.StableId;
        void* pointer;
        byte status = Engine.GetNativeResource(id, (byte)access, &pointer);
        if (status != 0)
            throw NativeResourceFailure(ResourceTypeMetadata<T>.Name, typeof(T).Name, access, status);
        Unsafe.WriteUnaligned(NextSlot(), (nint)pointer);
    }

    /// <summary>The address of a value the caller holds (a component row, a local).</summary>
    public void PushAddress<T>(ref T value) where T : unmanaged =>
        Unsafe.WriteUnaligned(NextSlot(), (nint)Unsafe.AsPointer(ref value));

    /// <summary>A value copied for the call, passed by address.</summary>
    public void PushCopy<T>(in T value) where T : unmanaged
    {
        byte* data = _frame.Allocate(sizeof(T));
        Unsafe.WriteUnaligned(data, value);
        Unsafe.WriteUnaligned(NextSlot(), (nint)data);
    }

    // ---- Invocation ------------------------------------------------------

    /// <summary>Call the trampoline; a failure becomes an <see cref="EngineException"/>.</summary>
    public void Invoke()
    {
        byte status = ((delegate* unmanaged[Cdecl]<byte*, byte*, byte>)_address)(
            _frame.Arguments, _frame.Result);
        if (status != 0)
        {
            string message = Engine.TakeMirrorText(0) ?? $"native status {status}";
            throw new EngineException($"{_typeName}::{_method} failed: {message}");
        }
    }

    // ---- Results ---------------------------------------------------------

    /// <summary>The result, read from the start of the result buffer.</summary>
    public readonly T Result<T>() where T : unmanaged => Unsafe.ReadUnaligned<T>(_frame.Result);

    /// <summary>A part of the result, at its offset.</summary>
    public readonly T ResultAt<T>(int offset) where T : unmanaged =>
        Unsafe.ReadUnaligned<T>(_frame.Result + offset);

    /// <summary>Whether an optional result is present.</summary>
    public readonly bool ResultPresent() => _frame.Result[0] != 0;

    /// <summary>A returned box, at its offset.</summary>
    public readonly RustObjectHandle ResultObject(int offset) =>
        new(Unsafe.ReadUnaligned<nint>(_frame.Result + offset));

    /// <summary>A returned string.</summary>
    public readonly string ResultString() => Engine.TakeMirrorText(1) ?? string.Empty;

    // ---- Encoding helpers ------------------------------------------------

    private readonly void WriteUtf8(byte* slot, string value)
    {
        int length = Encoding.UTF8.GetByteCount(value);
        byte* data = _frame.Allocate(length);
        Encoding.UTF8.GetBytes(value, new Span<byte>(data, length));
        WriteSpan(slot, data, length);
    }

    private static void WriteSpan(byte* slot, byte* data, int count)
    {
        Unsafe.WriteUnaligned(slot, (nint)data);
        Unsafe.WriteUnaligned(slot + 8, checked((uint)count));
    }

    internal static InvalidOperationException NativeResourceFailure(
        string name, string csharpName, QueryAccess access, byte status) =>
        ((NativeResourceStatus)status) switch
        {
            NativeResourceStatus.NoValue =>
                new InvalidOperationException($"The world holds no {name} resource."),
            NativeResourceStatus.AccessNotDeclared => new InvalidOperationException(
                $"This system did not declare {access} access to {name}. Add a " +
                $"{(access == QueryAccess.Write ? "ResMut" : "Res")}<{csharpName}> parameter."),
            NativeResourceStatus.NoInvocation => new InvalidOperationException(
                $"{name} can only be reached from inside an [System] or [Startup] call."),
            _ => new InvalidOperationException(
                $"{name} could not be reached (status {status})."),
        };
}

/// <summary>
/// One thread's argument slots, result buffer and argument arena, reused by
/// every mirrored call that thread makes.
/// </summary>
/// <remarks>
/// The arena is a list of native blocks that never move, so a pointer written
/// into a slot stays valid until the call returns; the next call on the
/// thread rewinds it. Per thread because scheduled systems call mirrored
/// functions from parallel batches.
/// </remarks>
internal sealed unsafe class MirrorFrame
{
    /// <summary>The most arguments, receiver included, one call can take.</summary>
    internal const int MaxSlots = 32;

    private const int DefaultResultSize = 64;
    private const int BlockSize = 64 * 1024;

    [ThreadStatic]
    private static MirrorFrame? t_current;

    private readonly List<(nint Data, int Size)> _blocks = new();
    private int _block;
    private int _used;
    private int _resultSize;

    private MirrorFrame()
    {
        Arguments = (byte*)NativeMemory.AlignedAlloc(MaxSlots * 16, 16);
        _resultSize = DefaultResultSize;
        Result = (byte*)NativeMemory.AlignedAlloc((nuint)_resultSize, 16);
    }

    /// <summary>The argument slots.</summary>
    internal byte* Arguments { get; }

    /// <summary>The result buffer.</summary>
    internal byte* Result { get; private set; }

    /// <summary>This thread's frame, rewound for a new call.</summary>
    internal static MirrorFrame Current(int returnSize)
    {
        MirrorFrame frame = t_current ??= new MirrorFrame();
        frame._block = 0;
        frame._used = 0;
        int needed = Math.Max(DefaultResultSize, returnSize);
        if (needed > frame._resultSize)
        {
            NativeMemory.AlignedFree(frame.Result);
            frame.Result = (byte*)NativeMemory.AlignedAlloc((nuint)needed, 16);
            frame._resultSize = needed;
        }
        new Span<byte>(frame.Result, frame._resultSize).Clear();
        return frame;
    }

    /// <summary>Copy space for one argument's data, 16-byte aligned.</summary>
    internal byte* Allocate(int size)
    {
        size = (size + 15) & ~15;
        while (true)
        {
            if (_block < _blocks.Count)
            {
                (nint data, int capacity) = _blocks[_block];
                if (_used + size <= capacity)
                {
                    byte* pointer = (byte*)data + _used;
                    _used += size;
                    return pointer;
                }
                _block++;
                _used = 0;
                continue;
            }
            int capacityNeeded = Math.Max(BlockSize, size);
            _blocks.Add(((nint)NativeMemory.AlignedAlloc((nuint)capacityNeeded, 16), capacityNeeded));
        }
    }
}

// =============================================================================
// Asset Store Operations
// =============================================================================

/// <summary>What <see cref="AssetManagerExtensions.Import{T}"/> does when the source has no <c>.meta</c> file yet.</summary>
public enum MetadataPolicy : byte
{
    /// <summary>Use the initial settings and write nothing.</summary>
    ReadIfPresent = 0,
    /// <summary>Use the initial settings and write the <c>.meta</c> file beside the source.</summary>
    CreateIfMissing = 1,
}

/// <summary>Where an imported asset's guid came from.</summary>
public enum MetadataSource : byte
{
    /// <summary>Read from the <c>.meta</c> file.</summary>
    ReadFromFile = 0,
    /// <summary>A <c>.meta</c> file was written now.</summary>
    CreatedOnDisk = 1,
    /// <summary>Generated in memory; no file was written.</summary>
    InMemoryOnly = 2,
}

/// <summary>The result of importing one asset.</summary>
public readonly struct ImportedAsset<T>
{
    /// <summary>The asset's handle.</summary>
    public readonly Handle<T> Handle;

    /// <summary>
    /// The asset's guid, as the 32 hexadecimal digits its file holds, or
    /// <c>null</c> when it carries none (a source added in memory).
    /// </summary>
    public readonly string? Guid;

    /// <summary>The path was already loaded, and its existing handle was returned.</summary>
    public readonly bool AlreadyLoaded;

    /// <summary>Where the guid came from.</summary>
    public readonly MetadataSource Metadata;

    internal ImportedAsset(Handle<T> handle, string? guid, bool alreadyLoaded, MetadataSource metadata)
    {
        Handle = handle;
        Guid = guid;
        AlreadyLoaded = alreadyLoaded;
        Metadata = metadata;
    }
}

/// <summary>
/// The engine's asset store operations, on the <see cref="AssetManager"/>
/// resource a system or startup declared - the managed face of Rust's
/// <c>AssetManager</c>, for every mirrored asset type.
/// </summary>
/// <remarks>
/// Each call reaches the asset type's own generated trampoline, so this class
/// names no asset type: a mesh, a texture, a sound and any other extension's
/// asset go through the same methods.
/// </remarks>
public static class AssetManagerExtensions
{
    /// <summary>Store <paramref name="asset"/>; the object is moved into the store.</summary>
    public static Handle<T> Add<T>(this ResMut<AssetManager> assets, T asset)
        where T : RustObject, IRustAsset<T>
    {
        var call = MirrorCall.Begin(T.RustTypeName, "__asset_add");
        call.PushResource<AssetManager>(QueryAccess.Write);
        call.PushMoved(asset);
        call.Invoke();
        return call.Result<Handle<T>>();
    }

    /// <summary>Store <paramref name="asset"/> under <paramref name="name"/>; a name in use throws.</summary>
    public static Handle<T> AddNamed<T>(this ResMut<AssetManager> assets, string name, T asset)
        where T : RustObject, IRustAsset<T>
    {
        var call = MirrorCall.Begin(T.RustTypeName, "__asset_add_named");
        call.PushResource<AssetManager>(QueryAccess.Write);
        call.PushString(name);
        call.PushMoved(asset);
        call.Invoke();
        return call.Result<Handle<T>>();
    }

    /// <summary>Store <paramref name="asset"/> under a name and a guid (32 hexadecimal digits).</summary>
    public static Handle<T> AddNamedWithGuid<T>(this ResMut<AssetManager> assets, string name, string guid, T asset)
        where T : RustObject, IRustAsset<T>
    {
        var call = MirrorCall.Begin(T.RustTypeName, "__asset_add_named_with_guid");
        call.PushResource<AssetManager>(QueryAccess.Write);
        call.PushString(name);
        call.PushString(guid);
        call.PushMoved(asset);
        call.Invoke();
        return call.Result<Handle<T>>();
    }

    /// <summary>Remove an asset, returning it; <c>null</c> when the handle is stale.</summary>
    public static T? Remove<T>(this ResMut<AssetManager> assets, Handle<T> handle)
        where T : RustObject, IRustAsset<T>
    {
        var call = MirrorCall.Begin(T.RustTypeName, "__asset_remove");
        call.PushResource<AssetManager>(QueryAccess.Write);
        call.Push(handle);
        call.Invoke();
        RustObjectHandle removed = call.ResultObject(0);
        return removed.Pointer == 0 ? null : T.Wrap(removed);
    }

    /// <summary>Whether <paramref name="handle"/> names a live asset.</summary>
    public static bool Contains<T>(this Res<AssetManager> assets, Handle<T> handle)
        where T : RustObject, IRustAsset<T> => ContainsCore(handle);

    /// <summary>Whether <paramref name="handle"/> names a live asset.</summary>
    public static bool Contains<T>(this ResMut<AssetManager> assets, Handle<T> handle)
        where T : RustObject, IRustAsset<T> => ContainsCore(handle);

    /// <summary>The asset stored under <paramref name="name"/>, if any.</summary>
    public static Handle<T>? Find<T>(this Res<AssetManager> assets, string name)
        where T : RustObject, IRustAsset<T> => LookupCore<T>("__asset_handle_by_name", name);

    /// <summary>The asset stored under <paramref name="name"/>, if any.</summary>
    public static Handle<T>? Find<T>(this ResMut<AssetManager> assets, string name)
        where T : RustObject, IRustAsset<T> => LookupCore<T>("__asset_handle_by_name", name);

    /// <summary>The asset keyed by <paramref name="guid"/> (32 hexadecimal digits), if any.</summary>
    public static Handle<T>? FindByGuid<T>(this Res<AssetManager> assets, string guid)
        where T : RustObject, IRustAsset<T> => LookupCore<T>("__asset_handle_by_guid", guid);

    /// <summary>The asset keyed by <paramref name="guid"/> (32 hexadecimal digits), if any.</summary>
    public static Handle<T>? FindByGuid<T>(this ResMut<AssetManager> assets, string guid)
        where T : RustObject, IRustAsset<T> => LookupCore<T>("__asset_handle_by_guid", guid);

    /// <summary>The guid of an asset, or <c>null</c> when it has none.</summary>
    public static string? GuidOf<T>(this Res<AssetManager> assets, Handle<T> handle)
        where T : RustObject, IRustAsset<T> => GuidOfCore(handle);

    /// <summary>The guid of an asset, or <c>null</c> when it has none.</summary>
    public static string? GuidOf<T>(this ResMut<AssetManager> assets, Handle<T> handle)
        where T : RustObject, IRustAsset<T> => GuidOfCore(handle);

    /// <summary>
    /// Import the source at <paramref name="path"/> (below <c>res</c>) through
    /// its <c>.meta</c> file, as Rust's <c>AssetManager::import</c> does.
    /// </summary>
    /// <param name="assets">The store.</param>
    /// <param name="path">The source file, below the project's <c>res</c>.</param>
    /// <param name="policy">Whether to write a missing <c>.meta</c> file.</param>
    /// <param name="initialSettingsJson">The type's import settings as JSON, used only when no <c>.meta</c> file exists; <c>null</c> for its defaults.</param>
    public static ImportedAsset<T> Import<T>(
        this ResMut<AssetManager> assets,
        string path,
        MetadataPolicy policy = MetadataPolicy.CreateIfMissing,
        string? initialSettingsJson = null)
        where T : RustObject, IRustImportedAsset<T>
    {
        var call = MirrorCall.Begin(T.RustTypeName, "__asset_import");
        call.PushResource<AssetManager>(QueryAccess.Write);
        call.PushString(path);
        call.Push((byte)policy);
        call.PushString(initialSettingsJson ?? string.Empty);
        call.Invoke();
        return ReadImport<T>(ref call);
    }

    /// <summary>
    /// Load the asset at <paramref name="path"/> (below <c>res</c>) and return
    /// its handle - the managed face of Rust's <c>World::load_asset</c>.
    /// </summary>
    /// <param name="assets">The store.</param>
    /// <param name="path">The source file, below the project's <c>res</c>.</param>
    /// <remarks>
    /// The path names a file, not a type: the caller's <typeparamref name="T"/>
    /// decides how the store files it, and the <c>.meta</c> beside the file
    /// decides how it is read. The guid that file holds - or the one written
    /// when it has none - is what lets the handle be saved as a reference, and a
    /// path that is already loaded hands back the handle it was loaded under
    /// instead of decoding again. This is <see cref="Import{T}"/> with the two
    /// things a plain load does not need dropped: the policy is always
    /// <see cref="MetadataPolicy.CreateIfMissing"/> and the flags are not
    /// returned.
    /// </remarks>
    /// <exception cref="EngineException">The file cannot be read, does not decode as <typeparamref name="T"/>, or names an asset of another type.</exception>
    public static Handle<T> Load<T>(this ResMut<AssetManager> assets, string path)
        where T : RustObject, IRustImportedAsset<T> => assets.Import<T>(path).Handle;

    /// <summary>
    /// Import the standalone file at <paramref name="path"/> (below <c>res</c>),
    /// whose header holds its guid, as Rust's <c>AssetManager::import_standalone</c> does.
    /// </summary>
    public static ImportedAsset<T> ImportStandalone<T>(this ResMut<AssetManager> assets, string path)
        where T : RustObject, IRustStandaloneAsset<T>
    {
        var call = MirrorCall.Begin(T.RustTypeName, "__asset_import_standalone");
        call.PushResource<AssetManager>(QueryAccess.Write);
        call.PushString(path);
        call.Invoke();
        return ReadImport<T>(ref call);
    }

    private static ImportedAsset<T> ReadImport<T>(ref MirrorCall call) =>
        new(call.Result<Handle<T>>(),
            GuidFrom(call.ResultString()),
            call.ResultAt<byte>(8) != 0,
            (MetadataSource)call.ResultAt<byte>(9));

    /// <summary>
    /// The string channel carries an empty guid for an asset that has none,
    /// so an empty string reads back as <c>null</c>.
    /// </summary>
    private static string? GuidFrom(string guid) => guid.Length == 0 ? null : guid;

    private static bool ContainsCore<T>(Handle<T> handle) where T : RustObject, IRustAsset<T>
    {
        var call = MirrorCall.Begin(T.RustTypeName, "__asset_contains");
        call.PushResource<AssetManager>(QueryAccess.Read);
        call.Push(handle);
        call.Invoke();
        return call.Result<bool>();
    }

    private static Handle<T>? LookupCore<T>(string operation, string key) where T : RustObject, IRustAsset<T>
    {
        var call = MirrorCall.Begin(T.RustTypeName, operation);
        call.PushResource<AssetManager>(QueryAccess.Read);
        call.PushString(key);
        call.Invoke();
        return call.ResultPresent() ? call.ResultAt<Handle<T>>(16) : null;
    }

    private static string? GuidOfCore<T>(Handle<T> handle) where T : RustObject, IRustAsset<T>
    {
        var call = MirrorCall.Begin(T.RustTypeName, "__asset_guid_of");
        call.PushResource<AssetManager>(QueryAccess.Read);
        call.Push(handle);
        call.Invoke();
        return GuidFrom(call.ResultString());
    }
}
