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
        return Native.pill_get_transform(entity, out transform);
    }

    public static bool SetTransform(ulong entity, in Transform transform)
    {
        return Native.pill_set_transform(entity, in transform);
    }
}
