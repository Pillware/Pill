// Managed mirror of examples/project_rs/src/lib.rs.
//
// Responsibilities
// - Runs the ball physics: five balls bouncing inside a fixed box.
// - Drives the project's single spline from the ball centres and keeps one
//   sample dot on the curve every SplineSampleStep of `t`.
// - Fills the world with those entities, up to a target count, the way the
//   Rust init does.
//
// Design
// The scene, the constants, the per-frame math and the shapes of the systems
// are the Rust project's - including the three-query `spline_path` pass - so
// both projects run the same demo against the same world. Three things cannot
// be expressed the same way on the managed side, and each is named where it
// matters:
// 1. `SimulationTime` is an engine resource here, as it is in Rust, reached
//    through `ResMut<T>`. The ball system stamps it before reading it rather
//    than a separate time system writing it, because the scheduler orders
//    systems by access and does not promise which of two disjoint ones runs
//    first - stamping where the value is used needs no such promise.
// 2. A startup cannot query, so filling the world up to a target count - which
//    the Rust init checks by counting entities - is done by the spawn systems:
//    they run every frame and create only what is missing, which is also what
//    keeps a hot reload from duplicating the scene.
// 3. Registration needs no code here: the loader discovers the attributed
//    systems and registers the project's components from the managed manifest,
//    where the Rust init calls `register_system` and `register_component`
//    itself.

using System.Diagnostics;
using System.Numerics;
using System.Runtime.InteropServices;

// The renderer's components, generated from their Rust registration.
using pill_master_renderer;

using static TracyLive.ProjectConstants;
using Spline = pill_spline.Spline;
using Vector3f = pill_spline.Vector3f;

namespace TracyLive;

// =============================================================================
// Constants
// =============================================================================

/// <summary>Scene constants, the mirror of the Rust project's constants block.</summary>
internal static class ProjectConstants
{
    internal const float FixedDeltaTime = 1.0f / 60.0f;
    internal const float Gravity = 800.0f;
    internal const float BounceVelocityY = -800.0f;
    internal const float BounceVelocityX = 350.0f;
    internal const float Restitution = 0.1f;

    /// <summary>Upward speed restored when a floor bounce would decay to rest.</summary>
    internal const float MinimumBounceVelocityY = 500.0f;

    internal const float FloorY = 580.0f;
    internal const float CeilingY = 20.0f;
    internal const float LeftWall = 20.0f;
    internal const float RightWall = 780.0f;

    /// <summary>Number of balls, and with it the number of spline control points.</summary>
    internal const int BallCount = 5;

    /// <summary>Curve parameter between two neighbouring sample dots.</summary>
    internal const float SplineSampleStep = 0.05f;

    /// <summary>Sample dots on the curve: one every step, both endpoints included.</summary>
    internal const int SplineSampleCount = 21;

    /// <summary>Edge length of a sample dot, in pixels.</summary>
    internal const float SplineSampleDotSize = 6.0f;
}

// =============================================================================
// Simulation time
// =============================================================================

/// <summary>
/// Wall-clock delta between frames, the managed twin of the Rust project's
/// `SimulationTime` resource.
/// </summary>
/// <remarks>
/// A resource rather than a static, which is what makes it survive a hot
/// reload: a static lives in the collectible load context the reload replaces,
/// so the clock would restart on every code edit, while the resource's bytes
/// are the engine's and outlive the assembly.
///
/// The timestamp is carried in the resource for the same reason. A zero one
/// means "never stamped", which is what a freshly created resource reads as,
/// so the first frame after a cold start falls back to the fixed delta instead
/// of measuring against the epoch.
/// </remarks>
[EcsResource]
[StructLayout(LayoutKind.Sequential)]
public struct SimulationTime
{
    /// <summary>Seconds the previous frame took.</summary>
    public float DeltaSeconds;

    /// <summary>Stopwatch timestamp of the previous stamp, or zero if never.</summary>
    public long LastFrameTimestamp;
}

/// <summary>Stamping helper for <see cref="SimulationTime"/>.</summary>
internal static class SimulationClock
{
    /// <summary>
    /// Stamps the time elapsed since the previous frame into the resource.
    /// </summary>
    /// <remarks>
    /// Clamped because a breakpoint or a slow reload can stretch a single frame
    /// far enough to throw a ball straight through a wall.
    /// </remarks>
    internal static void Stamp(ref SimulationTime time)
    {
        long now = Stopwatch.GetTimestamp();
        time.DeltaSeconds = time.LastFrameTimestamp == 0
            ? FixedDeltaTime
            : Math.Clamp(
                (float)Stopwatch.GetElapsedTime(time.LastFrameTimestamp, now).TotalSeconds,
                0.0f,
                0.1f);
        time.LastFrameTimestamp = now;
    }
}

// =============================================================================
// Ball physics
// =============================================================================

/// <summary>Steps every ball by the frame delta and copies its state into its mesh.</summary>
public static class BallPhysicsSystem
{
    /// <summary>Advances one ball by its own delta and bounces it off the box.</summary>
    public static void Simulate(ref PhysicsState state)
    {
        if (state.Active == 0)
            return;

        float delta = Math.Clamp(state.DeltaTime, 0.0f, 0.1f);
        state.VelocityY += Gravity * delta;
        state.PositionX += state.VelocityX * delta;
        state.PositionY += state.VelocityY * delta;

        if (state.PositionY + state.Radius >= FloorY)
        {
            state.PositionY = FloorY - state.Radius;
            state.VelocityY = -MathF.Abs(state.VelocityY) * Restitution;
            // Restitution alone decays each bounce towards rest; restore a
            // minimum upward speed so balls bounce forever.
            if (MathF.Abs(state.VelocityY) < MinimumBounceVelocityY)
                state.VelocityY = -MinimumBounceVelocityY;
        }
        if (state.PositionY - state.Radius <= CeilingY)
        {
            state.PositionY = CeilingY + state.Radius;
            state.VelocityY = MathF.Abs(state.VelocityY) * Restitution;
        }
        if (state.PositionX - state.Radius <= LeftWall)
        {
            state.PositionX = LeftWall + state.Radius;
            state.VelocityX = MathF.Abs(state.VelocityX) * Restitution;
        }
        if (state.PositionX + state.Radius >= RightWall)
        {
            state.PositionX = RightWall - state.Radius;
            state.VelocityX = -MathF.Abs(state.VelocityX) * Restitution;
        }
    }

    [System]
    public static void Run(
        ResMut<SimulationTime> time,
        Query<Write<PhysicsState>, Write<TransformComponent>> query)
    {
        ref SimulationTime simulation = ref time.Value;
        SimulationClock.Stamp(ref simulation);
        float deltaSeconds = simulation.DeltaSeconds;

        foreach (var row in query.Rows())
        {
            ref var physics = ref row.PhysicsState;
            ref var transform = ref row.TransformComponent;

            physics.DeltaTime = deltaSeconds;
            Simulate(ref physics);

            // Physics coordinates are the ball's centre in the project's
            // 800x600 pixel space; the scene places the mesh from them, and
            // the renderer draws its quad from the entity's transform.
            transform = TransformComponent.At((physics.PositionX - 400.0f) / 80.0f, (300.0f - physics.PositionY) / 80.0f, 0.0f, physics.Radius / 80.0f);
        }
    }
}

// =============================================================================
// Spline path
// =============================================================================

/// <summary>Shared helpers for the spline pass: the point value and the seed curve.</summary>
/// <remarks>
/// The module's mirror declares `Vector3f` with typed `X`/`Y`/`Z` members, so
/// the seed spline is written with ordinary assignments over a cast of its
/// control point storage; live rows are edited through the module's
/// `SetControlPointLocation` instead. The curve math itself is the module's,
/// reached through the mirrored `GetLocationX`/`GetLocationY` getters. The
/// seed spline is built from the balls' spawn positions for the dots' first
/// frame.
/// </remarks>
internal static class SplinePath
{
    /// <summary>One control point, the shape glam's `Vector3f::new` takes.</summary>
    internal static Vector3f Vec3(float x, float y, float z) => new() { X = x, Y = y, Z = z };

    /// <summary>
    /// A spline through the balls' spawn positions, for seeding the dots
    /// before the first snapshot writes the live curve.
    /// </summary>
    internal static Spline CreateSeedSpline()
    {
        var spline = new Spline();
        // The raw span is 200 bytes wide: the cast keeps the 16 control
        // points at the front and drops the count/elo tail.
        Span<Vector3f> points = MemoryMarshal.Cast<byte, Vector3f>(spline.Raw);
        for (int index = 0; index < BallCount; index++)
        {
            PhysicsState ball = WorldSetup.BallSpawnState(index);
            points[index] = Vec3(ball.PositionX, ball.PositionY, 0.0f);
        }
        spline.ControlPointCount = BallCount;
        // The module's `from_points` builds on `Spline::default()`, so a
        // freshly seeded spline carries that default rather than zero.
        spline.Elo = 30.0f;
        return spline;
    }
}

/// <summary>Rebuilds the spline from the ball centres and walks the sample dots along the result.</summary>
/// <remarks>
/// The managed counterpart of the Rust `spline_path_system`: three query
/// parameters in one system, each iterating independently. The physics system
/// touches the same components in the same frame, and the scheduler is free to
/// batch it either side of this system; a centre can therefore be one frame
/// old by the time it becomes a control point, which is invisible at 60 Hz and
/// keeps the spline out of the physics step.
/// </remarks>
public static class SplinePathSystem
{
    [System]
    public static void Run(
        Query<Read<PhysicsState>> balls,
        Query<Write<Spline>> splines,
        Query<Write<SplineSample>, Write<TransformComponent>> samples)
    {
        // Step 1: collect the ball centres in the order the control points
        // take. Iteration walks the ball archetype row by row and the balls
        // are created in index order, so the i-th centre seen belongs to the
        // i-th ball. The points are staged because the two query iterations
        // cannot interleave: the centres are gathered first, then handed to
        // the spline in Step 2.
        Span<Vector3f> controlPoints = stackalloc Vector3f[BallCount];
        int count = 0;
        foreach (var row in balls.Rows())
        {
            if (count == BallCount)
                break;

            ref readonly var ball = ref row.PhysicsState;
            controlPoints[count] = SplinePath.Vec3(ball.PositionX, ball.PositionY, 0.0f);
            count++;
        }

        // Step 2: publish the points, then place the dots on the curve they
        // describe. The spline keeps its own copy of the centres, so the balls
        // need no relationship to it and stay free to keep moving.
        foreach (var row in splines.Rows())
        {
            ref var spline = ref row.Spline;
            for (int index = 0; index < count; index++)
            {
                // The module writes the point in place: the mirrored call
                // receives the row reference's live storage. The demo never
                // exceeds MAX_CONTROL_POINTS, so the capacity result is
                // deliberately discarded.
                spline.SetControlPointLocation(
                    (uint)index,
                    controlPoints[index].X,
                    controlPoints[index].Y);
            }
            spline.ControlPointCount = (uint)count;

            foreach (var sampleRow in samples.Rows())
            {
                ref var sample = ref sampleRow.SplineSample;
                float x = spline.GetLocationX(sample.T);
                float y = spline.GetLocationY(sample.T);

                // Samples are curve points and the dot is centred on them;
                // the transform places the mesh's quad from that centre.
                sampleRow.TransformComponent = TransformComponent.At((x - 400.0f) / 80.0f, (300.0f - y) / 80.0f, 0.0f, SplineSampleDotSize / 160.0f);
            }
        }
    }
}

// =============================================================================
// World setup
// =============================================================================

/// <summary>Spawn layout, shared by the systems that fill the world.</summary>
internal static class WorldSetup
{
    /// <summary>Physics state for the <paramref name="index"/>-th ball in the spawn sequence.</summary>
    /// <remarks>
    /// Balls line up across the play area with alternating travel direction and
    /// a slightly different launch speed each, so they spread out over the box
    /// instead of crossing it as one block.
    /// </remarks>
    internal static PhysicsState BallSpawnState(int index) => new()
    {
        DeltaTime = FixedDeltaTime,
        PositionX = 90.0f + index * 150.0f,
        PositionY = 120.0f,
        VelocityX = index % 2 == 0 ? BounceVelocityX : -BounceVelocityX,
        VelocityY = BounceVelocityY - index * 25.0f,
        Radius = 10.0f + index % 4 * 2.0f,
        Active = 1,
    };
}

/// <summary>Creates the balls that are missing, up to <see cref="BallCount"/>.</summary>
/// <remarks>
/// The Rust init fills the world up to a target count instead of spawning a
/// fresh set on every rebuild, because hot reload preserves the entities that
/// already exist. A managed startup cannot query, so that fill runs here: once
/// per frame, creating only what the world is missing.
/// </remarks>
public static class BallSpawnSystem
{
    [System]
    public static void Run(Query<Read<PhysicsState>> query, Commands commands)
    {
        int existing = 0;
        foreach (var row in query.Rows())
        {
            _ = row.PhysicsState;
            existing++;
        }

        for (int index = existing; index < BallCount; index++)
        {
            PhysicsState ball = WorldSetup.BallSpawnState(index);
            commands.CreateEntity()
                .With(ball)
                .With(TransformComponent.At((ball.PositionX - 400.0f) / 80.0f, (300.0f - ball.PositionY) / 80.0f, 0.0f, ball.Radius / 80.0f))
                .With(MeshRendererComponent.None)
                .Build();
        }
    }
}

/// <summary>Creates the project's spline when the world has none.</summary>
/// <remarks>
/// Seeded with the balls' spawn positions, so even the first frame draws a
/// curve through them instead of every sample dot stacked on the origin. In a
/// managed run the world usually already has a spline: `Spline` resolves to
/// the component type the `pill_spline` module registered, and the module
/// seeds a demo path of its own on load.
/// </remarks>
public static class SplineSpawnSystem
{
    [System]
    public static void Run(Query<Read<Spline>> query, Commands commands)
    {
        foreach (var row in query.Rows())
        {
            _ = row.Spline;
            return;
        }

        commands.CreateEntity().With(SplinePath.CreateSeedSpline()).Build();
    }
}

/// <summary>Creates the sample dots that are missing, up to the target count.</summary>
/// <remarks>
/// Each new dot is seeded on the spawn curve; from the next frame on, the
/// sample system places it on the live one. Only the position is ever
/// rewritten, so the inspector can retune the dots' size and colour while the
/// scene runs.
/// </remarks>
public static class SplineSampleSpawnSystem
{
    [System]
    public static void Run(Query<Read<SplineSample>> query, Commands commands)
    {
        int existing = 0;
        foreach (var row in query.Rows())
        {
            _ = row.SplineSample;
            existing++;
        }
        if (existing >= SplineSampleCount)
            return;

        // Seeded on the spawn curve through the module's math; from the next
        // frame on, the sample system places the dots on the live one.
        Spline seed = SplinePath.CreateSeedSpline();
        for (int index = existing; index < SplineSampleCount; index++)
        {
            float t = index * SplineSampleStep;
            float x = seed.GetLocationX(t);
            float y = seed.GetLocationY(t);
            commands.CreateEntity()
                .With(new SplineSample { T = t })
                .With(TransformComponent.At((x - 400.0f) / 80.0f, (300.0f - y) / 80.0f, 0.0f, SplineSampleDotSize / 160.0f))
                .With(MeshRendererComponent.None)
                .Build();
        }
    }
}

/// <summary>One camera looking down -Z at the mesh scene.</summary>
public static class CameraSpawnSystem
{
    [System]
    public static void Run(Query<Read<CameraComponent>> cameras, Commands commands)
    {
        foreach (var row in cameras.Rows()) { return; }
        commands.CreateEntity().With(DirectionalLightComponent.From(Vector3.One, 3)).With(TransformComponent.At(0, 0, 0, 1)).Build();
        commands.CreateEntity().With(CameraComponent.Perspective(60, 0.1f, 1000)).With(TransformComponent.At(0, 0, 9, 1)).Build();
    }
}
