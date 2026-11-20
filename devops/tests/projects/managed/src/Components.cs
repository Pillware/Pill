// Components owned by this project.
//
// Responsibilities
// - Declare the ball physics state and the spline sample dot the systems read
//   and write.
//
// Mirrors the component declarations in examples/project_rs/src/lib.rs: same
// names, same field order, same field types. The native host discovers these
// layouts from the managed component manifest and registers them itself - no
// Rust mirror and no match arm is involved on either side.

using System.Runtime.InteropServices;

// The renderer's components the docs below refer to, generated from Rust.
using pill_master_renderer;

namespace TracyLive;

/// <summary>
/// A rigid ball that bounces inside a fixed box, simulated each frame.
/// </summary>
/// <remarks>
/// The field attributes are the Inspector decorators (`Pill`'s Unity
/// vocabulary): presentation only, so they never reach the schema hash the
/// host compares against this declaration.
/// </remarks>
[StructLayout(LayoutKind.Sequential)]
public struct PhysicsState
{
    [HideInInspector] public float DeltaTime;

    [Header("Position")] public float PositionX;

    public float PositionY;

    [Header("Velocity")]
    [Space(10)] public float VelocityX;

    public float VelocityY;

    [Tooltip("Radius of the ball in world units")]
    [Min(0)] public float Radius;

    /// <summary>Rust's `active: bool`; only zero means inactive.</summary>
    public byte Active;
}

/// <summary>
/// One dot on the project's spline, drawn at the curve parameter `t`.
/// </summary>
/// <remarks>
/// The dot carries a <see cref="TransformComponent"/> and a
/// <see cref="MeshRendererComponent"/>, so it renders like any other mesh, and
/// the sample system moves it along the curve as the balls move.
/// </remarks>
[StructLayout(LayoutKind.Sequential)]
public struct SplineSample
{
    /// <summary>Curve parameter this dot samples, running from 0.0 to 1.0.</summary>
    [Header("Spline")]
    [Tooltip("Curve parameter this dot samples")]
    [Range(0f, 1f)]
    public float T;
}
