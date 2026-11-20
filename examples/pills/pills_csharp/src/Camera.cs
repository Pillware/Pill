// The free flying camera and its field of view keys.
//
// Managed counterpart of examples/pills/pills_rust/src/camera.rs: WASD flies,
// E and Q rise and sink, Shift sprints, a right-button drag looks around, and T
// and G change the view width. Both movement and view are smoothed. Time and
// input come from the engine through Engine.Time and Engine.Input, so the
// systems read the same frame a native system does.
//
// It is one system because it is one camera: a separate view-width system would
// need a second query over the same rows and a second place the camera's limits
// live.

using System.Numerics;
using pill_master_renderer;
// The ECS vocabulary (`Query`, `Res`) and `Handle`.
using TracyLive;

namespace Pills;

/// <summary>View maths and limits the camera systems share.</summary>
internal static class CameraMath
{
    /// <summary>The narrowest field of view, in degrees; the bound is exclusive.</summary>
    internal const float NarrowestFov = 10.0f;

    /// <summary>The widest field of view, in degrees; the bound is exclusive.</summary>
    internal const float WidestFov = 120.0f;

    /// <summary>The view never tilts this far up or down, so it cannot flip over.</summary>
    internal const float PitchLimitDegrees = 89.0f;

    /// <summary>The camera's orientation for a pitch and a yaw, in degrees.</summary>
    /// <remarks>
    /// Yaw turns around the world's up axis and pitch tilts the view up and
    /// down; with both at zero the camera looks down -Z. This is the same
    /// rotation glam's YXZ euler order builds in the Rust project.
    /// </remarks>
    internal static Quaternion ViewRotation(Vector3 pitchYawDegrees) =>
        Quaternion.CreateFromYawPitchRoll(
            float.DegreesToRadians(pitchYawDegrees.Y),
            float.DegreesToRadians(pitchYawDegrees.X),
            0.0f);

    /// <summary><paramref name="value"/> with its Y dropped and unit length, or zero.</summary>
    internal static Vector3 FlattenedOnPlane(Vector3 value) =>
        NormalizeOrZero(new Vector3(value.X, 0.0f, value.Z));

    /// <summary><paramref name="value"/> at unit length, or zero when it is zero.</summary>
    internal static Vector3 NormalizeOrZero(Vector3 value)
    {
        float lengthSquared = value.LengthSquared();
        return lengthSquared > 0.0f ? value / MathF.Sqrt(lengthSquared) : Vector3.Zero;
    }
}

/// <summary>Flies the camera, turns it from right button drags and widens or narrows its view.</summary>
public static class FpsCameraSystem
{
    /// <summary>Degrees the field of view changes per second while T or G is held.</summary>
    private const float FovDegreesPerSecond = 100.0f;

    [System]
    public static void Run(
        Query<Write<TransformComponent>, Write<CameraComponent>, Write<CameraMovementComponent>> cameras)
    {
        float deltaTime = Engine.Time.DeltaSeconds;
        // The window does not capture the cursor, so the view only follows the
        // mouse while the right button is held.
        (float mouseDeltaX, float mouseDeltaY) = Engine.Input.MouseButtonHeld(MouseButton.Right)
            ? Engine.Input.MouseDelta
            : (0.0f, 0.0f);

        // T widens the view and G narrows it; holding both leaves it alone.
        float fovChange = 0.0f;
        if (Engine.Input.KeyHeld(KeyCode.KeyT))
            fovChange += FovDegreesPerSecond;
        if (Engine.Input.KeyHeld(KeyCode.KeyG))
            fovChange -= FovDegreesPerSecond;

        foreach (var row in cameras.Rows())
        {
            ref var transform = ref row.TransformComponent;
            ref var camera = ref row.CameraComponent;
            ref var movement = ref row.CameraMovementComponent;
            if (camera.Enabled == 0)
                continue;

            // Moving the mouse right turns right, moving it down looks down.
            movement.TargetRotation = new Vector3(
                Math.Clamp(
                    movement.TargetRotation.X - mouseDeltaY * movement.MouseSensitivity,
                    -CameraMath.PitchLimitDegrees,
                    CameraMath.PitchLimitDegrees),
                movement.TargetRotation.Y - mouseDeltaX * movement.MouseSensitivity,
                movement.TargetRotation.Z);

            // Ease the view towards where the mouse points.
            float rotationBlend = 1.0f - MathF.Exp(-movement.RotationLerpSpeed * deltaTime);
            Vector3 currentRotation = movement.CurrentRotation;
            currentRotation.X += (movement.TargetRotation.X - currentRotation.X) * rotationBlend;
            currentRotation.Y += (movement.TargetRotation.Y - currentRotation.Y) * rotationBlend;
            movement.CurrentRotation = currentRotation;

            Quaternion rotation = CameraMath.ViewRotation(currentRotation);
            transform.Rotation = rotation;

            // Walk on the horizontal plane the camera faces; E and Q fly
            // straight up and down.
            Vector3 forward = CameraMath.FlattenedOnPlane(Vector3.Transform(-Vector3.UnitZ, rotation));
            Vector3 right = CameraMath.FlattenedOnPlane(Vector3.Transform(Vector3.UnitX, rotation));
            Vector3 direction = Vector3.Zero;
            if (Engine.Input.KeyHeld(KeyCode.KeyW))
                direction += forward;
            if (Engine.Input.KeyHeld(KeyCode.KeyS))
                direction -= forward;
            if (Engine.Input.KeyHeld(KeyCode.KeyD))
                direction += right;
            if (Engine.Input.KeyHeld(KeyCode.KeyA))
                direction -= right;
            if (Engine.Input.KeyHeld(KeyCode.KeyE))
                direction.Y += 1.0f;
            if (Engine.Input.KeyHeld(KeyCode.KeyQ))
                direction.Y -= 1.0f;

            float speed = Engine.Input.KeyHeld(KeyCode.ShiftLeft)
                ? movement.MoveSpeed * movement.SprintMultiplier
                : movement.MoveSpeed;
            Vector3 targetVelocity = CameraMath.NormalizeOrZero(direction) * speed;
            movement.TargetVelocity = targetVelocity;

            // Ease the velocity, so starting and stopping are smooth.
            float velocityBlend = 1.0f - MathF.Exp(-movement.LerpSpeed * deltaTime);
            Vector3 currentVelocity = movement.CurrentVelocity +
                (targetVelocity - movement.CurrentVelocity) * velocityBlend;
            movement.CurrentVelocity = currentVelocity;

            transform.Translation += currentVelocity * deltaTime;

            float fov = camera.VerticalFov + fovChange * deltaTime;
            if (fov > CameraMath.NarrowestFov && fov < CameraMath.WidestFov)
                camera.VerticalFov = fov;
        }
    }
}
