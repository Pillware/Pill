// Managed mirrors of the renderer's components.
//
// These match the `#[repr(C)]` Rust definitions in `../src/component.rs`
// field for field: same names, same order, same types. The host binds them by
// canonical name and validates the layouts against the schema strings in
// `pill_csharp_bridge::components`, so a divergence is rejected at startup
// rather than corrupting memory.
//
// They live beside the renderer, not in csharp_runtime, for the same reason
// the Rust definitions do: a sprite is a renderer concept. A managed project
// that draws includes this file; one that does not, does not.

using System.Runtime.InteropServices;

namespace TracyLive;

/// <summary>World-space position of an entity's top-left draw origin, in pixels.</summary>
[EcsSharedComponent]
[StructLayout(LayoutKind.Sequential)]
public struct Position
{
    public float X;
    public float Y;
}

/// <summary>Plain RGBA color, 0.0-1.0 per channel.</summary>
[EcsSharedComponent]
[StructLayout(LayoutKind.Sequential)]
public struct Color
{
    public float R;
    public float G;
    public float B;
    public float A;
}

/// <summary>Axis-aligned colored rectangle drawn at an entity's <see cref="Position"/>.</summary>
[EcsSharedComponent]
[StructLayout(LayoutKind.Sequential)]
public struct Sprite
{
    public float Width;
    public float Height;
    public Color Color;
}
