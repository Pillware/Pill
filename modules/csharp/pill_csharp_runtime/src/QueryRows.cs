// The stack-only row views and the composable query arities built on them.
//
// Responsibilities:
// - Declares the plain data a row exposes: the entity handle, its stable id,
//   one joined column and the joined column set.
// - Holds the ref-struct accessors a system reads a row through, and the
//   shared enumerator every arity wraps.
// - Declares the eight closed query arities discovery registers.
//
// Design:
// - Rows and enumerators are ref structs over native column pointers, so a
//   query allocates nothing per frame and no pointer escapes the call.
// - One shared enumerator does the joining; each arity only supplies its term
//   list and its typed row type, so the join is written once.

using System.Diagnostics.CodeAnalysis;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace TracyLive;

// =============================================================================
// Stack-only Row Views
// =============================================================================

/// <summary>ABI-compatible managed entity handle.</summary>
[StructLayout(LayoutKind.Sequential)]
public readonly struct Entity
{
    public readonly ulong Id;
    public readonly uint Generation;

    public Entity(ulong id, uint generation) => (Id, Generation) = (id, generation);
}

/// <summary>
/// The stable 128-bit identity of one entity, as saved data names it.
/// </summary>
/// <remarks>
/// A handle names a slot the world recycles; this id names the entity. Read it
/// with <see cref="Engine.TryGetStableId"/> and keep it for anything that has
/// to find the entity again later. The id is random, so it says nothing about
/// spawn order, and it stops resolving when its entity is destroyed.
///
/// Two halves rather than a 128-bit value: no C ABI in the engine table carries
/// one. The textual form is the hex string Rust's `EntityId` prints - high half
/// first - which is what a saved scene records.
/// </remarks>
public readonly struct StableEntityId : IEquatable<StableEntityId>
{
    public readonly ulong Low;
    public readonly ulong High;

    public StableEntityId(ulong low, ulong high) => (Low, High) = (low, high);

    public bool Equals(StableEntityId other) => Low == other.Low && High == other.High;

    public override bool Equals(object? obj) => obj is StableEntityId other && Equals(other);

    public override int GetHashCode() => HashCode.Combine(Low, High);

    public static bool operator ==(StableEntityId left, StableEntityId right) => left.Equals(right);

    public static bool operator !=(StableEntityId left, StableEntityId right) => !left.Equals(right);

    /// <summary>The 32-digit hex form saved data records.</summary>
    public override string ToString() => $"{High:x16}{Low:x16}";
}

/// <summary>One archetype of the native world, by the two halves of its id.</summary>
internal readonly record struct ArchetypeKey(ulong Low, ulong High);

/// <summary>One component (or the entity column) of a query enumeration.</summary>
internal struct QueryColumn
{
    internal QueryTermDescriptor Term;
    /// <summary>Invocation the chunk behind this column was issued to.</summary>
    internal uint ScopeToken;
    internal IntPtr Data;
    internal int Length;
    internal bool Present;
    internal IntPtr Ticks;
    internal uint ChangeTick;
}

/// <summary>The joined columns of one query enumeration, stored inline.</summary>
/// <remarks>Eight slots: the widest query shape.</remarks>
[InlineArray(8)]
internal struct QueryColumns
{
    private QueryColumn _first;
}

/// <summary>Optional writable reference valid only for the current row.</summary>
public readonly unsafe ref struct OptionalWriteRef<T> where T : unmanaged
{
    private readonly T* _value;
    private readonly NativeComponentTicks* _ticks;
    private readonly uint _changeTick;
    internal OptionalWriteRef(T* value, NativeComponentTicks* ticks, uint changeTick)
    {
        _value = value;
        _ticks = ticks;
        _changeTick = changeTick;
    }
    public bool HasValue => _value != null;
    public ref T Value
    {
        get
        {
            if (_value == null)
                throw new InvalidOperationException($"Optional component {typeof(T).FullName} is absent.");
            if (_ticks == null)
                throw new InvalidOperationException(
                    $"Writable component {typeof(T).FullName} has no native change-tick column.");
            _ticks->Changed = _changeTick;
            return ref *_value;
        }
    }
}

/// <summary>Optional read-only reference valid only for the current row.</summary>
public readonly unsafe ref struct OptionalReadRef<T> where T : unmanaged
{
    private readonly T* _value;
    internal OptionalReadRef(T* value) => _value = value;
    public bool HasValue => _value != null;
    public ref readonly T Value
    {
        get
        {
            if (_value == null)
                throw new InvalidOperationException($"Optional component {typeof(T).FullName} is absent.");
            return ref *_value;
        }
    }
}

/// <summary>Stack-only view over one joined ECS row.</summary>
public readonly unsafe ref struct QueryRow
{
    private readonly QueryColumn _c0, _c1, _c2, _c3, _c4, _c5, _c6, _c7;
    private readonly int _count;
    private readonly int _row;

    internal QueryRow(
        QueryColumn c0, QueryColumn c1, QueryColumn c2, QueryColumn c3,
        QueryColumn c4, QueryColumn c5, QueryColumn c6, QueryColumn c7,
        int count, int row) =>
        (_c0, _c1, _c2, _c3, _c4, _c5, _c6, _c7, _count, _row) =
        (c0, c1, c2, c3, c4, c5, c6, c7, count, row);

    /// <summary>Borrow a required writable component declared by Write&lt;T&gt;.</summary>
    public ref T Write<T>() where T : unmanaged
    {
        QueryColumn column = Find<T>(QueryAccess.Write, optional: false);
        MarkChanged(column);
        return ref ((T*)column.Data)[_row];
    }

    /// <summary>Borrow a required read-only component declared by Read&lt;T&gt;.</summary>
    public ref readonly T Read<T>() where T : unmanaged
    {
        QueryColumn column = Find<T>(QueryAccess.Read, optional: false);
        return ref ((T*)column.Data)[_row];
    }

    /// <summary>Borrow an optional writable component when it is present.</summary>
    public OptionalWriteRef<T> OptionalWrite<T>() where T : unmanaged
    {
        QueryColumn column = Find<T>(QueryAccess.Write, optional: true);
        return new OptionalWriteRef<T>(
            column.Present ? &((T*)column.Data)[_row] : null,
            column.Present ? &((NativeComponentTicks*)column.Ticks)[_row] : null,
            column.ChangeTick);
    }

    /// <summary>Borrow an optional read-only component when it is present.</summary>
    public OptionalReadRef<T> OptionalRead<T>() where T : unmanaged
    {
        QueryColumn column = Find<T>(QueryAccess.Read, optional: true);
        return new OptionalReadRef<T>(column.Present ? &((T*)column.Data)[_row] : null);
    }

    /// <summary>Return the current entity declared by EntityTerm.</summary>
    public Entity Entity
    {
        get
        {
            for (var i = 0; i < _count; i++)
            {
                QueryColumn column = Column(i);
                if (column.Term.IsEntity)
                    return ((Entity*)column.Data)[_row];
            }
            throw new InvalidOperationException("This query does not declare EntityTerm.");
        }
    }

    private QueryColumn Find<T>(QueryAccess access, bool optional) where T : unmanaged
    {
        StableComponentId key = ComponentTypeMetadata<T>.StableId;
        for (var i = 0; i < _count; i++)
        {
            QueryColumn column = Column(i);
            if (!column.Term.IsEntity && column.Term.ComponentKey == key.Low &&
                column.Term.ComponentKeyHigh == key.High &&
                column.Term.Access == access && column.Term.Optional == optional)
            {
                // Every typed accessor resolves its column here, so one check
                // covers Read, Write, OptionalRead and OptionalWrite. Compiled
                // out of `Release` configurations.
                if (column.Present)
                    Engine.ValidateChunkScope(column.ScopeToken, typeof(T).FullName ?? typeof(T).Name);
                return column;
            }
        }
        throw new InvalidOperationException(
            $"This query does not declare {(optional ? "optional " : "")}{access.ToString().ToLowerInvariant()} " +
            $"access to {typeof(T).FullName}.");
    }

    private void MarkChanged(QueryColumn column)
    {
        if (column.Ticks == IntPtr.Zero)
            throw new InvalidOperationException(
                $"Writable component {column.Term.ComponentType!.FullName} has no native change-tick column.");
        ((NativeComponentTicks*)column.Ticks)[_row].Changed = column.ChangeTick;
    }

    private QueryColumn Column(int index)
    {
        switch (index)
        {
            case 0: return _c0;
            case 1: return _c1;
            case 2: return _c2;
            case 3: return _c3;
            case 4: return _c4;
            case 5: return _c5;
            case 6: return _c6;
            case 7: return _c7;
            default: throw new ArgumentOutOfRangeException(nameof(index));
        }
    }
}

// =============================================================================
// Shared Query Iterator
// =============================================================================

/// <summary>Stack-only iterator shared by every composable query shape.</summary>
public ref struct QueryEnumerator
{
    private readonly QueryTermDescriptor[] _terms;
    // The joined columns live in one contiguous buffer so a typed row can
    // address every slot through a single reference to element zero. The
    // buffer is inline in the enumerator, not a heap array: a query is
    // enumerated every frame, and an array here cost every system one
    // 536-byte allocation per query per frame.
    private QueryColumns _columns;
    private readonly int _driver;
    private uint _nextDriverChunk;
    private int _row;
    private int _length;

    internal QueryEnumerator(QueryDescriptor descriptor)
    {
        _terms = descriptor.TermArray;
        _columns = default;
        _driver = FindDriver(_terms);
        _nextDriverChunk = 0;
        _row = -1;
        _length = 0;
    }

    /// <summary>Return the current stack-only joined row.</summary>
    public QueryRow Current => new(
        _columns[0],
        _columns[1],
        _columns[2],
        _columns[3],
        _columns[4],
        _columns[5],
        _columns[6],
        _columns[7],
        _terms.Length,
        _row);

    /// <summary>Advance one row inside the joined chunk.</summary>
    /// <remarks>
    /// This is the per-row hot path, so it is deliberately one branch: the
    /// typed enumerator's MoveNext inlines it, and everything that happens
    /// once per chunk lives in <see cref="MoveToNextChunk"/>. Merging the two
    /// makes MoveNext big enough that the JIT stops inlining it, which
    /// measurably moves the per-row cost from ~0.4 ns to ~3 ns.
    /// </remarks>
    internal bool TryAdvanceRow() => ++_row < _length;

    /// <summary>Join the next archetype chunk; runs once per chunk, not per row.</summary>
    internal bool MoveToNextChunk()
    {
        while (TryLoadDriver(_nextDriverChunk++, out var archetype, out var driverChunk))
        {
            _length = checked((int)driverChunk.Length);
            if (_length == 0)
                continue;

            var matched = true;
            for (var i = 0; i < _terms.Length; i++)
            {
                QueryTermDescriptor term = _terms[i];
                NativeComponentChunk chunk;
                bool present;
                if (i == _driver)
                {
                    chunk = driverChunk;
                    present = true;
                }
                else
                {
                    present = Engine.TryGetArchetypeChunk(
                        term, archetype.Low, archetype.High, out chunk);
                    if (!present && !term.Optional)
                    {
                        matched = false;
                        break;
                    }
                }

                if (present && chunk.Length != driverChunk.Length)
                    throw new InvalidOperationException("ECS component chunk lengths are inconsistent.");
                // Entity rows arrive in the const `Entities` slot, which the
                // native ABI leaves the writable `Data` slot null for; the
                // joined column keeps one row-base field either way.
                IntPtr rows = term.IsEntity ? chunk.Entities : chunk.Data;
                SetColumn(i, new QueryColumn
                {
                    Term = term,
                    Data = rows,
                    Length = present ? checked((int)chunk.Length) : 0,
                    Present = present,
                    Ticks = present ? chunk.Ticks : IntPtr.Zero,
                    ChangeTick = present ? chunk.ChangeTick : 0,
                    ScopeToken = present ? chunk.ScopeToken : 0,
                });
            }

            if (!matched)
                continue;
            _row = 0;
            return true;
        }
        return false;
    }

    /// <summary>Advance within the current archetype or join the next one.</summary>
    public bool MoveNext()
    {
        if (TryAdvanceRow())
            return true;
        return MoveToNextChunk();
    }

    private static int FindDriver(QueryTermDescriptor[] terms)
    {
        for (var i = 0; i < terms.Length; i++)
            if (!terms[i].IsEntity && !terms[i].Optional)
                return i;
        for (var i = 0; i < terms.Length; i++)
            if (terms[i].IsEntity)
                return i;
        return -1; // An implicit entity driver makes optional-only queries useful.
    }

    private bool TryLoadDriver(
        uint index, out ArchetypeKey archetype, out NativeComponentChunk chunk)
    {
        bool found = _driver >= 0 && !_terms[_driver].IsEntity
            ? Engine.TryGetChunk(_terms[_driver], index, out chunk)
            : Engine.TryGetEntityChunk(index, out chunk);
        archetype = found
            ? new ArchetypeKey(chunk.ArchetypeLow, chunk.ArchetypeHigh)
            : default;
        return found;
    }

    private void SetColumn(int index, QueryColumn value) => _columns[index] = value;

    /// <summary>Reference to one joined column, for the typed row wrappers.</summary>
    /// <remarks>
    /// Ref-returning an element of this enumerator's own column buffer needs
    /// the explicit [UnscopedRef] opt-in.
    /// The contract is narrow: only the typed enumerator calls this, and the
    /// row it feeds never outlives the enumerator that owns the columns.
    /// </remarks>
    [UnscopedRef]
    internal ref QueryColumn ColumnRef(int index) => ref _columns[index];

    /// <summary>Index of the current row inside the joined chunk.</summary>
    internal int RowIndex => _row;

    /// <summary>
    /// Run one slice per contiguous row range of every matched chunk across
    /// the host's shared pool, joining before returning.
    /// </summary>
    /// <remarks>
    /// Resolution - walking chunks, joining columns, validating scope tokens -
    /// runs here on the calling thread exactly as the sequential walk does.
    /// Workers then see only the snapshot (<see cref="ParallelSegment"/>), and
    /// a slice body must not call engine APIs: workers carry no managed
    /// invocation scope and every entry point refuses them.
    ///
    /// A single work item runs inline on this thread (no FFI, no pool hop).
    /// <paramref name="rowsPerSlice"/> is clamped to
    /// <see cref="ParallelRows.MinimumRowsPerSlice"/>. The enumerator is
    /// consumed, like the sequential walk: create it fresh per call.
    /// </remarks>
    /// <exception cref="InvalidOperationException">
    /// The calling thread has no managed invocation scope, or a parallel pass
    /// is already running on it.
    /// </exception>
    public void ForEachSliceParallel(
        ParallelSliceRunner runner, int rowsPerSlice = ParallelRows.DefaultRowsPerSlice)
    {
        ArgumentNullException.ThrowIfNull(runner);
        ParallelRows.EnterPass();
        try
        {
            if (rowsPerSlice < ParallelRows.MinimumRowsPerSlice)
                rowsPerSlice = ParallelRows.MinimumRowsPerSlice;

            var items = new List<ParallelWorkItem>();
            while (MoveToNextChunk())
            {
                ParallelSegment segment = SnapshotSegment();
                for (var start = 0; start < segment.RowCount; start += rowsPerSlice)
                {
                    items.Add(new ParallelWorkItem(
                        segment, start, Math.Min(rowsPerSlice, segment.RowCount - start)));
                }
            }
            if (items.Count == 0)
                return;
            ParallelRows.RunSlices(items.ToArray(), runner);
        }
        finally
        {
            ParallelRows.ExitPass();
        }
    }

    /// <summary>Snapshot the current joined chunk into a worker-safe segment.</summary>
    private ParallelSegment SnapshotSegment()
    {
        var data = new IntPtr[8];
        var ticks = new IntPtr[8];
        var present = new bool[8];
        var changeTick = new uint[8];
        for (var i = 0; i < _terms.Length; i++)
        {
            QueryColumn column = _columns[i];
            data[i] = column.Data;
            ticks[i] = column.Ticks;
            present[i] = column.Present;
            changeTick[i] = column.ChangeTick;
            if (column.Present)
            {
                // The check QueryRow's typed accessors make lazily, done once
                // here: workers inherit pointers that were validated already.
                Engine.ValidateChunkScope(
                    column.ScopeToken,
                    column.Term.ComponentType?.FullName ?? "component");
            }
        }
        return new ParallelSegment(data, ticks, present, changeTick, _length);
    }
}

// =============================================================================
// Composable Query Arities
// =============================================================================

/// <summary>Shared behavior for all composable query arities.</summary>
public abstract class QueryBase : IQueryDescriptor
{
    protected QueryBase(QueryDescriptor descriptor) => Descriptor = descriptor;
    public QueryDescriptor Descriptor { get; }
    public QueryEnumerator GetEnumerator() => new(Descriptor);
}

public sealed class Query<T1> : QueryBase where T1 : IQueryTerm
{
    private static readonly QueryDescriptor Cached = new(typeof(T1));
    public Query() : base(Cached) { }
    public new QueryEnumerator<T1, None, None, None, None, None, None, None> GetEnumerator() => new(Descriptor);
}

public sealed class Query<T1, T2> : QueryBase where T1 : IQueryTerm where T2 : IQueryTerm
{
    private static readonly QueryDescriptor Cached = new(typeof(T1), typeof(T2));
    public Query() : base(Cached) { }
    public new QueryEnumerator<T1, T2, None, None, None, None, None, None> GetEnumerator() => new(Descriptor);
}

public sealed class Query<T1, T2, T3> : QueryBase where T1 : IQueryTerm where T2 : IQueryTerm where T3 : IQueryTerm
{
    private static readonly QueryDescriptor Cached = new(typeof(T1), typeof(T2), typeof(T3));
    public Query() : base(Cached) { }
    public new QueryEnumerator<T1, T2, T3, None, None, None, None, None> GetEnumerator() => new(Descriptor);
}

public sealed class Query<T1, T2, T3, T4> : QueryBase where T1 : IQueryTerm where T2 : IQueryTerm where T3 : IQueryTerm where T4 : IQueryTerm
{
    private static readonly QueryDescriptor Cached = new(typeof(T1), typeof(T2), typeof(T3), typeof(T4));
    public Query() : base(Cached) { }
    public new QueryEnumerator<T1, T2, T3, T4, None, None, None, None> GetEnumerator() => new(Descriptor);
}

public sealed class Query<T1, T2, T3, T4, T5> : QueryBase where T1 : IQueryTerm where T2 : IQueryTerm where T3 : IQueryTerm where T4 : IQueryTerm where T5 : IQueryTerm
{
    private static readonly QueryDescriptor Cached = new(typeof(T1), typeof(T2), typeof(T3), typeof(T4), typeof(T5));
    public Query() : base(Cached) { }
    public new QueryEnumerator<T1, T2, T3, T4, T5, None, None, None> GetEnumerator() => new(Descriptor);
}

public sealed class Query<T1, T2, T3, T4, T5, T6> : QueryBase where T1 : IQueryTerm where T2 : IQueryTerm where T3 : IQueryTerm where T4 : IQueryTerm where T5 : IQueryTerm where T6 : IQueryTerm
{
    private static readonly QueryDescriptor Cached = new(typeof(T1), typeof(T2), typeof(T3), typeof(T4), typeof(T5), typeof(T6));
    public Query() : base(Cached) { }
    public new QueryEnumerator<T1, T2, T3, T4, T5, T6, None, None> GetEnumerator() => new(Descriptor);
}

public sealed class Query<T1, T2, T3, T4, T5, T6, T7> : QueryBase where T1 : IQueryTerm where T2 : IQueryTerm where T3 : IQueryTerm where T4 : IQueryTerm where T5 : IQueryTerm where T6 : IQueryTerm where T7 : IQueryTerm
{
    private static readonly QueryDescriptor Cached = new(typeof(T1), typeof(T2), typeof(T3), typeof(T4), typeof(T5), typeof(T6), typeof(T7));
    public Query() : base(Cached) { }
    public new QueryEnumerator<T1, T2, T3, T4, T5, T6, T7, None> GetEnumerator() => new(Descriptor);
}

public sealed class Query<T1, T2, T3, T4, T5, T6, T7, T8> : QueryBase where T1 : IQueryTerm where T2 : IQueryTerm where T3 : IQueryTerm where T4 : IQueryTerm where T5 : IQueryTerm where T6 : IQueryTerm where T7 : IQueryTerm where T8 : IQueryTerm
{
    private static readonly QueryDescriptor Cached = new(typeof(T1), typeof(T2), typeof(T3), typeof(T4), typeof(T5), typeof(T6), typeof(T7), typeof(T8));
    public Query() : base(Cached) { }
    public new QueryEnumerator<T1, T2, T3, T4, T5, T6, T7, T8> GetEnumerator() => new(Descriptor);
}
