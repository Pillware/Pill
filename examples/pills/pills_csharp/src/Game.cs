// The demo's scene, its scattered pills and its live tuning keys.
//
// Managed counterpart of examples/pills/pills_rust/src/game.rs's remaining
// half: `create_scene`'s camera and pills become the startup below, and
// `demo_control_system` becomes the system of the same name. Unlike the Rust
// project's `#[pill_project] fn init`, which runs again on every hot reload, a
// managed startup runs exactly once for the life of the host, so nothing here
// needs the "already built" guards the Rust version keeps.
//
// Asset loading goes through the asset store the renderer data crate mirrors
// to C#: `assets.Import<Mesh>`/`<Texture>` for the mesh and the map (whose
// `.meta` files this project ships, so they import under their own guids), and
// `assets.ImportStandalone<Material>` for the `.material` file, which is a
// standalone asset with no `.meta` of its own. A startup declares the resources
// it needs as parameters, like every system in this project.

using System.Numerics;
// The renderer's components, generated from their Rust registration.
using pill_master_renderer;
// The renderer's assets, builders and pipeline resource, generated from their
// Rust declarations.
using pill_rendering;
// The ECS vocabulary (`Query`, `Commands`, `Res`) and `Handle`.
using TracyLive;

namespace Pills;

/// <summary>The scene's constants: what to load, and what to scatter where.</summary>
internal static class ProjectConstants
{
    /// <summary>How many pills drift through the field.</summary>
    internal const int PillCount = 200_000;

    /// <summary>Uniform scale of every pill.</summary>
    internal const float PillScale = 0.35f;

    /// <summary>Where the camera starts: at eye height, ten units in front of the origin.</summary>
    internal static readonly Vector3 CameraStart = new(0.0f, 1.6f, -10.0f);

    /// <summary>
    /// The camera starts turned around to face the origin, because a camera
    /// looks down its local -Z axis.
    /// </summary>
    internal const float CameraStartYawDegrees = 180.0f;

    /// <summary>The pill mesh, relative to `res`. The path is also the asset's name.</summary>
    internal const string PillMesh = "models/pill.obj";

    /// <summary>The pill material's file, relative to `res`.</summary>
    internal const string PillMaterial = "materials/pill.material";
}

/// <summary>Loads the scene's assets and builds it, once.</summary>
public static class SceneStartup
{
    [Startup]
    public static void Start(
        Commands commands,
        ResMut<AssetManager> assets)
    {
        // The renderer's data crate installed the PBR chain when the host
        // loaded it, and the material file below names that shader by guid;
        // this project's job is the assets themselves.
        Handle<Mesh> pillMesh = assets.Import<Mesh>(
            ProjectConstants.PillMesh, MetadataPolicy.CreateIfMissing).Handle;

        // The material names its map by guid, so the map loads first. A map
        // with no `.meta` file yet is read with the type named here and gets
        // one written beside it, exactly as the Rust project's imports do.
        ImportTexture(assets, "textures/grid.png", "Color");

        Handle<Material> pillMaterial = assets.ImportStandalone<Material>(
            ProjectConstants.PillMaterial).Handle;

        SpawnCamera(commands);
        SpawnPills(commands, pillMesh, pillMaterial, ProjectConstants.PillCount);
    }

    /// <summary>Imports one map with the texture type to read it as.</summary>
    private static void ImportTexture(ResMut<AssetManager> assets, string path, string textureType) =>
        assets.Import<Texture>(
            path,
            MetadataPolicy.CreateIfMissing,
            $"{{\"texture_type\":\"{textureType}\"}}");

    /// <summary>Spawns the free flying camera.</summary>
    private static void SpawnCamera(Commands commands)
    {
        Vector3 startRotation = new(0.0f, ProjectConstants.CameraStartYawDegrees, 0.0f);
        var transform = TransformComponent.At(
            ProjectConstants.CameraStart.X,
            ProjectConstants.CameraStart.Y,
            ProjectConstants.CameraStart.Z,
            1.0f);
        transform.Rotation = CameraMath.ViewRotation(startRotation);
        commands.CreateEntity()
            .With(transform)
            .With(CameraComponent.Perspective(75.0f, 0.1f, 1000.0f))
            .With(new CameraMovementComponent
            {
                MoveSpeed = 25.0f,
                SprintMultiplier = 2.0f,
                LerpSpeed = 8.0f,
                MouseSensitivity = 0.1f,
                RotationLerpSpeed = 25.0f,
                CurrentRotation = startRotation,
                TargetRotation = startRotation,
            })
            .Build();
    }

    /// <summary>Scatters `count` pills through the field, each turned and pushed at random.</summary>
    private static void SpawnPills(
        Commands commands, Handle<Mesh> mesh, Handle<Material> material, int count)
    {
        MeshRendererComponent renderer = MeshRendererComponent.From(mesh, material);

        for (int index = 0; index < count; index++)
        {
            // Each pill draws its eight values from its own block of seeds, so
            // the scatter depends only on the index.
            ulong seed = (ulong)index * 8;
            Quaternion rotation =
                Quaternion.CreateFromAxisAngle(Vector3.UnitX, Scatter.Between(seed, 0.0f, 2.0f * MathF.PI)) *
                Quaternion.CreateFromAxisAngle(Vector3.UnitY, Scatter.Between(seed + 1, 0.0f, 2.0f * MathF.PI)) *
                Quaternion.CreateFromAxisAngle(Vector3.UnitZ, Scatter.Between(seed + 2, 0.0f, 2.0f * MathF.PI));

            var transform = TransformComponent.At(
                Scatter.Between(seed + 5, -20.0f, 20.0f),
                Scatter.Between(seed + 6, 0.0f, 40.0f),
                Scatter.Between(seed + 7, -20.0f, 20.0f),
                ProjectConstants.PillScale);
            transform.Rotation = rotation;

            commands.CreateEntity()
                .With(new CurlNoiseComponent
                {
                    Velocity = Vector3.Zero,
                    CurlStrength = Scatter.Between(seed + 3, 70.0f, 120.0f),
                    NoiseScale = Scatter.Between(seed + 4, 0.1f, 0.2f),
                })
                .With(transform)
                .With(renderer)
                .Build();
        }
    }
}

/// <summary>Tunes the curl noise field while its keys are held.</summary>
public static class DemoControlSystem
{
    /// <summary>How far each key moves its value per frame.</summary>
    private const float ScaleStep = 0.0005f;
    private const float AttractionStep = 5.0f;
    private const float DampingStep = 0.01f;
    private const float EpsilonStep = 0.001f;

    [System]
    public static void Run(ResMut<DemoState> demoState)
    {
        ref DemoState demo = ref demoState.Value;

        // Two keys per value, one step each way (O/P, L/K, M/N, V/B).
        bool changed = false;
        changed |= Tune(KeyCode.KeyO, ScaleStep, ref demo.CurlScale);
        changed |= Tune(KeyCode.KeyP, -ScaleStep, ref demo.CurlScale);
        changed |= Tune(KeyCode.KeyL, AttractionStep, ref demo.CurlAttraction);
        changed |= Tune(KeyCode.KeyK, -AttractionStep, ref demo.CurlAttraction);
        changed |= Tune(KeyCode.KeyM, DampingStep, ref demo.CurlDamping);
        changed |= Tune(KeyCode.KeyN, -DampingStep, ref demo.CurlDamping);
        changed |= Tune(KeyCode.KeyV, EpsilonStep, ref demo.CurlEpsilon);
        changed |= Tune(KeyCode.KeyB, -EpsilonStep, ref demo.CurlEpsilon);

        if (changed)
        {
            Log.Info(
                $"[pills] curl scale: {demo.CurlScale:F4}, " +
                $"attraction: {demo.CurlAttraction:F2}, " +
                $"damping: {demo.CurlDamping:F4}, " +
                $"epsilon: {demo.CurlEpsilon:F4}");
        }

        // Moves `value` by `step` while its key is held, and says whether it did.
        static bool Tune(KeyCode key, float step, ref float value)
        {
            if (!Engine.Input.KeyHeld(key))
                return false;
            value += step;
            return true;
        }
    }
}

/// <summary>A pseudo random value in <c>[0, 1)</c> for a seed.</summary>
/// <remarks>
/// The SplitMix64 finalizer: a bijection on 64 bits, so consecutive seeds give
/// uncorrelated values. The scatter is a pure function of a pill's index - the
/// same in both projects - so neither needs a generator with state, which in a
/// reloadable assembly would also mean a destructor in the unloaded image.
/// </remarks>
internal static class Scatter
{
    /// <summary>A value in <c>[0, 1)</c>.</summary>
    internal static float Unit(ulong seed)
    {
        ulong mixed = unchecked(seed + 0x9E37_79B9_7F4A_7C15);
        mixed = unchecked((mixed ^ (mixed >> 30)) * 0xBF58_476D_1CE4_E5B9);
        mixed = unchecked((mixed ^ (mixed >> 27)) * 0x94D0_49BB_1331_11EB);
        mixed ^= mixed >> 31;
        // The top 24 bits fill an f32 mantissa exactly.
        return (mixed >> 40) / (float)(1UL << 24);
    }

    /// <summary>A value uniform in <c>[minimum, maximum)</c>.</summary>
    internal static float Between(ulong seed, float minimum, float maximum) =>
        minimum + Unit(seed) * (maximum - minimum);
}
