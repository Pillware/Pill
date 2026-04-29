using System;

namespace Pill.ManagedHost;

public static class Engine
{
    public static void Log(string message)
    {
        Console.WriteLine($"[C#] {message}");
    }

    // TODO: these are fake right now - later call engine APIs
    public static bool TryGetTransform(ulong entity, out Transform transform)
    {
        // MVP fake transform.
        transform = new Transform
        {
            Position = new Vec3 { X = 0, Y = 0, Z = 0 },
            Rotation = new Vec3 { X = 0, Y = 0, Z = 0 },
            Scale = new Vec3 { X = 1, Y = 1, Z = 1 },
        };

        return true;
    }

    public static bool SetTransform(ulong entity, in Transform transform)
    {
        // MVP fake write.
        Log(
            $"SetTransform entity={entity} " +
            $"pos=({transform.Position.X}, {transform.Position.Y}, {transform.Position.Z}) " +
            $"rot=({transform.Rotation.X}, {transform.Rotation.Y}, {transform.Rotation.Z})"
        );

        return true;
    }
}
