// The cube scene: a file-authored mesh and material, spun every frame.
//
// Responsibilities
// - Import `res/models/cube.obj` and `res/materials/cube.material` at startup.
// - Place the camera and the cube.
// - Spin the cube with a per-frame delta carried in an engine resource.
//
// Design
// `[Startup]` runs once for the life of the host, where a Rust project's
// `init` runs again on every hot reload; nothing here needs the "already
// built" guards the Rust twin keeps. The material file is imported after the
// renderer's data crate has installed the PBR chain it names by guid - that
// happens when the host loads the module, before any project code runs.
//
// No light entity: the shipped passes carry their own rig baked into their
// shaders, so a `DirectionalLightComponent` would only draw a renderer warning.

using System.Diagnostics;
using System.Numerics;
using System.Runtime.InteropServices;
// The renderer's components, generated from their Rust registration.
using pill_master_renderer;
// The renderer's assets and their store operations.
using pill_rendering;
// The ECS vocabulary (`Query`, `Commands`, the attribute types) and `Handle`.
using TracyLive;

namespace CubeExample;

/// <summary>Where the cube's model and material live, below <c>res</c>.</summary>
internal static class CubePaths
{
    internal const string Mesh = "models/cube.obj";
    internal const string Material = "materials/cube.material";
}

/// <summary>Seconds the previous frame took, kept where a reload cannot lose it.</summary>
/// <remarks>
/// A resource rather than a static: a static lives in the collectible load
/// context a reload replaces, so the clock would restart on every edit. A zero
/// timestamp means "never stamped", which is what a freshly created resource
/// reads as, so the first frame steps by nothing instead of measuring against
/// the epoch.
/// </remarks>
[EcsResource]
[StructLayout(LayoutKind.Sequential)]
public struct FrameClock
{
    /// <summary>Seconds the previous frame took.</summary>
    public float DeltaSeconds;

    /// <summary>Stopwatch timestamp of the previous stamp, or zero if never.</summary>
    public long LastFrameTimestamp;
}

/// <summary>Loads the cube's files and builds the scene, once.</summary>
public static class CubeStartup
{
    /// <summary>Imports both assets and spawns the camera and the cube.</summary>
    [Startup]
    public static void Start(Commands commands, ResMut<AssetManager> assets)
    {
        Handle<Mesh> mesh = assets.Import<Mesh>(CubePaths.Mesh, MetadataPolicy.CreateIfMissing).Handle;
        Handle<Material> material = assets.ImportStandalone<Material>(CubePaths.Material).Handle;

        // The camera looks down its local -Z axis from +Z.
        commands.CreateEntity()
            .With(TransformComponent.At(0.0f, 0.0f, 5.0f, 1.0f))
            .With(CameraComponent.Perspective(60.0f, 0.1f, 1000.0f))
            .Build();
        commands.CreateEntity()
            .With(TransformComponent.Identity)
            .With(MeshRendererComponent.From(mesh, material))
            .Build();
    }
}

/// <summary>Spins every drawn entity around X, Y and Z.</summary>
public static class SpinSystem
{
    private const float RadiansPerSecondX = 0.6f;
    private const float RadiansPerSecondY = 1.0f;
    private const float RadiansPerSecondZ = 0.3f;

    /// <summary>Turns the spun transform by one frame's worth of rotation.</summary>
    /// <remarks>
    /// The delta is clamped because a breakpoint or a slow reload can stretch
    /// one frame far enough to look like a jump rather than a spin.
    /// </remarks>
    [System]
    public static void Run(
        ResMut<FrameClock> clock,
        Query<Write<TransformComponent>, Read<MeshRendererComponent>> query)
    {
        ref FrameClock frame = ref clock.Value;
        long now = Stopwatch.GetTimestamp();
        frame.DeltaSeconds = frame.LastFrameTimestamp == 0
            ? 0.0f
            : Math.Clamp(
                (float)Stopwatch.GetElapsedTime(frame.LastFrameTimestamp, now).TotalSeconds,
                0.0f,
                0.1f);
        frame.LastFrameTimestamp = now;

        float delta = frame.DeltaSeconds;
        if (delta <= 0.0f)
            return;

        Quaternion step = Quaternion.CreateFromYawPitchRoll(
            RadiansPerSecondY * delta,
            RadiansPerSecondX * delta,
            RadiansPerSecondZ * delta);

        foreach (var row in query.Rows())
        {
            ref var transform = ref row.TransformComponent;
            transform.Rotation = Quaternion.Normalize(step * transform.Rotation);
        }
    }
}
