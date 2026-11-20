// C# component discovery and native manifest generation.
//
// Every unmanaged query component and supported project-declared struct is
// described before Rust registers managed systems or runs startup commands.
// Native-owned mirrors and project-owned structs use the same schema format and
// stable 128-bit identity.

using System.Runtime.InteropServices;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace TracyLive.Loader;

internal sealed record ComponentFieldManifest(
    string Name, int Offset, int Size, string PrimitiveType,
    ComponentFieldManifest[] Fields, ComponentFieldDecoratorManifest[] Decorators,
    string? Default = null);

/// <summary>
/// One presentation decorator a field declared, in the editor's vocabulary.
/// </summary>
/// <remarks>
/// One record shape covers the whole vocabulary instead of a polymorphic
/// union, because the source-generated JSON contract handles a flat record
/// with unused optional members and nothing else. <see cref="Kind"/> picks
/// the decorator; the payload members it reads are documented on each kind.
/// The host refuses a decorator that is missing the payload its kind needs.
/// </remarks>
internal sealed record ComponentFieldDecoratorManifest(
    string Kind, string? Text = null, float? Number = null,
    float? Min = null, float? Max = null, int? MinLines = null, int? MaxLines = null,
    int? Lines = null);

internal sealed record ProjectManifest(
    ulong StableIdLow, ulong StableIdHigh, string FullName,
    int Size, int Alignment, ulong SchemaHash, bool Shared,
    ComponentFieldManifest[] Fields, string Kind, string[] Aliases);

/// <summary>
/// What one manifest entry declares. Resources ride in the same array as
/// components so a generation's whole declaration reaches the host in one
/// transfer and is registered in one transaction.
/// </summary>
internal static class ManifestKinds
{
    /// <summary>Per-entity storage, registered by the host as a column.</summary>
    internal const string Component = "component";

    /// <summary>One value for the whole world, registered as a resource.</summary>
    internal const string Resource = "resource";
}

/// <summary>
/// Source-generated JSON contract for the component manifest. Reflection-based
/// serialization is disabled under NativeAOT, so the manifest must go through
/// the generated context.
/// </summary>
[JsonSourceGenerationOptions(PropertyNamingPolicy = JsonKnownNamingPolicy.SnakeCaseLower)]
[JsonSerializable(typeof(ProjectManifest[]))]
internal partial class ProjectManifestJsonContext : JsonSerializerContext
{
}

internal static class ProjectManifestBuilder
{
    internal static byte[] Build(IEnumerable<ManagedSystem> systems, Assembly? projectAssembly = null)
    {
        IEnumerable<Type> queryComponents = systems
            .SelectMany(system => system.Queries)
            .Where(query => query is not null)
            .SelectMany(query => query!.Terms)
            .Where(term => !term.IsEntity)
            .Select(term => term.ComponentType!);
        // Resources are discovered by their attribute alone, never by use: a
        // resource nothing reads yet still has to be registered, or the first
        // system that adds a Res<T> would fail against a host that never heard
        // of it. The attribute is also what keeps an ordinary project struct
        // from being mistaken for one.
        Type[] declaredResources = projectAssembly is null
            ? []
            : projectAssembly.GetTypes().Where(IsResourceCandidate).ToArray();
        IEnumerable<Type> declaredProjectComponents = projectAssembly is null
            ? []
            : projectAssembly.GetTypes()
                .Where(type => !declaredResources.Contains(type))
                .Where(IsProjectComponentCandidate);
        ProjectManifest[] components = queryComponents
            .Concat(declaredProjectComponents)
            .Distinct()
            .Where(type => !declaredResources.Contains(type))
            .Select(type => Describe(type, ManifestKinds.Component))
            .Concat(declaredResources.Select(type => Describe(type, ManifestKinds.Resource)))
            .OrderBy(component => component.StableIdHigh)
            .ThenBy(component => component.StableIdLow)
            .ToArray();
        return JsonSerializer.SerializeToUtf8Bytes(components, new JsonSerializerOptions
        {
            // NativeAOT disables reflection-based serialization; the generated
            // context is the metadata source. The naming policy is stated here
            // as well as in the context attribute: when a JsonSerializerOptions
            // carries a TypeInfoResolver, its own PropertyNamingPolicy wins.
            TypeInfoResolver = ProjectManifestJsonContext.Default,
            PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        });
    }

    /// <summary>
    /// Include supported unmanaged structs declared by the project even when
    /// they currently appear only in Commands.With/Add/Remove calls. Query
    /// terms remain authoritative for shared runtime component discovery.
    /// </summary>
    private static bool IsProjectComponentCandidate(Type type)
    {
        // A generated marker for a Rust-owned resource is a struct with no
        // fields, which would otherwise pass for an empty component.
        if (!type.IsValueType || type.IsEnum || type.IsPrimitive || type.IsGenericType ||
            type.IsDefined(typeof(CompilerGeneratedAttribute), inherit: false) ||
            ResourceNames.IsNative(type))
            return false;
        try
        {
            ValidateValueType(type, new HashSet<Type>());
            return true;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
    }

    /// <summary>
    /// Whether this type is a project-declared resource: a blittable struct
    /// carrying <see cref="EcsResourceAttribute"/>.
    /// </summary>
    /// <remarks>
    /// A struct that carries the attribute and cannot be described is a
    /// declaration error rather than something to skip quietly, so the layout
    /// failure is allowed to escape here where
    /// <see cref="IsProjectComponentCandidate"/> swallows it - that one is
    /// guessing which structs are components, this one is being told.
    /// </remarks>
    private static bool IsResourceCandidate(Type type)
    {
        if (!type.IsDefined(typeof(EcsResourceAttribute), inherit: false))
            return false;
        if (!type.IsValueType || type.IsEnum || type.IsPrimitive || type.IsGenericType)
            throw new InvalidOperationException(
                $"Resource {type.FullName} must be a plain, non-generic struct.");
        ValidateValueType(type, new HashSet<Type>());
        return true;
    }

    /// <summary>
    /// Whether the native host binds this component itself rather than
    /// registering it as a descriptor byte-level layout.
    /// </summary>
    /// <remarks>
    /// Two sources, both meaning "the host holds a canonical schema for this
    /// type": the <see cref="EcsSharedComponentAttribute"/> a shared component
    /// declares, and membership of the csharp_runtime assembly, which stays
    /// recognised so runtime-owned mirrors need no attribute of their own.
    /// </remarks>
    private static bool IsShared(Type type) =>
        type.IsDefined(typeof(EcsSharedComponentAttribute), inherit: false) ||
        type.Assembly == typeof(Engine).Assembly;

    private static ProjectManifest Describe(Type type, string kind)
    {
        ValidateValueType(type, new HashSet<Type>());
        // A resource may declare its own identity so it can meet a Rust module
        // that names the same resource; a component's identity is always its
        // full type name. Both the id and the manifest's `full_name` come from
        // the same string, because the host recomputes one from the other and
        // refuses the entry if they disagree.
        string identity = kind == ManifestKinds.Resource
            ? ResourceNames.Of(type)
            : type.FullName ?? type.Name;
        StableComponentId id = Engine.StableIdOf(identity);
        ComponentFieldManifest[] fields = DescribeFields(type);
        string schema = SchemaText(type, fields);
        return new ProjectManifest(
            id.Low,
            id.High,
            identity,
            NativeLayout.SizeOf(type),
            NativeLayout.AlignmentOf(type),
            Hash64(schema),
            // A resource has no native binding to be shared with: the host
            // registers it from this declaration and holds no rival schema.
            kind == ManifestKinds.Component && IsShared(type),
            fields,
            kind,
            DeclaredAliases(type, kind));
    }

    /// <summary>
    /// The previous names a type declares, sorted so the manifest is stable
    /// across runs - reflection order is not promised to be.
    /// </summary>
    private static string[] DeclaredAliases(Type type, string kind)
    {
        Type attributeType = kind == ManifestKinds.Resource
            ? typeof(EcsResourceAliasAttribute)
            : typeof(EcsComponentAliasAttribute);
        return type.GetCustomAttributes(attributeType, inherit: false)
            .Select(attribute => attribute switch
            {
                EcsResourceAliasAttribute resource => resource.OldName,
                EcsComponentAliasAttribute component => component.OldName,
                _ => string.Empty,
            })
            .Where(oldName => oldName.Length != 0)
            .OrderBy(oldName => oldName, StringComparer.Ordinal)
            .ToArray();
    }

    private static void ValidateValueType(Type type, HashSet<Type> visiting)
    {
        if (!type.IsValueType || type.IsAutoLayout)
            throw new InvalidOperationException(
                $"Component {type.FullName} must be a sequential or explicit-layout value type.");
        // An explicit layout is readable only through the offsets its fields
        // carry, and its stride only through a declared size; without one the
        // host would allocate a column the managed side cannot stride, so the
        // shape is refused where it is described rather than measured wrongly.
        if (type.IsExplicitLayout && (type.StructLayoutAttribute?.Size ?? 0) <= 0)
            throw new InvalidOperationException(
                $"Component {type.FullName} declares LayoutKind.Explicit without a Size; " +
                "add [StructLayout(LayoutKind.Explicit, Size = N)] or use a sequential layout.");
        if (!visiting.Add(type))
            throw new InvalidOperationException($"Component {type.FullName} has a recursive layout.");
        foreach (var field in type.GetFields(
                     System.Reflection.BindingFlags.Instance |
                     System.Reflection.BindingFlags.Public |
                     System.Reflection.BindingFlags.NonPublic))
        {
            Type fieldType = field.FieldType;
            if (fieldType.IsEnum)
                fieldType = Enum.GetUnderlyingType(fieldType);
            if (fieldType == typeof(bool) || fieldType == typeof(char) ||
                (!fieldType.IsPrimitive && !fieldType.IsValueType) ||
                fieldType.IsPointer || fieldType.IsByRef)
                throw new InvalidOperationException(
                    $"Component field {type.FullName}.{field.Name} has unsupported type {field.FieldType}.");
            if (!fieldType.IsPrimitive)
                ValidateValueType(fieldType, visiting);
        }
        visiting.Remove(type);
    }

    private static ComponentFieldManifest[] DescribeFields(Type type) => type
        .GetFields(System.Reflection.BindingFlags.Instance |
                   System.Reflection.BindingFlags.Public |
                   System.Reflection.BindingFlags.NonPublic)
        .OrderBy(field => NativeLayout.FieldOffset(type, field.Name))
        .Select(field =>
        {
            Type valueType = field.FieldType.IsEnum
                ? Enum.GetUnderlyingType(field.FieldType)
                : field.FieldType;
            bool primitive = valueType.IsPrimitive;
            // A declared default travels with the field as text, and the host
            // refuses one that does not parse as the field's type. Read on
            // nested fields too, so the host's refusal names it rather than
            // the attribute being silently ignored.
            var declaredDefault = (EcsFieldDefaultAttribute?)Attribute.GetCustomAttribute(
                field, typeof(EcsFieldDefaultAttribute), inherit: false);
            // A Handle<T> field is an asset reference: described by the Rust
            // type path of T, so the host records what it references.
            string? handleAsset = HandleAssetPath(valueType);
            string primitiveType = handleAsset is not null
                ? "handle:" + handleAsset
                : primitive ? valueType.FullName! : "struct";
            return new ComponentFieldManifest(
                field.Name,
                NativeLayout.FieldOffset(type, field.Name),
                NativeLayout.SizeOf(valueType),
                primitiveType,
                primitive || handleAsset is not null ? [] : DescribeFields(valueType),
                DescribeDecorators(field),
                declaredDefault?.Literal);
        })
        .ToArray();

    /// <summary>
    /// The Rust type path of the asset a <c>Handle&lt;T&gt;</c> type references,
    /// or <c>null</c> when <paramref name="type"/> is not a handle or its asset
    /// mirror carries no <c>RustType</c> constant.
    /// </summary>
    /// <remarks>
    /// Every generated asset mirror declares <c>public const string RustType</c>;
    /// a handle whose argument lacks it stays an opaque struct, as before.
    /// </remarks>
    private static string? HandleAssetPath(Type type)
    {
        if (!type.IsGenericType || type.GetGenericTypeDefinition() != typeof(TracyLive.Handle<>))
            return null;
        Type asset = type.GetGenericArguments()[0];
        return asset
            .GetField("RustType", BindingFlags.Public | BindingFlags.Static)?
            .GetRawConstantValue() as string;
    }

    /// <summary>
    /// The presentation decorators a field declared, in declaration order.
    /// </summary>
    /// <remarks>
    /// Presentation only: these never enter <see cref="SchemaText"/>, so
    /// decorating a field can never migrate a component's rows. Read on nested
    /// fields too, so a decorated nested leaf reaches the Inspector the same
    /// way a top-level one does.
    /// </remarks>
    private static ComponentFieldDecoratorManifest[] DescribeDecorators(FieldInfo field)
    {
        List<ComponentFieldDecoratorManifest> decorators = [];
        foreach (object attribute in field.GetCustomAttributes(inherit: false))
        {
            switch (attribute)
            {
                case HideInInspectorAttribute:
                    decorators.Add(new ComponentFieldDecoratorManifest("hide_in_inspector"));
                    break;
                case HeaderAttribute header:
                    decorators.Add(new ComponentFieldDecoratorManifest("header", Text: header.header));
                    break;
                case SpaceAttribute space:
                    decorators.Add(new ComponentFieldDecoratorManifest("space", Number: space.height));
                    break;
                case TooltipAttribute tooltip:
                    decorators.Add(new ComponentFieldDecoratorManifest("tooltip", Text: tooltip.tooltip));
                    break;
                case RangeAttribute range:
                    decorators.Add(new ComponentFieldDecoratorManifest(
                        "range", Min: range.min, Max: range.max));
                    break;
                case TextAreaAttribute area:
                    decorators.Add(new ComponentFieldDecoratorManifest(
                        "text_area", MinLines: area.minLines, MaxLines: area.maxLines));
                    break;
                case MultilineAttribute multiline:
                    decorators.Add(new ComponentFieldDecoratorManifest(
                        "multiline", Lines: multiline.lines));
                    break;
                case MinAttribute min:
                    decorators.Add(new ComponentFieldDecoratorManifest("min", Number: min.min));
                    break;
            }
        }
        return decorators.ToArray();
    }

    private static string SchemaText(Type type, ComponentFieldManifest[] fields)
    {
        // Layout only: names, offsets, sizes and types. A field's presentation
        // decorators and its declared default are deliberately absent, so
        // changing either never reads as a schema change.
        var text = new StringBuilder(type.FullName).Append('|')
            .Append(NativeLayout.SizeOf(type)).Append('|').Append(NativeLayout.AlignmentOf(type));
        AppendFields(text, fields);
        return text.ToString();
    }

    private static void AppendFields(StringBuilder text, ComponentFieldManifest[] fields)
    {
        foreach (var field in fields)
        {
            text.Append('|').Append(field.Name).Append('@').Append(field.Offset)
                .Append(':').Append(field.Size).Append(':').Append(field.PrimitiveType);
            AppendFields(text, field.Fields);
        }
    }

    private static ulong Hash64(string value)
    {
        const ulong prime = 0x100000001b3;
        ulong hash = 0xcbf29ce484222325;
        foreach (byte item in Encoding.UTF8.GetBytes(value))
        {
            hash ^= item;
            hash = unchecked(hash * prime);
        }
        return hash;
    }
}
