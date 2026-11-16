// The declarations a gameplay assembly marks its ECS members with.
//
// Responsibilities:
// - Declares the attributes system discovery reads: [System], [Startup],
//   [EcsSharedComponent], [EcsComponentAlias] and [EcsFieldDefault].
//
// Design:
// - Attributes only carry what the manifest needs to say; every rule about
//   what a declaration means lives in ProjectHost and the native host.

using System.Globalization;

namespace TracyLive;

// =============================================================================
// System Declaration
// =============================================================================

/// <summary>
/// Marks a static method as an ECS system and states where it runs.
/// </summary>
/// <remarks>
/// The properties mirror the Rust `#[system]` attribute, so a system written in
/// either language takes its place in the one schedule by the same rules:
/// `(phase, order)` across every artifact, and equal values keep registration
/// order. Every value is optional; the defaults describe an ordinary
/// project-phase system that runs only while the game plays.
/// </remarks>
[AttributeUsage(AttributeTargets.Method)]
public sealed class SystemAttribute : Attribute
{
    /// <summary>
    /// Display name, which is the system's identity in the reload signature,
    /// in the editor and in every log line. Defaults to `Type.Method`.
    /// </summary>
    public string? Name { get; set; }

    /// <summary>
    /// Sort key within the phase. Lower runs first, and the value orders this
    /// system against every other artifact's, Rust or managed.
    /// </summary>
    public int Order { get; set; }

    /// <summary>Which phase of the frame the system runs in.</summary>
    public RunPhase RunPhase { get; set; } = RunPhase.Project;

    /// <summary>
    /// Whether the system also runs while the editor edits, and while the
    /// engine is paused. The default keeps the game's behaviour under Play and
    /// Stop.
    /// </summary>
    public bool RunInEditMode { get; set; }

    /// <summary>
    /// Conditions that must all hold for the system to run, named with
    /// `nameof(...)`.
    /// </summary>
    /// <remarks>
    /// A condition is a static method that takes the read-only subset of a
    /// system's parameters and returns `bool`. It is evaluated in the managed
    /// runner before the body, so a skipped system still costs one transition.
    /// </remarks>
    public string[]? RunIf { get; set; }

    /// <summary>
    /// The type <see cref="RunIf"/> names its conditions on, when they are not
    /// declared on the system's own type.
    /// </summary>
    public Type? RunIfType { get; set; }
}

/// <summary>
/// The phase of the frame a <see cref="SystemAttribute"/> declares.
/// </summary>
public enum RunPhase
{
    /// <summary>Before the first deferred-command flush.</summary>
    PreProject = 0,

    /// <summary>The game's own frame work: after the first flush, before the second.</summary>
    Project = 1,

    /// <summary>After the second flush, where the frame's last structural changes are in.</summary>
    PostProject = 2,
}

/// <summary>Marks a one-shot method run before the first ECS frame.</summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class StartupAttribute : Attribute;

/// <summary>
/// Marks a component whose layout the native host already knows and binds
/// natively, rather than registering as a descriptor byte-level component.
/// </summary>
/// <remarks>
/// This is the managed half of a shared ABI: the host holds a canonical schema
/// for the type and validates this declaration against it, so the two cannot
/// drift. The renderer's components carry it.
///
/// It used to be inferred from assembly identity - "declared in csharp_runtime"
/// stood in for "the host binds it natively". That proxy broke once shared
/// components moved out to the crate that owns them, so the property is now
/// declared rather than guessed.
/// </remarks>
[AttributeUsage(AttributeTargets.Struct)]
public sealed class EcsSharedComponentAttribute : Attribute;

/// <summary>
/// Declares a name this struct used to be known by, so a rename migrates the
/// rows instead of reading as a disappearance.
/// </summary>
/// <remarks>
/// The manifest carries these names, and the host resolves each one to the
/// registration that answered to it: that registration's rows move onto this
/// type's new identity, in place, with fields matched by name exactly as a
/// layout change matches them. Without the alias a rename is not migratable -
/// the old declaration's storage cannot be safely retired - so the host
/// refuses the manifest and asks for a restart.
///
/// One hop only: an alias must name a declaration that was live when this
/// type was last registered, not another alias.
/// </remarks>
[AttributeUsage(AttributeTargets.Struct, AllowMultiple = true)]
public sealed class EcsComponentAliasAttribute : Attribute
{
    /// <summary>Declare one previous name of this component.</summary>
    /// <param name="oldName">
    /// The full name the type was declared under, as it appeared in the
    /// manifest - for example <c>"project_cs.SplineSample"</c>.
    /// </param>
    public EcsComponentAliasAttribute(string oldName) => OldName = oldName;

    /// <summary>The previous name this declaration claims.</summary>
    public string OldName { get; }
}

/// <summary>
/// Declares the value a newly added or reset field starts from, instead of
/// zero. Applies to component and resource fields alike.
/// </summary>
/// <remarks>
/// Read when the host migrates stored bytes: a field the migration carries
/// over keeps its value, and only a field the new layout leaves empty takes
/// the default. Literals only - the value travels as text in the manifest, and
/// the host refuses one that does not parse as the field's own type, so the
/// choice of constructor here is what makes a default on a field it cannot
/// describe a compile error rather than a runtime surprise.
/// </remarks>
[AttributeUsage(AttributeTargets.Field)]
public sealed class EcsFieldDefaultAttribute : Attribute
{
    /// <summary>Declare a floating-point default.</summary>
    public EcsFieldDefaultAttribute(double value) =>
        Literal = value.ToString("R", CultureInfo.InvariantCulture);

    /// <summary>Declare a single-precision default.</summary>
    public EcsFieldDefaultAttribute(float value) =>
        Literal = value.ToString("R", CultureInfo.InvariantCulture);

    /// <summary>Declare a signed integer default.</summary>
    public EcsFieldDefaultAttribute(long value) =>
        Literal = value.ToString(CultureInfo.InvariantCulture);

    /// <summary>Declare a signed integer default from an <see cref="int"/>.</summary>
    public EcsFieldDefaultAttribute(int value) =>
        Literal = value.ToString(CultureInfo.InvariantCulture);

    /// <summary>Declare an unsigned integer default.</summary>
    public EcsFieldDefaultAttribute(ulong value) =>
        Literal = value.ToString(CultureInfo.InvariantCulture);

    /// <summary>Declare a boolean default.</summary>
    public EcsFieldDefaultAttribute(bool value) => Literal = value ? "true" : "false";

    /// <summary>The literal the host parses against the field's declared type.</summary>
    public string Literal { get; }
}
