using System.Runtime.InteropServices;

namespace Pill.ManagedHost;

[StructLayout(LayoutKind.Sequential)]
public struct Vec3
{
    public float X;
    public float Y;
    public float Z;
}

[StructLayout(LayoutKind.Sequential)]
public struct Transform
{
    public Vec3 Position;
    public Vec3 Rotation;
    public Vec3 Scale;
}
