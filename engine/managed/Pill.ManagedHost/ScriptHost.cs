using System;

namespace Pill.ManagedHost;

public static class ScriptHost
{
    public static int Initialize(IntPtr args, int sizeBytes)
    {
        Console.WriteLine("Hello from C#!");
        return 2137;
    }
}
