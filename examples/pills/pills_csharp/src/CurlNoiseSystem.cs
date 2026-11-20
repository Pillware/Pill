// Moves every pill through an animated curl noise field.
//
// Managed counterpart of examples/pills/pills_rust/src/curl_noise_system.rs,
// and the same algorithm: Bridson, Hourihan and Nordenstam's curl noise ("Curl
// Noise for Procedural Fluid Flow", SIGGRAPH 2007) - take a vector potential
// built from three decorrelated noise fields and move along its curl, which is
// divergence free, so the pills swirl instead of bunching into sinks.
//
// Like the Rust system, the row loop is a parallel pass: `ForEachParallel`
// dispatches it onto the host's shared pool (the runtime resolves the chunks
// on the frame thread first). The body runs on worker threads, so it touches
// only the two component references it is handed plus a snapshot of the
// tuning resource taken before the pass - no engine calls, no Commands.

using System.Numerics;
using pill_master_renderer;
// The ECS vocabulary (`Query`, `Res`, `ForEachParallel`) and `Handle`.
using TracyLive;

namespace Pills;

/// <summary>Animates every pill through the curl noise field, one frame at a time.</summary>
public static class CurlNoiseSystem
{
    [System]
    public static void Run(
        ResMut<DemoState> demoState,
        Query<Write<TransformComponent>, Write<CurlNoiseComponent>> pills)
    {
        ref DemoState demo = ref demoState.Value;
        if (demo.Initialized == 0)
            demo = DemoState.TunedDefaults();

        // Frame-thread snapshot: workers cannot read the resource, so the
        // pass carries the tuning by value along with the two frame clocks.
        DemoState tuning = demo;
        float deltaTime = Engine.Time.DeltaSeconds;
        float elapsed = Engine.Time.ElapsedSeconds;
        pills.ForEachParallel(
            1024,
            (ref TransformComponent transform, ref CurlNoiseComponent pill) =>
            {
                Vector3 position = transform.Translation;
                Vector3 velocity = pill.Velocity;
                CurlNoiseField.StepPill(
                    ref position,
                    ref velocity,
                    pill.CurlStrength,
                    in tuning,
                    elapsed,
                    deltaTime);
                transform.Translation = position;
                pill.Velocity = velocity;
            });
    }
}

/// <summary>The field's maths and its constants, apart from the system that drives it.</summary>
/// <remarks>
/// Kept out of the system-declaring type because a static field of an array
/// or a struct is exactly the state PILL0301 exists to flag - here it is
/// genuinely constant, and a type that declares no systems is free to say so.
/// </remarks>
internal static class CurlNoiseField
{
    /// <summary>The box the pills are kept inside.</summary>
    private static readonly Vector3 BoxMinimum = new(-120.0f, -50.0f, -120.0f);
    private static readonly Vector3 BoxMaximum = new(120.0f, 140.0f, 120.0f);

    /// <summary>
    /// The point the pills are pulled towards. The original demo computed it as
    /// the box's corner sum over twelve, which puts it at (0, 7.5, 0); the look
    /// is tuned to that point.
    /// </summary>
    private static readonly Vector3 AttractionCenter = new(
        (BoxMinimum.X + BoxMaximum.X) / 12.0f,
        (BoxMinimum.Y + BoxMaximum.Y) / 12.0f,
        (BoxMinimum.Z + BoxMaximum.Z) / 12.0f);

    /// <summary>Fastest a pill may move, in world units per second.</summary>
    private const float MaximumSpeed = 20.0f;

    /// <summary>How fast the noise field scrolls, per second, along every axis.</summary>
    private const float FieldScrollSpeed = 0.4f;

    /// <summary>Offsets that decorrelate the three noise fields of the vector potential.</summary>
    private static readonly Vector3[] FieldOffsets =
    [
        Vector3.Zero,
        new(31.416f, 0.0f, 0.0f),
        new(0.0f, 67.254f, 0.0f),
    ];

    /// <summary>The next position and velocity of one pill.</summary>
    internal static void StepPill(
        ref Vector3 position,
        ref Vector3 velocity,
        float curlStrength,
        in DemoState demoState,
        float time,
        float deltaTime)
    {
        Vector3 curl = CurlNoise(position, time, demoState.CurlEpsilon, demoState.CurlScale);
        Vector3 force = curl * curlStrength;

        // A constant strength pull towards the middle of the field.
        Vector3 toCenter = AttractionCenter - position;
        float distanceToCenter = toCenter.Length();
        if (distanceToCenter > 0.1f)
            force += toCenter * (demoState.CurlAttraction / distanceToCenter);

        // The original demo keeps a third of the previous velocity before damping.
        velocity = ClampLengthMax(
            (velocity / 3.0f + force * deltaTime) * demoState.CurlDamping,
            MaximumSpeed);
        position = Vector3.Clamp(position + velocity * deltaTime, BoxMinimum, BoxMaximum);
    }

    /// <summary><paramref name="value"/> cut down to <paramref name="maximum"/> length.</summary>
    private static Vector3 ClampLengthMax(Vector3 value, float maximum)
    {
        float length = value.Length();
        return length > maximum ? value * (maximum / length) : value;
    }

    /// <summary>The curl of the vector potential at a point, by central differences.</summary>
    private static Vector3 CurlNoise(Vector3 point, float time, float epsilon, float scale)
    {
        Vector3 animated = point * scale + new Vector3(time * FieldScrollSpeed);
        // The cell of each field around the centre point. The six samples are
        // within `epsilon` of it, so they nearly always fall in these cells and
        // the whole curl hashes 24 corners instead of 144.
        LatticeCell cell0 = LatticeCell.At(Floor(animated + FieldOffsets[0]));
        LatticeCell cell1 = LatticeCell.At(Floor(animated + FieldOffsets[1]));
        LatticeCell cell2 = LatticeCell.At(Floor(animated + FieldOffsets[2]));

        // The derivative of every component of the potential along one axis.
        Vector3 Derivative(Vector3 axis) =>
            (Potential(animated + axis * epsilon, cell0, cell1, cell2) -
             Potential(animated - axis * epsilon, cell0, cell1, cell2)) / (2.0f * epsilon);

        Vector3 alongX = Derivative(Vector3.UnitX);
        Vector3 alongY = Derivative(Vector3.UnitY);
        Vector3 alongZ = Derivative(Vector3.UnitZ);

        return new Vector3(
            alongY.Z - alongZ.Y,
            alongZ.X - alongX.Z,
            alongX.Y - alongY.X);
    }

    /// <summary>
    /// The vector potential at a point: three noise fields, offset to
    /// decorrelate them. A field whose point lies in the cell handed in reuses
    /// its hashes; any other hashes its own.
    /// </summary>
    private static Vector3 Potential(
        Vector3 point,
        in LatticeCell cell0,
        in LatticeCell cell1,
        in LatticeCell cell2) => new(
        Field(point + FieldOffsets[0], cell0),
        Field(point + FieldOffsets[1], cell1),
        Field(point + FieldOffsets[2], cell2));

    /// <summary>One noise field, reusing a nearby cell's hashes when they apply.</summary>
    private static float Field(Vector3 fieldPoint, in LatticeCell cachedCell)
    {
        Vector3 origin = Floor(fieldPoint);
        return origin == cachedCell.Origin
            ? cachedCell.Noise(fieldPoint)
            : LatticeCell.At(origin).Noise(fieldPoint);
    }

    /// <summary>Pseudo random value in <c>[0, 1)</c> for a lattice point.</summary>
    private static float Hash(Vector3 point)
    {
        float value = MathF.Sin(point.X * 127.1f + point.Y * 311.7f + point.Z * 74.7f);
        float scaled = value * 43_758.547f;
        return scaled - MathF.Floor(scaled);
    }

    /// <summary>The sample point rounded down, axis by axis.</summary>
    private static Vector3 Floor(Vector3 point) =>
        new(MathF.Floor(point.X), MathF.Floor(point.Y), MathF.Floor(point.Z));

    /// <summary>
    /// The eight lattice hashes around one cell of the noise lattice.
    /// </summary>
    /// <remarks>
    /// Hashing is the expensive part of the noise (each hash is a sine), and
    /// the curl samples six points within a few thousandths of each other,
    /// which nearly always share a cell. Keeping a cell's hashes lets every
    /// sample in it reuse them instead of hashing the same eight corners again.
    /// </remarks>
    private readonly struct LatticeCell
    {
        /// <summary>The cell's lowest corner: the sample point rounded down.</summary>
        private readonly Vector3 _origin;

        // The corner hashes, named for the (x, y, z) offsets that reach them;
        // the Rust project keeps the same eight values in one indexed array.
        private readonly float _c000, _c100, _c010, _c110, _c001, _c101, _c011, _c111;

        private LatticeCell(Vector3 origin, float c000, float c100, float c010, float c110,
            float c001, float c101, float c011, float c111)
        {
            _origin = origin;
            _c000 = c000;
            _c100 = c100;
            _c010 = c010;
            _c110 = c110;
            _c001 = c001;
            _c101 = c101;
            _c011 = c011;
            _c111 = c111;
        }

        /// <summary>The cell <paramref name="origin"/> names, its corners hashed.</summary>
        internal static LatticeCell At(Vector3 origin) => new(
            origin,
            Hash(origin),
            Hash(origin + new Vector3(1.0f, 0.0f, 0.0f)),
            Hash(origin + new Vector3(0.0f, 1.0f, 0.0f)),
            Hash(origin + new Vector3(1.0f, 1.0f, 0.0f)),
            Hash(origin + new Vector3(0.0f, 0.0f, 1.0f)),
            Hash(origin + new Vector3(1.0f, 0.0f, 1.0f)),
            Hash(origin + new Vector3(0.0f, 1.0f, 1.0f)),
            Hash(origin + new Vector3(1.0f, 1.0f, 1.0f)));

        /// <summary>The cell's lowest corner.</summary>
        internal readonly Vector3 Origin => _origin;

        /// <summary>
        /// Smooth value noise at <paramref name="point"/>, which lies in this
        /// cell: the corner hashes blended with a smoothstep in each axis.
        /// </summary>
        internal readonly float Noise(Vector3 point)
        {
            Vector3 offset = point - _origin;
            Vector3 blend = offset * offset * (new Vector3(3.0f) - 2.0f * offset);

            float bottomFront = Lerp(_c000, _c100, blend.X);
            float topFront = Lerp(_c010, _c110, blend.X);
            float bottomBack = Lerp(_c001, _c101, blend.X);
            float topBack = Lerp(_c011, _c111, blend.X);
            float front = Lerp(bottomFront, topFront, blend.Y);
            float back = Lerp(bottomBack, topBack, blend.Y);
            return Lerp(front, back, blend.Z);
        }

        private static float Lerp(float from, float to, float amount) =>
            from + (to - from) * amount;
    }
}
