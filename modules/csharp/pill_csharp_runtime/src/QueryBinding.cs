// Compile-time query row binding.
//
// Responsibilities:
// - Resolve Read<T>/Write<T>/OptionalRead<T>/OptionalWrite<T> against the
//   query's own ordered term list instead of searching the joined columns.
// - Keep row construction to two words - a reference to the enumerator's
//   joined columns plus the row index - so a row costs two stores per row
//   instead of a copy of every column reference.
//
// Design:
// - QueryEnumerator<T1..T8> pads every closed query shape to eight slots with
//   None, so a single row type serves every arity.
// - Each accessor branch compares the requested component type with one slot's
//   IQueryTerm metadata. For a closed value-type query both sides are constant,
//   so the JIT folds every non-matching branch and resolves the matching one to
//   a direct reference: no per-row lookup, no per-row switch on access.
// - The row keeps the guardrails of the shape-erased QueryRow: required access
//   that the query did not declare throws, and only Write<T> stamps the row's
//   change tick.

using System.Diagnostics.CodeAnalysis;
using System.Diagnostics;
using System.Runtime.CompilerServices;

namespace TracyLive;

/// <summary>
/// Typed row view over one joined entity, bound to the query's own term list.
/// </summary>
/// <remarks>
/// Constructed by <see cref="QueryEnumerator{T1, T2, T3, T4, T5, T6, T7, T8}"/>
/// with a reference to the first of the enumerator's joined columns; the slot
/// for a term is that reference plus the term's constant index, so a row is
/// two words and constructing one per iteration costs two stores.
/// </remarks>
public readonly unsafe ref struct QueryRow<T1, T2, T3, T4, T5, T6, T7, T8>
    where T1 : IQueryTerm
    where T2 : IQueryTerm
    where T3 : IQueryTerm
    where T4 : IQueryTerm
    where T5 : IQueryTerm
    where T6 : IQueryTerm
    where T7 : IQueryTerm
    where T8 : IQueryTerm
{
    private readonly ref QueryColumn _columns;
    private readonly int _row;

    internal QueryRow(ref QueryColumn columns, int row)
    {
        _columns = ref columns;
        _row = row;
    }

    /// <summary>Borrow a required writable component declared by Write&lt;T&gt;.</summary>
    /// <remarks>
    /// AggressiveInlining keeps the accessor flat for callers that reach a row
    /// through a generated wrapper: the extra layer otherwise pushes the chain
    /// past the JIT's inline budget and costs several nanoseconds per row.
    /// </remarks>
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public ref T Write<T>() where T : unmanaged
    {
        if (Matches<T, T1>(QueryAccess.Write, optional: false)) return ref WriteFrom<T>(0);
        if (Matches<T, T2>(QueryAccess.Write, optional: false)) return ref WriteFrom<T>(1);
        if (Matches<T, T3>(QueryAccess.Write, optional: false)) return ref WriteFrom<T>(2);
        if (Matches<T, T4>(QueryAccess.Write, optional: false)) return ref WriteFrom<T>(3);
        if (Matches<T, T5>(QueryAccess.Write, optional: false)) return ref WriteFrom<T>(4);
        if (Matches<T, T6>(QueryAccess.Write, optional: false)) return ref WriteFrom<T>(5);
        if (Matches<T, T7>(QueryAccess.Write, optional: false)) return ref WriteFrom<T>(6);
        if (Matches<T, T8>(QueryAccess.Write, optional: false)) return ref WriteFrom<T>(7);
        throw Mismatch<T>(QueryAccess.Write, optional: false);
    }

    /// <summary>Borrow a required read-only component declared by Read&lt;T&gt;.</summary>
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public ref readonly T Read<T>() where T : unmanaged
    {
        if (Matches<T, T1>(QueryAccess.Read, optional: false)) return ref ReadFrom<T>(0);
        if (Matches<T, T2>(QueryAccess.Read, optional: false)) return ref ReadFrom<T>(1);
        if (Matches<T, T3>(QueryAccess.Read, optional: false)) return ref ReadFrom<T>(2);
        if (Matches<T, T4>(QueryAccess.Read, optional: false)) return ref ReadFrom<T>(3);
        if (Matches<T, T5>(QueryAccess.Read, optional: false)) return ref ReadFrom<T>(4);
        if (Matches<T, T6>(QueryAccess.Read, optional: false)) return ref ReadFrom<T>(5);
        if (Matches<T, T7>(QueryAccess.Read, optional: false)) return ref ReadFrom<T>(6);
        if (Matches<T, T8>(QueryAccess.Read, optional: false)) return ref ReadFrom<T>(7);
        throw Mismatch<T>(QueryAccess.Read, optional: false);
    }

    /// <summary>Borrow an optional writable component when it is present.</summary>
    public OptionalWriteRef<T> OptionalWrite<T>() where T : unmanaged
    {
        if (Matches<T, T1>(QueryAccess.Write, optional: true)) return OptionalWriteFrom<T>(0);
        if (Matches<T, T2>(QueryAccess.Write, optional: true)) return OptionalWriteFrom<T>(1);
        if (Matches<T, T3>(QueryAccess.Write, optional: true)) return OptionalWriteFrom<T>(2);
        if (Matches<T, T4>(QueryAccess.Write, optional: true)) return OptionalWriteFrom<T>(3);
        if (Matches<T, T5>(QueryAccess.Write, optional: true)) return OptionalWriteFrom<T>(4);
        if (Matches<T, T6>(QueryAccess.Write, optional: true)) return OptionalWriteFrom<T>(5);
        if (Matches<T, T7>(QueryAccess.Write, optional: true)) return OptionalWriteFrom<T>(6);
        if (Matches<T, T8>(QueryAccess.Write, optional: true)) return OptionalWriteFrom<T>(7);
        throw Mismatch<T>(QueryAccess.Write, optional: true);
    }

    /// <summary>Borrow an optional read-only component when it is present.</summary>
    public OptionalReadRef<T> OptionalRead<T>() where T : unmanaged
    {
        if (Matches<T, T1>(QueryAccess.Read, optional: true)) return OptionalReadFrom<T>(0);
        if (Matches<T, T2>(QueryAccess.Read, optional: true)) return OptionalReadFrom<T>(1);
        if (Matches<T, T3>(QueryAccess.Read, optional: true)) return OptionalReadFrom<T>(2);
        if (Matches<T, T4>(QueryAccess.Read, optional: true)) return OptionalReadFrom<T>(3);
        if (Matches<T, T5>(QueryAccess.Read, optional: true)) return OptionalReadFrom<T>(4);
        if (Matches<T, T6>(QueryAccess.Read, optional: true)) return OptionalReadFrom<T>(5);
        if (Matches<T, T7>(QueryAccess.Read, optional: true)) return OptionalReadFrom<T>(6);
        if (Matches<T, T8>(QueryAccess.Read, optional: true)) return OptionalReadFrom<T>(7);
        throw Mismatch<T>(QueryAccess.Read, optional: true);
    }

    /// <summary>Return the current entity declared by EntityTerm.</summary>
    public Entity Entity
    {
        get
        {
            if (T1.IsEntity) return ((Entity*)Slot(0).Data)[_row];
            if (T2.IsEntity) return ((Entity*)Slot(1).Data)[_row];
            if (T3.IsEntity) return ((Entity*)Slot(2).Data)[_row];
            if (T4.IsEntity) return ((Entity*)Slot(3).Data)[_row];
            if (T5.IsEntity) return ((Entity*)Slot(4).Data)[_row];
            if (T6.IsEntity) return ((Entity*)Slot(5).Data)[_row];
            if (T7.IsEntity) return ((Entity*)Slot(6).Data)[_row];
            if (T8.IsEntity) return ((Entity*)Slot(7).Data)[_row];
            throw new InvalidOperationException("This query does not declare EntityTerm.");
        }
    }

    /// <summary>Whether one slot declares exactly the requested access.</summary>
    private static bool Matches<T, TTerm>(QueryAccess access, bool optional)
        where T : unmanaged
        where TTerm : IQueryTerm
        => !TTerm.IsEntity
            && TTerm.Access == access
            && TTerm.Optional == optional
            && TTerm.DataType == typeof(T);

    /// <summary>Reference to one joined column; the index is a constant per branch.</summary>
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private ref QueryColumn Slot(int index) => ref Unsafe.Add(ref _columns, index);

    /// <summary>
    /// Reject a column whose chunk belongs to an earlier invocation.
    /// </summary>
    /// <remarks>
    /// The typed row is the hot path, so this is
    /// <see cref="ConditionalAttribute"/> on DEBUG and the call disappears
    /// entirely from a `Release` configuration - the same posture Unity takes
    /// with its job safety system. In a `Debug` configuration it turns a read
    /// of storage that has since moved into a named error.
    /// </remarks>
    [Conditional("DEBUG")]
    private static void ValidateScope(ref QueryColumn column)
    {
        if (column.Present)
            Engine.ValidateChunkScope(
                column.ScopeToken, column.Term.ComponentType?.FullName ?? "entity");
    }

    /// <summary>Read-only reference into a required slot's row.</summary>
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private ref readonly T ReadFrom<T>(int index) where T : unmanaged
    {
        ref QueryColumn column = ref Slot(index);
        ValidateScope(ref column);
        return ref ((T*)column.Data)[_row];
    }

    /// <summary>Writable reference into a required slot's row, marking it changed.</summary>
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private ref T WriteFrom<T>(int index) where T : unmanaged
    {
        ref QueryColumn column = ref Slot(index);
        ValidateScope(ref column);
        MarkChanged(ref column);
        return ref ((T*)column.Data)[_row];
    }

    /// <summary>Optional read-only borrow of one slot's row.</summary>
    private OptionalReadRef<T> OptionalReadFrom<T>(int index) where T : unmanaged
    {
        ref QueryColumn column = ref Slot(index);
        ValidateScope(ref column);
        return new OptionalReadRef<T>(column.Present ? &((T*)column.Data)[_row] : null);
    }

    /// <summary>Optional writable borrow of one slot's row.</summary>
    private OptionalWriteRef<T> OptionalWriteFrom<T>(int index) where T : unmanaged
    {
        ref QueryColumn column = ref Slot(index);
        ValidateScope(ref column);
        return new OptionalWriteRef<T>(
            column.Present ? &((T*)column.Data)[_row] : null,
            column.Present ? &((NativeComponentTicks*)column.Ticks)[_row] : null,
            column.ChangeTick);
    }

    /// <summary>Stamp the row's change tick.</summary>
    /// <remarks>
    /// The column argument is taken by reference and its type is read only on
    /// the failure path: loading <c>Term.ComponentType</c> eagerly costs two
    /// dependent loads on every row of a writable pass.
    /// </remarks>
    private void MarkChanged(ref QueryColumn column)
    {
        if (column.Ticks == IntPtr.Zero)
            ThrowMissingTicks(column.Term.ComponentType);
        ((NativeComponentTicks*)column.Ticks)[_row].Changed = column.ChangeTick;
    }

    /// <summary>The refusal raised for a writable column without change ticks.</summary>
    private static void ThrowMissingTicks(Type? componentType)
        => throw new InvalidOperationException(
            $"Writable component {componentType!.FullName} has no native change-tick column.");

    /// <summary>The refusal raised when the query did not declare the requested access.</summary>
    private static InvalidOperationException Mismatch<T>(QueryAccess access, bool optional)
        where T : unmanaged
        => new(
            $"This query does not declare {(optional ? "optional " : "")}" +
            $"{access.ToString().ToLowerInvariant()} access to {typeof(T).FullName}.");
}

/// <summary>
/// Typed iterator over a closed query shape; every slot below the arity holds
/// the term's marker and every unused slot holds <see cref="None"/>.
/// </summary>
/// <remarks>
/// Wraps the shape-erased <see cref="QueryEnumerator"/> for chunk joining and
/// exposes the current row as a
/// <see cref="QueryRow{T1, T2, T3, T4, T5, T6, T7, T8}"/> that references the
/// joined columns in place.
/// </remarks>
public ref struct QueryEnumerator<T1, T2, T3, T4, T5, T6, T7, T8>
    where T1 : IQueryTerm
    where T2 : IQueryTerm
    where T3 : IQueryTerm
    where T4 : IQueryTerm
    where T5 : IQueryTerm
    where T6 : IQueryTerm
    where T7 : IQueryTerm
    where T8 : IQueryTerm
{
    private QueryEnumerator _core;

    public QueryEnumerator(QueryDescriptor descriptor) => _core = new(descriptor);

    /// <summary>Advance within the current archetype or join the next one.</summary>
    public bool MoveNext() => _core.MoveNext();

    /// <summary>Typed view of the current row.</summary>
    /// <remarks>
    /// [UnscopedRef] is required to hand a reference to this enumerator's
    /// columns to the row. The contract is the foreach shape itself: the
    /// enumerator lives for the whole loop and the row is used inside it.
    /// </remarks>
    [UnscopedRef]
    public QueryRow<T1, T2, T3, T4, T5, T6, T7, T8> Current => new(ref _core.ColumnRef(0), _core.RowIndex);
}
