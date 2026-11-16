// Managed parallel row execution: segments, slicing and dispatch.
//
// Responsibilities:
// - Snapshot a query's resolved chunks into ParallelSegment views worker
//   threads can read and write without touching the engine.
// - Split each chunk's rows into work items and run them through the host's
//   shared pool (Engine.ParallelFor), or inline when there is one item.
// - Refuse entry without a managed invocation scope, and refuse nested
//   passes, before any row is touched.
//
// Design:
// - Resolution stays on the frame thread: the enumerator walks chunks with
//   the same MoveToNextChunk the sequential path uses, validates scope tokens
//   once per chunk, then hands out raw pointers.
// - Workers run generated slice bodies over ParallelSegment helpers - pointer
//   arithmetic and tick writes only. An engine call from a worker is refused
//   by the thread-local scope check every engine entry point already performs.
// - Managed queries have chunk-level matching only, so segments need no row
//   masks. If per-row filters are ever added, segments must carry them.
// - Failure policy: a throwing row ends its own work item there; the other
//   work items still run (the join completes), and the first exception is
//   rethrown on the calling thread after the join - the same shape as the
//   engine's native parallel systems.
// - The default slice guidance mirrors the engine's (4096 rows, minimum 256)
//   so managed and native parallelism split work similarly.

namespace TracyLive;

/// <summary>Row-slicing constants for parallel passes.</summary>
public static class ParallelRows
{
    /// <summary>Rows per work item when the caller does not choose a size.</summary>
    public const int DefaultRowsPerSlice = 4096;

    /// <summary>Smallest accepted rows per slice; smaller requests are clamped.</summary>
    public const int MinimumRowsPerSlice = 256;

    /// <summary>
    /// Passes running on this thread; a body must not start another pass on
    /// the thread executing it.
    /// </summary>
    [ThreadStatic]
    private static int _passDepth;

    /// <summary>
    /// Enter a parallel pass: refuse nesting and refuse a missing managed
    /// invocation before any row is touched.
    /// </summary>
    /// <exception cref="InvalidOperationException">
    /// A pass is already running on this thread, or no managed system is
    /// scheduled on it.
    /// </exception>
    internal static void EnterPass()
    {
        if (_passDepth > 0)
        {
            throw new InvalidOperationException(
                "ForEachParallel cannot start another parallel pass from inside one; " +
                "the row body must not dispatch.");
        }
        if (Engine.CurrentScopeToken == 0)
        {
            throw new InvalidOperationException(
                "ForEachParallel requires a scheduled [System] on the calling " +
                "thread; no managed invocation is active.");
        }
        _passDepth++;
    }

    /// <summary>Leave the pass started by <see cref="EnterPass"/>.</summary>
    internal static void ExitPass() => _passDepth--;

    /// <summary>
    /// Run the pass: a single work item runs inline on the calling thread
    /// (no pool hop), anything else is dispatched and joined.
    /// </summary>
    internal static void RunSlices(ParallelWorkItem[] items, ParallelSliceRunner runner)
    {
        if (items.Length == 1)
        {
            ParallelWorkItem item = items[0];
            runner(item.Segment, item.Start, item.Count);
            return;
        }
        ParallelDispatch.Run(items.Length, new ParallelInvocation(items, runner));
    }
}

/// <summary>
/// One chunk's joined columns, snapshotted for worker threads.
/// </summary>
/// <remarks>
/// Generator-facing: only the runtime creates segments, and only generated
/// slice loops consume them. All pointers were resolved and validated on the
/// frame thread; the helpers here do plain pointer arithmetic, a row cannot
/// move while its pass is running, and simultaneous access to DISTINCT rows
/// is the supported pattern - the same disjointness the engine's Queries
/// rely on. Nothing here may be retained across frames or replays.
/// </remarks>
public sealed unsafe class ParallelSegment
{
    private readonly IntPtr[] _data;
    private readonly IntPtr[] _ticks;
    private readonly bool[] _present;
    private readonly uint[] _changeTick;

    internal ParallelSegment(
        IntPtr[] data, IntPtr[] ticks, bool[] present, uint[] changeTick, int rowCount)
    {
        _data = data;
        _ticks = ticks;
        _present = present;
        _changeTick = changeTick;
        RowCount = rowCount;
    }

    /// <summary>Rows the chunk held when it was resolved.</summary>
    public int RowCount { get; }

    /// <summary>Whether the slot's column exists in this chunk's archetype.</summary>
    public bool IsPresent(int slot) => _present[slot];

    /// <summary>Borrow the row of a required term at <paramref name="slot"/>.</summary>
    public ref T Get<T>(int slot, int row) where T : unmanaged =>
        ref ((T*)_data[slot])[row];

    /// <summary>Borrow the row of an optional writable term when present.</summary>
    public OptionalWriteRef<T> GetOptionalWrite<T>(int slot, int row) where T : unmanaged =>
        new(
            _present[slot] ? &((T*)_data[slot])[row] : null,
            _present[slot] ? &((NativeComponentTicks*)_ticks[slot])[row] : null,
            _changeTick[slot]);

    /// <summary>Borrow the row of an optional read-only term when present.</summary>
    public OptionalReadRef<T> GetOptionalRead<T>(int slot, int row) where T : unmanaged =>
        new(_present[slot] ? &((T*)_data[slot])[row] : null);

    /// <summary>Read the entity of the slot's entity term.</summary>
    public Entity GetEntity(int slot, int row) => ((Entity*)_data[slot])[row];

    /// <summary>
    /// Stamp the row's change tick, saying a slice wrote through a required
    /// writable term.
    /// </summary>
    /// <remarks>
    /// Called once per written row by the generated slice loop - the same
    /// stamp `QueryRow.Write&lt;T&gt;` applies on the sequential path.
    /// </remarks>
    public void MarkChanged(int slot, int row)
    {
        if (_ticks[slot] == IntPtr.Zero)
        {
            throw new InvalidOperationException(
                $"Writable term at slot {slot} has no native change-tick column.");
        }
        ((NativeComponentTicks*)_ticks[slot])[row].Changed = _changeTick[slot];
    }
}

/// <summary>One slice of one chunk: rows <c>[Start, Start + Count)</c>.</summary>
public delegate void ParallelSliceRunner(ParallelSegment segment, int start, int count);

/// <summary>One dispatched work item: a row range inside one segment.</summary>
internal readonly struct ParallelWorkItem
{
    internal readonly ParallelSegment Segment;
    internal readonly int Start;
    internal readonly int Count;

    internal ParallelWorkItem(ParallelSegment segment, int start, int count) =>
        (Segment, Start, Count) = (segment, start, count);
}

/// <summary>Maps dispatched item indices onto slices.</summary>
/// <remarks>
/// Per-item exception capture is <see cref="ParallelDispatch"/>'s: it records
/// the first failure on whichever thread ran it and rethrows it on the calling
/// thread after the join, so this type only maps indices to slices.
/// </remarks>
internal sealed class ParallelInvocation : IParallelWork
{
    private readonly ParallelWorkItem[] _items;
    private readonly ParallelSliceRunner _runner;

    internal ParallelInvocation(ParallelWorkItem[] items, ParallelSliceRunner runner) =>
        (_items, _runner) = (items, runner);

    /// <summary>Run the work item with this index.</summary>
    public void Invoke(int index)
    {
        ParallelWorkItem item = _items[index];
        _runner(item.Segment, item.Start, item.Count);
    }
}
