// The components and the tuning resource this project owns.
//
// Managed counterpart of examples/pills/pills_rust/src/game.rs's type section.
// The two components are plain blittable structs, discovered from their use in
// a query or a Commands call the way every project-owned component is. The
// Rust project declares them with a `shared = "pills_rust::..."` name so hot
// patched modules can meet them; nothing here shares them with Rust code, so
// they carry no [EcsSharedComponent] and the name they register under is their
// own full name.

using System.Numerics;
using System.Runtime.InteropServices;
// The component and resource attributes and the query vocabulary.
using TracyLive;

namespace Pills;

/// <summary>One pill's motion through the curl noise field.</summary>
/// <remarks>
/// The vectors are <see cref="Vector3"/> where the Rust component holds
/// `[f32; 3]`; both are three sequential floats, so the two layouts agree.
/// </remarks>
[StructLayout(LayoutKind.Sequential)]
public struct CurlNoiseComponent
{
    /// <summary>Current velocity, in world units per second.</summary>
    public Vector3 Velocity;

    /// <summary>How strongly the field pushes this pill.</summary>
    public float CurlStrength;

    /// <summary>Per-pill noise scale, drawn at spawn; the field uses the shared scale.</summary>
    public float NoiseScale;
}

/// <summary>Settings and smoothed state of the free flying camera.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct CameraMovementComponent
{
    /// <summary>Flying speed, in world units per second.</summary>
    public float MoveSpeed;

    /// <summary>Speed factor while Shift is held.</summary>
    public float SprintMultiplier;

    /// <summary>How quickly the velocity follows the keys.</summary>
    public float LerpSpeed;

    /// <summary>Degrees turned per pixel of mouse movement.</summary>
    public float MouseSensitivity;

    /// <summary>How quickly the view follows the mouse.</summary>
    public float RotationLerpSpeed;

    /// <summary>Velocity this frame, in world units per second.</summary>
    public Vector3 CurrentVelocity;

    /// <summary>Velocity the keys ask for.</summary>
    public Vector3 TargetVelocity;

    /// <summary>View this frame: pitch and yaw in degrees (the third value is unused).</summary>
    public Vector3 CurrentRotation;

    /// <summary>View the mouse asks for: pitch and yaw in degrees.</summary>
    public Vector3 TargetRotation;
}

/// <summary>The curl noise field's live tuning.</summary>
/// <remarks>
/// A managed resource starts as zeroed bytes, where the Rust project's
/// `register()` inserts its tuned defaults into a world that has none; with
/// damping and epsilon at zero the field would stand still, so the curl system
/// fills the defaults in on its first pass and records that in
/// <see cref="Initialized"/>. The flag lives in the engine's storage beside
/// the value, so a reload finds the seeded state where it left it and leaves
/// it alone.
/// </remarks>
[EcsResource]
[StructLayout(LayoutKind.Sequential)]
public struct DemoState
{
    /// <summary>Spatial frequency of the noise.</summary>
    public float CurlScale;

    /// <summary>Finite difference step used to take the curl.</summary>
    public float CurlEpsilon;

    /// <summary>Pull towards the middle of the field.</summary>
    public float CurlAttraction;

    /// <summary>Velocity multiplier applied every frame.</summary>
    public float CurlDamping;

    /// <summary>1 once <see cref="TunedDefaults"/> has been applied.</summary>
    public int Initialized;

    /// <summary>The values the original demo was tuned to.</summary>
    internal static DemoState TunedDefaults() => new()
    {
        CurlScale = 0.1005f,
        CurlEpsilon = 0.0038f,
        CurlAttraction = 155.0f,
        CurlDamping = 2.91f,
        Initialized = 1,
    };
}
