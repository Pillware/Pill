// The mirrored-method table: how managed code calls a Rust method.
//
// Responsibilities:
// - Registers the trampoline addresses the host publishes for each module
//   generation and resolves them into typed delegates.
// - Carries the opaque handles a mirrored call exchanges: a row address, a
//   trampoline address and a borrowed buffer view.
// - Holds the per-component metadata a query row resolves once.
//
// Design:
// - The table is one immutable dictionary swapped whole, so a refresh never
//   clears it under a call in flight; the delegate cache is keyed by the bind
//   generation, so a republish invalidates it.
// - Addresses are opaque structs with internal constructors, so a script
//   cannot invent one; that removes the accidental path, not the deliberate
//   one.

using System.Collections.Concurrent;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace TracyLive;

// =============================================================================
// Mirrored Rust Methods
// =============================================================================

/// <summary>
/// The address of a live value the runtime handed out.
/// </summary>
/// <remarks>
/// A script cannot construct one: the field and the constructor are both
/// internal, so the only way to hold a row address is to have been given it by
/// <see cref="MirrorMethods.AddressOf{T}(ref T)"/>. That does not make the
/// address safe to *keep* - it points into a native archetype column that a
/// structural change or a module reload can move - it makes it impossible to
/// invent, which removes the accidental path without pretending to remove the
/// deliberate one.
/// </remarks>
public readonly struct RowPointer
{
    internal readonly IntPtr Address;
    internal RowPointer(IntPtr address) => Address = address;
}

/// <summary>
/// A mirrored-method trampoline address the host published for the module
/// generation currently loaded.
/// </summary>
/// <remarks>
/// Opaque for the same reason as <see cref="RowPointer"/>, and with a shorter
/// life: a module reload republishes the table at fresh addresses, so a
/// trampoline resolved before a reload is stale after it.
/// </remarks>
public readonly struct MirrorTrampoline
{
    internal readonly IntPtr Address;
    internal MirrorTrampoline(IntPtr address) => Address = address;
}

/// <summary>
/// A borrowed window over a native buffer, exactly as a Rust trampoline
/// reported it.
/// </summary>
/// <remarks>
/// The pointer and the length always travel together and can only be produced
/// by a trampoline call, so managed code cannot pair an address with a length
/// of its own choosing. The lease ends when the call that produced it returns:
/// anything that resizes or replaces the container on the Rust side
/// invalidates it.
/// </remarks>
public readonly struct BufferView
{
    internal readonly IntPtr Data;

    /// <summary>Number of elements the trampoline reported.</summary>
    public readonly int Length;

    internal BufferView(IntPtr data, IntPtr length)
    {
        Data = data;
        Length = checked((int)length);
    }
}

/// <summary>
/// Registry of the Rust methods mirrored into generated C# structs.
///
/// Populated once at startup from the host's native table (see
/// <see cref="Engine.Bind"/>). Generated mirror methods resolve a typed
/// delegate over each method's exported C-ABI trampoline and invoke it with
/// the receiver's live address, so a call boxes and pins nothing. Generated
/// heap-field accessors skip the delegate layer entirely: they resolve the
/// trampoline's raw address once per host bind (guarded by
/// <see cref="Generation"/>) and call it through the <c>Invoke*</c> helpers
/// below, which use C-ABI function pointers with no marshalling between
/// managed and native code. Both paths stay callable from code compiled
/// without the `unsafe` keyword, so the reloadable project assembly needs no
/// <c>AllowUnsafeBlocks</c> - which guards that keyword, not memory safety:
/// the addresses these methods exchange are raw, and the project's code is
/// trusted the same way any other code the user builds is.
/// </summary>
public static class MirrorMethods
{
    // Concurrent because `Resolve` is reachable from scheduled systems, and the
    // scheduler runs batches of disjoint systems on parallel threads: a plain
    // `Dictionary` mutated from two of them at once is a lost update at best
    // and a corrupted bucket chain at worst. Every operation here is a single
    // `TryGetValue` or indexer write, so the concurrent forms cover it without
    // a lock.
    private static readonly ConcurrentDictionary<(string TypeName, string Method), (int Generation, Delegate Delegate)> Cache = new();

    // One immutable table, swapped whole. A refresh builds the next one beside
    // the live table and publishes it in one exchange, so a call in flight reads
    // either the previous bind's complete table or the new bind's - never the
    // cleared state in between.
    private static Dictionary<(string TypeName, string Method), IntPtr> _addresses = new();
    private static int _generation;

    /// <summary>
    /// Counts host binds. Generated accessors cache trampoline addresses and
    /// re-resolve whenever this value changes: reloading an extension
    /// maps it at a fresh base address, so the addresses from the previous
    /// bind dangle from then on.
    /// </summary>
    public static int Generation => global::System.Threading.Volatile.Read(ref _generation);

    /// <summary>An empty table for a refresh to fill before it is published.</summary>
    internal static Dictionary<(string TypeName, string Method), IntPtr> NewTable()
        => new();

    /// <summary>
    /// Replace the address table in one step and invalidate every cached
    /// delegate, which is what a host bind means.
    /// </summary>
    internal static void Publish(Dictionary<(string TypeName, string Method), IntPtr> table)
    {
        global::System.Threading.Interlocked.Exchange(ref _addresses, table);
        Cache.Clear();
        global::System.Threading.Volatile.Write(ref _generation, _generation + 1);
    }

    /// <summary>Drop all registered methods; called when the host rebinds.</summary>
    internal static void Reset() => Publish(NewTable());

    /// <summary>Register one mirrored method's trampoline address in a table.</summary>
    internal static void Register(
        Dictionary<(string TypeName, string Method), IntPtr> table,
        string typeName,
        string method,
        IntPtr address)
        => table[(typeName, method)] = address;

    /// <summary>
    /// Register one mirrored method into the live table, publishing a copy of
    /// it so a concurrent reader never sees the write half done.
    /// </summary>
    internal static void Register(string typeName, string method, IntPtr address)
    {
        Dictionary<(string TypeName, string Method), IntPtr> table =
            new(global::System.Threading.Volatile.Read(ref _addresses));
        Register(table, typeName, method, address);
        global::System.Threading.Interlocked.Exchange(ref _addresses, table);
    }

    /// <summary>
    /// Drop the resolved delegates without touching the addresses or the
    /// generation.
    /// </summary>
    /// <remarks>
    /// A collectible context unloading takes its delegate types with it, so the
    /// cache that holds them must go - but the trampoline addresses the host
    /// published are still the ones the next context calls, so the unload must
    /// not clear them.
    /// </remarks>
    internal static void DropDelegateCache() => Cache.Clear();

    /// <summary>
    /// Address of a mirrored Rust method's C-ABI trampoline, for generated
    /// accessors that call it through the <c>Invoke*</c> helpers.
    /// </summary>
    /// <exception cref="InvalidOperationException">
    /// No trampoline is registered for the method - most often because the
    /// module was statically linked rather than loaded as a dynamic library.
    /// </exception>
    public static MirrorTrampoline Address(string typeName, string method)
    {
        if (!global::System.Threading.Volatile.Read(ref _addresses)
                .TryGetValue((typeName, method), out IntPtr address))
        {
            throw new InvalidOperationException(
                $"No mirrored Rust method {typeName}::{method} is registered. " +
                "Mirrored methods are only available when the declaring module is " +
                "loaded as a dynamic library by the developer host.");
        }
        return new MirrorTrampoline(address);
    }

    /// <summary>
    /// Resolve (and cache) the typed delegate that calls a mirrored Rust
    /// method's C-ABI trampoline.
    /// </summary>
    /// <typeparam name="T">The generated delegate type for the method.</typeparam>
    public static T Resolve<T>(string typeName, string method) where T : Delegate
    {
        // A host republish since the last check invalidates every address this
        // cache holds; refreshing first means the generation read below is the
        // one the entries are compared against.
        Engine.RefreshMirrorMethodsIfStale();
        (string, string) key = (typeName, method);
        int generation = Generation;
        if (Cache.TryGetValue(key, out (int Generation, Delegate Delegate) cached)
            && cached.Generation == generation)
        {
            return (T)cached.Delegate;
        }
        T created = (T)Marshal.GetDelegateForFunctionPointer(
            Address(typeName, method).Address, typeof(T));
        Cache[key] = (generation, created);
        return created;
    }

    // One helper per heap-field trampoline shape. The address comes from the
    // host's registration table and the caller passes the component row's
    // live address, so the call reads and writes the real container: one
    // boundary call per member use, no delegate stub, no marshalling.

    /// <summary>
    /// Call a `(row, out data, out length)` trampoline - a `Vec`/`String` view
    /// or the count view of a `Vec&lt;String&gt;`.
    /// </summary>
    public static unsafe byte InvokeView(
        MirrorTrampoline address, RowPointer row, out BufferView view)
    {
        byte status = ((delegate* unmanaged[Cdecl]<IntPtr, out IntPtr, out IntPtr, byte>)
            address.Address)(row.Address, out IntPtr data, out IntPtr length);
        view = new BufferView(data, length);
        return status;
    }

    /// <summary>Call a `(row, count)` trampoline that resizes a container field.</summary>
    public static unsafe byte InvokeResize(MirrorTrampoline address, RowPointer row, int count)
        => ((delegate* unmanaged[Cdecl]<IntPtr, IntPtr, byte>)address.Address)(
            row.Address, (IntPtr)count);

    /// <summary>
    /// Call a `(row, index, out data, out length)` trampoline - one element of
    /// a `Vec&lt;String&gt;` (status: 0 reachable, 1 dead row, 2 out of range).
    /// </summary>
    public static unsafe byte InvokeItem(
        MirrorTrampoline address, RowPointer row, int index, out BufferView view)
    {
        byte status = ((delegate* unmanaged[Cdecl]<IntPtr, IntPtr, out IntPtr, out IntPtr, byte>)
            address.Address)(row.Address, (IntPtr)index, out IntPtr data, out IntPtr length);
        view = new BufferView(data, length);
        return status;
    }

    /// <summary>
    /// Call a `(row, index, utf8, length)` trampoline that replaces one
    /// element of a `Vec&lt;String&gt;` (status: 0 ok, 1 dead row, 2 out of
    /// range, 3 invalid UTF-8).
    /// </summary>
    /// <remarks>
    /// The payload crosses as a span rather than an address and a length, so
    /// the caller cannot pair a pointer with a length of its own choosing, and
    /// generated accessors no longer pin a byte array by hand.
    /// </remarks>
    public static unsafe byte InvokeSetItem(
        MirrorTrampoline address, RowPointer row, int index, ReadOnlySpan<byte> utf8)
    {
        fixed (byte* payload = utf8)
            return ((delegate* unmanaged[Cdecl]<IntPtr, IntPtr, IntPtr, IntPtr, byte>)
                address.Address)(
                    row.Address, (IntPtr)index, (IntPtr)payload, (IntPtr)utf8.Length);
    }

    /// <summary>
    /// Call a `(row, utf8, length)` trampoline - writing a `String` field or
    /// appending to a `Vec&lt;String&gt;`.
    /// </summary>
    public static unsafe byte InvokeUtf8Write(
        MirrorTrampoline address, RowPointer row, ReadOnlySpan<byte> utf8)
    {
        fixed (byte* payload = utf8)
            return ((delegate* unmanaged[Cdecl]<IntPtr, IntPtr, IntPtr, byte>)address.Address)(
                row.Address, (IntPtr)payload, (IntPtr)utf8.Length);
    }

    /// <summary>
    /// Address of a live value passed by reference, for generated heap-field
    /// accessors and mirrored method receivers.
    ///
    /// The address points at the storage the reference names - for a component
    /// row, the native column slot - so a trampoline reached through it reads
    /// and writes the real container rather than a copy of its header.
    /// </summary>
    public static unsafe RowPointer AddressOf<T>(ref T value) where T : unmanaged
        => new RowPointer(
            (IntPtr)global::System.Runtime.CompilerServices.Unsafe.AsPointer(ref value));
}

/// <summary>
/// Span helpers for generated heap-field accessors.
///
/// A generated accessor hands these methods the address and element count a
/// Rust trampoline (or a mirrored `DynamicBuffer` handle) reported, so managed
/// code iterates a component's live buffer with no per-element boundary call.
/// An empty buffer carries a null element pointer, which a zero count turns
/// into the empty span rather than an invalid one. A span is a lease, not
/// ownership: resizing or replacing the container on the Rust side invalidates
/// it.
/// </summary>
public static class ComponentViews
{
    /// <summary>Writable span over the buffer a trampoline reported.</summary>
    public static unsafe Span<T> AsSpan<T>(BufferView view) where T : unmanaged
        => view.Length == 0 ? Span<T>.Empty : new Span<T>((void*)view.Data, view.Length);

    /// <summary>Read-only span over the buffer a trampoline reported.</summary>
    public static unsafe ReadOnlySpan<T> AsReadOnlySpan<T>(BufferView view) where T : unmanaged
        => view.Length == 0 ? ReadOnlySpan<T>.Empty : new ReadOnlySpan<T>((void*)view.Data, view.Length);

    /// <summary>Decode a UTF-8 buffer a trampoline reported into a string.</summary>
    /// <remarks>
    /// The copy is the point: the view is a lease that ends when the call that
    /// produced it returns, and a string is the only form of the data that can
    /// outlive it.
    /// </remarks>
    public static string ToUtf8String(BufferView view)
        => view.Length == 0
            ? string.Empty
            : Marshal.PtrToStringUTF8(view.Data, view.Length) ?? string.Empty;

    /// <summary>
    /// Span over an engine-owned dynamic buffer whose `(ptr, len, cap)` handle
    /// is mirrored into a component row.
    /// </summary>
    /// <remarks>
    /// Separate from <see cref="AsSpan{T}(BufferView)"/> because this handle is
    /// mirrored *data* rather than the result of a trampoline call: the pointer
    /// is read out of the row itself, so there is no call to hand back a
    /// <see cref="BufferView"/>. Generated accessors are the only intended
    /// caller, and they read the pointer from private fields of the generated
    /// mirror struct.
    ///
    /// This is the one raw entry point left in the public surface. It survives
    /// because the alternative would be to expose the mirror's handle fields,
    /// which is strictly worse. The address-stability contract that makes a
    /// retained handle legal at all is the buffer allocator's, not this
    /// method's: resizing is a native call that ends every outstanding lease.
    /// </remarks>
    public static unsafe Span<T> OverDynamicBuffer<T>(UIntPtr data, int count) where T : unmanaged
        => count == 0 ? Span<T>.Empty : new Span<T>((void*)data, count);

    /// <summary>Read-only twin of <see cref="OverDynamicBuffer{T}"/>.</summary>
    public static unsafe ReadOnlySpan<T> OverDynamicBufferReadOnly<T>(UIntPtr data, int count)
        where T : unmanaged
        => count == 0 ? ReadOnlySpan<T>.Empty : new ReadOnlySpan<T>((void*)data, count);
}

/// <summary>
/// Per-closed-component metadata initialized once outside row iteration.
/// Generic static initialization avoids repeated Type.FullName lookup,
/// UTF-8 allocation, hashing, and Marshal.SizeOf calls in managed hot paths.
/// </summary>
internal static class ComponentTypeMetadata<T> where T : unmanaged
{
    internal static readonly StableComponentId StableId =
        Engine.ComponentStableId(typeof(T));

    /// <summary>
    /// Native size of one component row, taken from the runtime itself.
    /// </summary>
    /// <remarks>
    /// <c>Unsafe.SizeOf&lt;T&gt;()</c> is the number a row write actually
    /// touches: <c>((T*)column.Data)[row] = value</c> moves exactly this many
    /// bytes. <c>NativeLayout</c> predicts the same number by walking fields,
    /// because NativeAOT denies it <c>Marshal.SizeOf</c>, and the manifest the
    /// host registers a column stride from is built from that prediction.
    ///
    /// If the two ever disagree the column stride is wrong and every row write
    /// runs off the end of its slot, so the prediction is checked against the
    /// authority here - the one place a real <c>T</c> is in scope, which keeps
    /// the check working under NativeAOT where <c>MakeGenericMethod</c> over a
    /// value type is not available. Every component reachable from a query row
    /// or a deferred command resolves this class, so the check covers the
    /// whole reachable set without a registry to keep in sync.
    /// </remarks>
    internal static readonly int Size = SizeCheckedAgainstManifest();

    /// <summary>Return the runtime size after proving the manifest agrees.</summary>
    private static int SizeCheckedAgainstManifest()
    {
        int actual = Unsafe.SizeOf<T>();
        int describedByManifest = TracyLive.Loader.NativeLayout.SizeOf(typeof(T));
        if (describedByManifest != actual)
            throw new InvalidOperationException(
                $"Component {typeof(T).FullName} is {actual} bytes to the runtime but the " +
                $"component manifest describes {describedByManifest}. The native column would " +
                "be strided by the manifest size, so every row write would address the wrong " +
                "bytes. Remove StructLayout Pack, or declare an explicit Size that matches.");
        return actual;
    }
}
