// Managed-side driver for the host's parallel dispatch slot.
//
// Responsibilities:
// - Run one parallel call's work items across the host's shared Rayon pool
//   through EngineApi.ParallelFor, joining before returning.
// - Catch each work item's exception on its worker and rethrow the first one
//   on the calling thread.
//
// Design:
// - The per-call state is the GCHandle target: callbacks receive its address
//   back as an opaque pointer, so two systems dispatching concurrently never
//   share state and nothing static outlives a call.
// - Nothing in a work item may touch engine APIs: workers carry no managed
//   invocation scope, and every engine entry point refuses them on its own.
//   The calling thread is checked for a scope before dispatching.

using System.Runtime.CompilerServices;
using System.Runtime.ExceptionServices;
using System.Runtime.InteropServices;

namespace TracyLive;

/// <summary>
/// One parallel call's work: <see cref="Invoke"/> runs a single work item.
/// </summary>
internal interface IParallelWork
{
    /// <summary>Run the work item with this index.</summary>
    void Invoke(int index);
}

/// <summary>Drives <see cref="EngineApi.ParallelFor"/> for one call.</summary>
internal static unsafe class ParallelDispatch
{
    /// <summary>Every index ran.</summary>
    internal const byte Ok = 0;

    /// <summary>No managed system is scheduled on the calling thread.</summary>
    internal const byte NoScope = 3;

    /// <summary>The call came from inside a parallel callback.</summary>
    internal const byte Nested = 4;

    /// <summary>The host's dispatch failed internally, or predates the slot.</summary>
    internal const byte DispatchFailed = 5;

    /// <summary>
    /// Run <paramref name="work"/> for every index in <c>0..count</c> on the
    /// host's shared pool; returns after every item finished.
    /// </summary>
    /// <exception cref="InvalidOperationException">
    /// The call is refused: no scheduled system on this thread, or a nested
    /// dispatch from inside a parallel callback.
    /// </exception>
    internal static void Run(int count, IParallelWork work)
    {
        if (count <= 0)
            return;

        var call = new Invocation(work);
        GCHandle handle = GCHandle.Alloc(call);
        try
        {
            byte status = Engine.ParallelFor(
                &Trampoline, GCHandle.ToIntPtr(handle), (uint)count);
            switch (status)
            {
                case Ok:
                    break;
                case NoScope:
                    throw new InvalidOperationException(
                        "ForEachParallel requires a scheduled [System] on the calling " +
                        "thread; no managed invocation is active.");
                case Nested:
                    throw new InvalidOperationException(
                        "ForEachParallel cannot start another parallel pass from inside " +
                        "one; the row body must not dispatch.");
                default:
                    throw new InvalidOperationException(
                        $"the host's parallel dispatch failed (status {status}).");
            }
            call.FirstError?.Throw();
        }
        finally
        {
            handle.Free();
        }
    }

    /// <summary>One call's state; the trampoline resolves it from its handle.</summary>
    private sealed class Invocation : IParallelWork
    {
        private readonly IParallelWork _work;

        internal Invocation(IParallelWork work) => _work = work;

        /// <summary>The first exception any work item threw, if one did.</summary>
        internal ExceptionDispatchInfo? FirstError;

        public void Invoke(int index)
        {
            try
            {
                _work.Invoke(index);
            }
            catch (Exception error)
            {
                // Keep the first failure and let the remaining items finish;
                // the calling thread rethrows it after the join.
                Interlocked.CompareExchange(
                    ref FirstError, ExceptionDispatchInfo.Capture(error), null);
            }
        }
    }

    /// <summary>
    /// The <c>[UnmanagedCallersOnly]</c> entry the host's pool threads call
    /// once per work item.
    /// </summary>
    /// <remarks>
    /// A managed exception must never cross this frame: it runs outside any
    /// managed invocation, and there is no unwind path back into native code.
    /// </remarks>
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void Trampoline(nint state, uint index)
    {
        try
        {
            var call = (Invocation)GCHandle.FromIntPtr(state).Target!;
            call.Invoke((int)index);
        }
        catch
        {
            // Only reachable if the per-item catch above itself failed (for
            // example a corrupted state handle). Swallowing keeps the process
            // alive rather than aborting inside native code; the calling
            // thread still reports a dispatch failure through its own path.
        }
    }
}
