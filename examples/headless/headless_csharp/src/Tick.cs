// A pure simulation: tickers counting frames, reported through the engine log.
//
// Responsibilities
// - Define the ticker component and the report schedule resource.
// - Fill the world up to the target entity count.
// - Advance every ticker and log the world's total about once a second.
//
// Design
// The twin of `headless_rust`: `renderer: none` in `project_settings.yaml`
// keeps every renderer crate out of the load order, so a run opens no device.
// A managed startup runs once for the life of the host while the spawn system
// runs every frame, which is also what keeps a hot reload from duplicating the
// world - it creates only the entities that are missing.
//
// The report is on wall-clock time, not frames: a headless host has no vsync,
// so it runs the frame loop as fast as the machine allows and a frame-count
// interval would print thousands of lines a second.

using System.Diagnostics;
using System.Runtime.InteropServices;
// The ECS vocabulary (`Query`, `Commands`, the attribute types).
using TracyLive;

namespace HeadlessExample;

/// <summary>How many entities the world holds, and how often it reports.</summary>
internal static class HeadlessConfig
{
    /// <summary>Entities created at startup and kept for the run.</summary>
    internal const int TickerCount = 256;

    /// <summary>Seconds between two report lines.</summary>
    internal const float ReportIntervalSeconds = 1.0f;
}

/// <summary>One entity's frame counter.</summary>
/// <remarks>
/// A plain blittable struct: the runtime discovers it from its use in the
/// query below, and it survives a reload in the engine's own storage.
/// </remarks>
[StructLayout(LayoutKind.Sequential)]
public struct TickCounter
{
    /// <summary>Frames this entity has been stepped through.</summary>
    public ulong Ticks;
}

/// <summary>How far the report schedule has got.</summary>
[EcsResource]
[StructLayout(LayoutKind.Sequential)]
public struct ReportSchedule
{
    /// <summary>Seconds since the last report was logged.</summary>
    public float SecondsSinceReport;

    /// <summary>Stopwatch timestamp of the previous frame, or zero if never stamped.</summary>
    public long LastFrameTimestamp;
}

/// <summary>Creates the tickers that are missing, up to the target count.</summary>
public static class TickerSpawnSystem
{
    /// <summary>Counts the tickers and creates the ones the world lacks.</summary>
    [System]
    public static void Run(Query<Read<TickCounter>> query, Commands commands)
    {
        int existing = 0;
        foreach (var row in query.Rows())
        {
            _ = row.TickCounter;
            existing++;
        }

        for (int index = existing; index < HeadlessConfig.TickerCount; index++)
            commands.CreateEntity().With(new TickCounter { Ticks = 0 }).Build();
    }
}

/// <summary>Advances every ticker and reports the world's total.</summary>
public static class TickSystem
{
    /// <summary>Steps each ticker by one frame and logs on the report seconds.</summary>
    [System]
    public static void Run(Query<Write<TickCounter>> query, ResMut<ReportSchedule> report)
    {
        ulong total = 0;
        foreach (var row in query.Rows())
        {
            ref var ticker = ref row.TickCounter;
            ticker.Ticks += 1;
            total += ticker.Ticks;
        }

        ref ReportSchedule schedule = ref report.Value;
        long now = Stopwatch.GetTimestamp();
        if (schedule.LastFrameTimestamp != 0)
        {
            schedule.SecondsSinceReport += (float)Stopwatch.GetElapsedTime(
                schedule.LastFrameTimestamp, now).TotalSeconds;
        }
        schedule.LastFrameTimestamp = now;

        if (schedule.SecondsSinceReport >= HeadlessConfig.ReportIntervalSeconds)
        {
            schedule.SecondsSinceReport = 0.0f;
            Log.Info($"[headless] tickers={HeadlessConfig.TickerCount} total_ticks={total}");
        }
    }
}
