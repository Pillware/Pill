// The cube whose model is a shipped OBJ and whose material is built in code:
// no `.meta`, no scene file, no editor layer.
//
// Responsibilities
// - Import `res/models/cube.obj` without reading or writing metadata beside it,
//   and build the material through the renderer's mirrored builder.
// - Place the camera and the cube.
// - Spin the cube with a per-frame delta carried in an engine resource.
//
// Design
// The twin of `cube`, which ships the same model *and* a committed `.meta` so
// the asset has a stable guid for references and the editor can edit it. This
// project imports the model with `MetadataPolicy.ReadIfPresent`: it decodes
// under the importer's defaults and its guid lives only in memory, so the `res`
// tree stays exactly what the repository ships. The material names the PBR
// shader by the guid its source `.meta` carries - the same guid `cube.material`
// uses - so a run without the renderer's data crate loaded would draw nothing.
// No light entity: the shipped passes carry their own rig baked into their
// shaders.

using System.Diagnostics;
using System.Numerics;
using System.Runtime.InteropServices;
// The renderer's components, generated from their Rust registration.
using pill_master_renderer;
// The renderer's asset types, their builders and their store operations.
using pill_master_renderer_data;
// The ECS vocabulary (`Query`, `Commands`, the attribute types) and `Handle`.
using TracyLive;

namespace CubeNoEditorExample;

/// <summary>The paths and the tint the scene is built from.</summary>
internal static class CubeAssets
{
    /// <summary>The model, relative to `res`; deliberately without a `.meta`.</summary>
    internal const string MeshPath = "models/cube.obj";

    /// <summary>The name the material is stored under.</summary>
    internal const string MaterialName = "cube.material";

    /// <summary>The PBR shader every material in this renderer names by guid.</summary>
    internal const string PbrShaderGuid = "8cedb4197c9f26a86708b1cd14399eab";

    /// <summary>The cube's surface colour.</summary>
    internal static readonly Vector3 Tint = new(0.80f, 0.80f, 0.82f);

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

/// <summary>Builds the cube's two assets and the scene around them, once.</summary>
public static class CubeStartup
{
    /// <summary>Stores the mesh and material, then spawns the camera and cube.</summary>
    [Startup]
    public static void Start(Commands commands, ResMut<AssetManager> assets)
    {
        // `ReadIfPresent` is the point of this project: the source decodes under
        // the importer's defaults, the guid stays in memory, and no `.meta` is
        // written beside the file.
        Handle<Mesh> mesh = assets.Import<Mesh>(
            CubeAssets.MeshPath, MetadataPolicy.ReadIfPresent).Handle;

        MaterialBuilder builder = Material.Builder(CubeAssets.MaterialName)
            .ColorParameter("pbr_base", CubeAssets.Tint)
            .ColorParameter("pbr_emissive", Vector3.Zero)
            .ScalarParameter("pbr_metallic", 0.0f)
            .ScalarParameter("pbr_roughness", 0.35f);
        // The chain is installed by the renderer's data crate; naming its
        // shader explicitly keeps the material off the built-in fallback.
        if (assets.FindByGuid<Shader>(CubeAssets.PbrShaderGuid) is Handle<Shader> shader)
            builder = builder.Shader(shader);
        Handle<Material> material = assets.AddNamed(CubeAssets.MaterialName, builder.Build());

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
