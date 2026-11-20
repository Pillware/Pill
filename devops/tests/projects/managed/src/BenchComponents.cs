// Components for the managed iteration benchmark.
//
// Responsibilities
// - Declare the three blittable component types the benchmark systems iterate.
//
// Design
// These are a deliberate, field-for-field mirror of the components in
// `modules/engine/pill_engine/benches/query_iteration.rs`, because the whole point of
// this project is to put a managed number beside that Rust one. Same field
// count, same widths, same three-component population on every entity: an
// 8-byte Position, an 8-byte Velocity and a 4-byte Health. Getting this wrong
// is not a detail - a benchmark that moves fewer bytes per row than the one it
// is compared against reports a speed it did not earn.

using System.Runtime.InteropServices;

namespace ManagedTestProject;

/// <summary>Two-dimensional position. Mirrors the Rust bench's `Position`.</summary>
///
/// <remarks>
/// `LayoutKind.Sequential` is load-bearing, not decoration: the host registers
/// this type from the managed manifest and the engine stores its rows as raw
/// bytes, so the managed and native views have to agree on field order.
/// </remarks>
[StructLayout(LayoutKind.Sequential)]
public struct BenchPosition
{
    public float X;
    public float Y;
}

/// <summary>Two-dimensional velocity. Mirrors the Rust bench's `Velocity`.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct BenchVelocity
{
    public float X;
    public float Y;
}

/// <summary>Scalar hit points. Mirrors the Rust bench's `Health(f32)`.</summary>
///
/// <remarks>
/// A named field rather than a tuple struct, which C# has no equivalent of; the
/// stored bytes are the same single `f32`.
/// </remarks>
[StructLayout(LayoutKind.Sequential)]
public struct BenchHealth
{
    public float Value;
}
