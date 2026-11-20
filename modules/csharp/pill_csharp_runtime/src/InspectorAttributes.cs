// Unity-compatible field decorators for the editor's Inspector.
//
// A component declares these on its fields the way a Unity MonoBehaviour does,
// and the component manifest carries them to the host, which hands them to the
// editor with the field layout. They are presentation only: storage, the
// schema hash and the native ABI are untouched, so decorating a field never
// migrates rows.
//
// The names are Unity's, deliberately without the `Ecs` prefix that marks the
// engine's control attributes (`EcsSharedComponent`, `EcsFieldDefault`, ...):
// this is authoring vocabulary a Unity developer already knows, and the point
// is that a MonoBehaviour's field attributes survive the move.

namespace TracyLive;

/// <summary>Unity's <c>[HideInInspector]</c>: the Inspector does not draw the field.</summary>
/// <remarks>
/// The field still exists, is still stored, serialized and copied with its
/// component; only its row in the Inspector disappears.
/// </remarks>
[AttributeUsage(AttributeTargets.Field, AllowMultiple = false)]
public sealed class HideInInspectorAttribute : Attribute;

/// <summary>Unity's <c>[Header("Text")]</c>: a separator line with the text above the field.</summary>
[AttributeUsage(AttributeTargets.Field, AllowMultiple = true)]
public sealed class HeaderAttribute : Attribute
{
    /// <summary>Declare a header line.</summary>
    /// <param name="header">Text the separator line shows.</param>
    public HeaderAttribute(string header) => this.header = header;

    /// <summary>Text the separator line shows.</summary>
    public string header { get; }
}

/// <summary>Unity's <c>[Space(10)]</c>: a vertical gap above the field.</summary>
[AttributeUsage(AttributeTargets.Field, AllowMultiple = true)]
public sealed class SpaceAttribute : Attribute
{
    /// <summary>Declare the gap Unity uses by default: 8 pixels.</summary>
    public SpaceAttribute() { }

    /// <summary>Declare a gap of a given height.</summary>
    /// <param name="height">Height of the gap, in pixels.</param>
    public SpaceAttribute(float height) => this.height = height;

    /// <summary>Height of the gap, in pixels; 8 when the constructor takes none.</summary>
    public float height { get; } = 8.0f;
}

/// <summary>Unity's <c>[Tooltip("Text")]</c>: hover text on the field's label.</summary>
[AttributeUsage(AttributeTargets.Field, AllowMultiple = false)]
public sealed class TooltipAttribute : Attribute
{
    /// <summary>Declare the hover text.</summary>
    /// <param name="tooltip">Text shown while the pointer rests on the label.</param>
    public TooltipAttribute(string tooltip) => this.tooltip = tooltip;

    /// <summary>Text shown while the pointer rests on the label.</summary>
    public string tooltip { get; }
}

/// <summary>Unity's <c>[Range(0, 100)]</c>: a slider over the range.</summary>
/// <remarks>
/// An integer field slides in whole steps. The parameters are <c>float</c> so
/// Unity's <c>[Range(0, 100)]</c> compiles through the implicit conversion.
/// </remarks>
[AttributeUsage(AttributeTargets.Field, AllowMultiple = false)]
public sealed class RangeAttribute : Attribute
{
    /// <summary>Declare the slider's range.</summary>
    /// <param name="min">Lowest value the slider offers.</param>
    /// <param name="max">Highest value the slider offers.</param>
    public RangeAttribute(float min, float max)
    {
        this.min = min;
        this.max = max;
    }

    /// <summary>Lowest value the slider offers.</summary>
    public float min { get; }

    /// <summary>Highest value the slider offers.</summary>
    public float max { get; }
}

/// <summary>Unity's <c>[TextArea]</c> / <c>[TextArea(3, 10)]</c>: a multi-line text box.</summary>
/// <remarks>
/// Pill's managed component declarations are blittable value types, so no
/// component field can be a <c>string</c> today. The attribute is defined for
/// parity with Unity and for the managed text lane a later item would add; the
/// text rendering is exercised on the Rust side, whose components can own text.
/// </remarks>
[AttributeUsage(AttributeTargets.Field, AllowMultiple = false)]
public sealed class TextAreaAttribute : Attribute
{
    /// <summary>Declare Unity's default: a three-row box, 3..3 lines.</summary>
    public TextAreaAttribute()
    {
        minLines = 3;
        maxLines = 3;
    }

    /// <summary>Declare a box of a given line span.</summary>
    /// <param name="minLines">Rows the box starts with.</param>
    /// <param name="maxLines">Rows the box grows to before it scrolls.</param>
    public TextAreaAttribute(int minLines, int maxLines)
    {
        this.minLines = minLines;
        this.maxLines = maxLines;
    }

    /// <summary>Rows the box starts with.</summary>
    public int minLines { get; }

    /// <summary>Rows the box grows to before it scrolls.</summary>
    public int maxLines { get; }
}

/// <summary>Unity's <c>[Multiline(5)]</c>: a fixed-height multi-line text box.</summary>
[AttributeUsage(AttributeTargets.Field, AllowMultiple = false)]
public sealed class MultilineAttribute : Attribute
{
    /// <summary>Declare Unity's default: three lines.</summary>
    public MultilineAttribute() => lines = 3;

    /// <summary>Declare a box of a given height.</summary>
    /// <param name="lines">Rows the box shows.</param>
    public MultilineAttribute(int lines) => this.lines = lines;

    /// <summary>Rows the box shows.</summary>
    public int lines { get; }
}

/// <summary>Unity's <c>[Min(0)]</c>: a drag value that never goes below the minimum.</summary>
[AttributeUsage(AttributeTargets.Field, AllowMultiple = false)]
public sealed class MinAttribute : Attribute
{
    /// <summary>Declare the floor.</summary>
    /// <param name="min">Lowest value the field's drag value accepts.</param>
    public MinAttribute(float min) => this.min = min;

    /// <summary>Lowest value the field's drag value accepts.</summary>
    public float min { get; }
}
