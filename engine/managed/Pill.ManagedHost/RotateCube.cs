namespace Pill.ManagedHost;

public sealed class RotateCube : PillScript
{
    public override void OnStart()
    {
        Engine.Log($"RotateCube.OnStart entity={Entity}");
    }

    public override void OnUpdate(float dt)
    {
        if (!TryGetTransform(out var transform))
        {
            Engine.Log($"RotateCube.OnUpdate failed to get transform for entity={Entity}");
            return;
        }

        transform.Rotation.Y += dt;

        SetTransform(transform);
        Engine.Log($"RotateCube.OnUpdate entity={Entity} dt={dt}");
    }

    public override void OnDestroy()
    {
        Engine.Log($"RotateCube.OnDestroy entity={Entity}");
    }
}
