// Managed component-iteration benchmark.
//
// Responsibilities
// - Fill the world with a fixed number of entities, set by PILL_BENCH_ENTITIES.
// - Time three query shapes that mirror `pill_engine`'s `query_iteration`
//   Criterion target, and print one parseable line per shape.
//
// Design
// The unit reported is nanoseconds per entity, because that is what makes the
// managed number comparable to the Rust one: Criterion times a tight in-process
// loop, while a managed system runs once per frame across the interop boundary.
// Amortised over a thousand entities or more, that per-frame transition is
// negligible per entity; below roughly a hundred it would dominate and the
// comparison would stop meaning anything, which is why the sweep starts at 1k.
//
// Only frames where the world already holds the full entity count are sampled,
// so the spawn frames - which do real allocation work - never enter a
// measurement. A warmup is skipped on top of that, to let the JIT settle.
//
// Each measured loop folds the values it reads into a sink that is written back
// to a static. Without that the loop body has no observable effect and the JIT
// is free to delete it, which would measure an empty iteration.

using System.Diagnostics;

// The engine's managed surface - Query, Read, Write, Commands and the
// [System] attribute - lives in this namespace.
using TracyLive;

namespace ManagedTestProject;

// =============================================================================
// Configuration
// =============================================================================

/// <summary>How many entities to measure over, and how long to measure.</summary>
internal static class BenchConfig
{
    /// <summary>Default when PILL_BENCH_ENTITIES is unset or unparseable.</summary>
    internal const int DefaultEntities = 10_000;

    /// <summary>Whether the benchmark was asked to run.</summary>
    ///
    /// <remarks>
    /// The bridge and analyzer suites drive this project as a plain C#
    /// project and expect it to be quiet; only a run that names an entity
    /// count measures anything.
    /// </remarks>
    internal static bool Enabled =>
        !string.IsNullOrEmpty(Environment.GetEnvironmentVariable("PILL_BENCH_ENTITIES"));

    /// <summary>Entities created per frame while filling the world.</summary>
    ///
    /// <remarks>
    /// Batched rather than created in one frame: a hundred thousand commands
    /// queued at once is a large single allocation spike, and the frame that
    /// flushed it would be an outlier in every metric the host reports.
    /// </remarks>
    internal const int SpawnBatch = 2_000;

    /// <summary>Frames discarded after the world is full, before sampling.</summary>
    internal const int WarmupFrames = 120;

    /// <summary>Frames sampled once warmup is done.</summary>
    internal const int SampleFrames = 300;

    /// <summary>Target entity count, read once from the environment.</summary>
    internal static readonly int TargetEntities = ReadTargetEntities();

    private static int ReadTargetEntities()
    {
        string? raw = Environment.GetEnvironmentVariable("PILL_BENCH_ENTITIES");
        return int.TryParse(raw, out int parsed) && parsed > 0 ? parsed : DefaultEntities;
    }
}

// =============================================================================
// Sampling
// =============================================================================

/// <summary>Collects per-frame timings for one query shape and reports once.</summary>
///
/// <remarks>
/// Reports the median rather than the mean: one frame that lands beside a GC or
/// a scheduler hiccup would move a mean of three hundred samples noticeably,
/// and the Rust side this is compared against is a median too.
/// </remarks>
internal sealed class CaseRecorder
{
    private readonly string _name;
    private readonly double[] _samples = new double[BenchConfig.SampleFrames];
    private int _framesSeen;
    private int _sampleCount;
    private bool _reported;

    /// <summary>Ticks to nanoseconds, from the platform's timer frequency.</summary>
    private static readonly double NanosecondsPerTick = 1_000_000_000.0 / Stopwatch.Frequency;

    internal CaseRecorder(string name) => _name = name;

    /// <summary>Whether every case has printed its line.</summary>
    internal bool Reported => _reported;

    /// <summary>Record one frame, ignoring it unless the world is full.</summary>
    ///
    /// <param name="ticks">Stopwatch ticks the iteration took.</param>
    /// <param name="entities">Rows the iteration actually walked.</param>
    internal void Record(long ticks, int entities)
    {
        // A frame that saw fewer rows than the target is a spawn frame; it
        // measures a partly filled world and must not enter the sample.
        if (_reported || entities < BenchConfig.TargetEntities)
        {
            return;
        }
        _framesSeen++;
        if (_framesSeen <= BenchConfig.WarmupFrames)
        {
            return;
        }
        if (_sampleCount < _samples.Length)
        {
            _samples[_sampleCount++] = ticks * NanosecondsPerTick;
        }
        if (_sampleCount == _samples.Length)
        {
            Report(entities);
        }
    }

    /// <summary>Print the one parseable line this case exists to produce.</summary>
    private void Report(int entities)
    {
        _reported = true;
        double[] ordered = (double[])_samples.Clone();
        Array.Sort(ordered);
        double median = ordered.Length % 2 == 1
            ? ordered[ordered.Length / 2]
            : (ordered[(ordered.Length / 2) - 1] + ordered[ordered.Length / 2]) / 2.0;
        double perEntity = median / entities;
        Console.WriteLine(
            $"[csharp_bench] case={_name} entities={entities} " +
            $"median_ns={median:F1} per_entity_ns={perEntity:F4} samples={ordered.Length}");
    }
}

/// <summary>The three cases, and the sink that keeps their loops observable.</summary>
internal static class BenchState
{
    internal static readonly CaseRecorder Unfiltered = new("iter_unfiltered");
    internal static readonly CaseRecorder Mutable = new("iter_mutable");
    internal static readonly CaseRecorder MultiComponent = new("iter_multi_component");

    /// <summary>Written by every measured loop so none can be optimized away.</summary>
    internal static float Sink;

    /// <summary>Whether every case has reported, printed once by the last one.</summary>
    internal static bool AllReported =>
        Unfiltered.Reported && Mutable.Reported && MultiComponent.Reported;

    private static bool _doneAnnounced;

    /// <summary>Print the terminator the benchmark wrapper waits for.</summary>
    /// <remarks>
    /// The wrapper lives outside the tracked tree, so this line is outside the
    /// log-token contract `test_log_contract.py` enforces.
    /// </remarks>
    internal static void AnnounceDoneOnce()
    {
        if (_doneAnnounced || !AllReported)
        {
            return;
        }
        _doneAnnounced = true;
        Console.WriteLine("[csharp_bench] done");
    }
}

// =============================================================================
// Systems
// =============================================================================

/// <summary>Fills the world up to the target entity count, a batch per frame.</summary>
///
/// <remarks>
/// A startup cannot query, so the world is filled from an ordinary system that
/// creates only what is missing - the same shape `examples/project_cs` uses, and
/// what keeps a hot reload from duplicating the scene.
/// </remarks>
public static class BenchSpawnSystem
{
    [System]
    public static void Run(Query<Read<BenchPosition>> query, Commands commands)
    {
        if (!BenchConfig.Enabled)
        {
            return;
        }

        int existing = 0;
        foreach (var row in query.Rows())
        {
            _ = row.BenchPosition;
            existing++;
        }
        if (existing >= BenchConfig.TargetEntities)
        {
            return;
        }

        int missing = BenchConfig.TargetEntities - existing;
        int batch = missing < BenchConfig.SpawnBatch ? missing : BenchConfig.SpawnBatch;
        for (int index = 0; index < batch; index++)
        {
            // The same deterministic values `setup_world` uses in the Rust
            // bench, so neither side is iterating a distribution the other is
            // not: position from the ordinal, a constant velocity, health
            // cycling 0..99.
            int ordinal = existing + index;
            commands.CreateEntity()
                .With(new BenchPosition { X = ordinal, Y = ordinal * 2 })
                .With(new BenchVelocity { X = 0.1f, Y = 0.2f })
                .With(new BenchHealth { Value = ordinal % 100 })
                // Build() is what submits the queued creation. Without it the
                // builder is filled and then dropped, which allocates every
                // frame and creates nothing.
                .Build();
        }
    }
}

/// <summary>Reads two components per row. Mirrors Rust `query_iter_unfiltered`.</summary>
///
/// <remarks>
/// That Rust benchmark queries `(&Position, &Velocity)` and sums
/// `position.x + velocity.x`, despite what its name suggests - "unfiltered"
/// describes the absence of a `With`/`Without` filter, not a single term. This
/// reads the same two components and does the same arithmetic.
/// </remarks>
public static class BenchIterUnfilteredSystem
{
    [System]
    public static void Run(Query<Read<BenchPosition>, Read<BenchVelocity>> query)
    {
        if (!BenchConfig.Enabled)
        {
            return;
        }

        float sumX = 0.0f;
        int entities = 0;
        long start = Stopwatch.GetTimestamp();
        foreach (var row in query.Rows())
        {
            ref readonly var position = ref row.BenchPosition;
            ref readonly var velocity = ref row.BenchVelocity;
            sumX += position.X + velocity.X;
            entities++;
        }
        long elapsed = Stopwatch.GetTimestamp() - start;
        BenchState.Sink = sumX;
        BenchState.Unfiltered.Record(elapsed, entities);
    }
}

/// <summary>Writes one, reads one. Mirrors Rust `query_iter_mutable`.</summary>
///
/// <remarks>
/// The Rust body is `(&mut Position, &Velocity)` doing `position.x +=
/// velocity.x; position.y += velocity.y`, which adds change-detection tick
/// writes on top of the read cost. Same two terms and same two adds here.
/// </remarks>
public static class BenchIterMutableSystem
{
    [System]
    public static void Run(Query<Write<BenchPosition>, Read<BenchVelocity>> query)
    {
        if (!BenchConfig.Enabled)
        {
            return;
        }

        int entities = 0;
        long start = Stopwatch.GetTimestamp();
        foreach (var row in query.Rows())
        {
            ref var position = ref row.BenchPosition;
            ref readonly var velocity = ref row.BenchVelocity;
            position.X += velocity.X;
            position.Y += velocity.Y;
            entities++;
        }
        long elapsed = Stopwatch.GetTimestamp() - start;
        BenchState.Mutable.Record(elapsed, entities);
    }
}

/// <summary>Reads three components. Mirrors Rust `query_multi_component`.</summary>
///
/// <remarks>
/// The Rust body queries `(&Position, &Velocity, &Health)` and sums one field
/// from each, which is a three-column archetype join rather than the two-column
/// one above.
/// </remarks>
public static class BenchIterMultiComponentSystem
{
    [System]
    public static void Run(
        Query<Read<BenchPosition>, Read<BenchVelocity>, Read<BenchHealth>> query)
    {
        if (!BenchConfig.Enabled)
        {
            return;
        }

        if (!BenchConfig.Enabled)
        {
            return;
        }

        float sum = 0.0f;
        int entities = 0;
        long start = Stopwatch.GetTimestamp();
        foreach (var row in query.Rows())
        {
            ref readonly var position = ref row.BenchPosition;
            ref readonly var velocity = ref row.BenchVelocity;
            ref readonly var health = ref row.BenchHealth;
            sum += position.X + velocity.X + health.Value;
            entities++;
        }
        long elapsed = Stopwatch.GetTimestamp() - start;
        BenchState.Sink = sum;
        BenchState.MultiComponent.Record(elapsed, entities);
        BenchState.AnnounceDoneOnce();
    }
}
