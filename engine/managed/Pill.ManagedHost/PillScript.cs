namespace Pill.ManagedHost;

public abstract class PillScript
{
    public ulong Entity { get; internal set; }

    public virtual void OnStart() {}
    public virtual void OnUpdate(float dt) {}
    public virtual void OnDestroy() {}

    protected bool TryGetTransform(out Transform transform)
        => Engine.TryGetTransform(Entity, out transform);

    protected bool SetTransform(in Transform transform)
        => Engine.SetTransform(Entity, in transform);
}
