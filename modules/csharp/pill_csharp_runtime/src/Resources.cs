// Managed access to engine resources.
//
// Responsibilities:
// - Declares the [EcsResource] attribute that marks a project's singleton
//   state, and the Res<T>/ResMut<T> system parameters that reach it.
// - Resolves a resource's bytes through the native table, once per access,
//   under the same scope and access-declaration rules queries obey.
//
// Design:
// - A resource is a blittable struct, exactly like a component, and reaches
//   the host through the same manifest: the identity is the hash of the full
//   type name, so the managed declaration and the native registration agree
//   without anything being generated.
// - Res<T> and ResMut<T> carry no state. Every `.Value` re-asks the host for
//   the pointer rather than caching one, which is what makes a stale borrow
//   impossible: there is no borrow to keep. A resource is touched a handful of
//   times per system, not once per row, so the per-access call costs nothing
//   the query data plane would have had to pay.
// - The scheduler learns about resource access the same way it learns about
//   component access: one reflected entry per declared parameter, so two
//   systems that write one resource are serialized rather than raced.

using System.Diagnostics;
using System.Runtime.CompilerServices;

namespace TracyLive;

// =============================================================================
// Declaration
// =============================================================================

/// <summary>
/// Marks a blittable struct as an engine resource: one value for the whole
/// world, reached by systems through <see cref="Res{T}"/> and
/// <see cref="ResMut{T}"/> rather than attached to an entity.
/// </summary>
/// <remarks>
/// This is where per-frame and per-session state belongs. A mutable static
/// looks like the same thing and is not: the scheduler cannot see a static, so
/// two systems sharing one race rather than being ordered, and every hot reload
/// loads the assembly into a fresh context and resets it. A resource is visible
/// to the scheduler and outlives the assembly that declared it.
///
/// The identity across the boundary is the type's full name by default, so it
/// must be unique in the process and is what a save file records. Pass a
/// <see cref="Name"/> to declare it instead: a resource a Rust module also
/// declares has to agree with that module's `Resource::shared_name`, and
/// renaming a C# type to match would be the tail wagging the dog.
/// </remarks>
[AttributeUsage(AttributeTargets.Struct)]
public sealed class EcsResourceAttribute : Attribute
{
    /// <summary>Declare the resource with the default identity: its full name.</summary>
    public EcsResourceAttribute()
    {
    }

    /// <summary>Declare the resource under an explicit cross-language name.</summary>
    /// <param name="name">
    /// The identity every artifact writes down, namespaced to stay unique -
    /// <c>"pill_spline::SplineSettings"</c>, not <c>"Settings"</c>.
    /// </param>
    public EcsResourceAttribute(string name) => Name = name;

    /// <summary>The declared identity, or null to use the type's full name.</summary>
    public string? Name { get; }
}

/// <summary>
/// Declares a name this resource used to be known by, so a rename carries its
/// stored value instead of reading as a disappearance.
/// </summary>
/// <remarks>
/// The resource twin of <see cref="EcsComponentAliasAttribute"/>: the host
/// resolves the old name to the registration that answered to it, moves the
/// value and the claims onto this type's new identity, and drops the old
/// declaration. One hop only, and the alias must name a registration that is
/// still live when the manifest arrives.
/// </remarks>
[AttributeUsage(AttributeTargets.Struct, AllowMultiple = true)]
public sealed class EcsResourceAliasAttribute : Attribute
{
    /// <summary>Declare one previous name of this resource.</summary>
    /// <param name="oldName">
    /// The declared identity the resource was registered under - the explicit
    /// <see cref="EcsResourceAttribute.Name"/> when it had one, its full type
    /// name otherwise.
    /// </param>
    public EcsResourceAliasAttribute(string oldName) => OldName = oldName;

    /// <summary>The previous name this declaration claims.</summary>
    public string OldName { get; }
}

/// <summary>Compile-time metadata every resource system parameter carries.</summary>
/// <remarks>
/// Static abstract members, like <see cref="IQueryTerm"/>, so system discovery
/// can read the declaration off the parameter type without instantiating it.
/// </remarks>
public interface IResourceParameter
{
    /// <summary>The resource struct this parameter reaches.</summary>
    static abstract Type ResourceType { get; }

    /// <summary>Access the scheduler must grant for this parameter.</summary>
    static abstract QueryAccess Access { get; }
}

// =============================================================================
// System Parameters
// =============================================================================

/// <summary>Read-only access to one engine resource.</summary>
/// <remarks>
/// Declaring <c>Res&lt;T&gt;</c> lets the scheduler run this system beside any
/// other reader of the same resource, and never beside a writer of it.
/// </remarks>
public readonly struct Res<T> : IResourceParameter where T : unmanaged
{
    static Type IResourceParameter.ResourceType => typeof(T);
    static QueryAccess IResourceParameter.Access => QueryAccess.Read;

    /// <summary>Borrow the resource's current value.</summary>
    /// <exception cref="InvalidOperationException">
    /// No ECS system is running on this thread, the system did not declare
    /// this resource, or the world holds no value for it yet.
    /// </exception>
    public ref readonly T Value
    {
        [MethodImpl(MethodImplOptions.AggressiveInlining)]
        get => ref ResourceAccess.Borrow<T>(QueryAccess.Read);
    }
}

/// <summary>Read-write access to one engine resource.</summary>
/// <remarks>
/// A writer is serialized against every other system that declares the same
/// resource, reader or writer, which is exactly the guarantee a static cannot
/// give. Writing through <see cref="Value"/> stamps the resource's change tick.
/// </remarks>
public readonly struct ResMut<T> : IResourceParameter where T : unmanaged
{
    static Type IResourceParameter.ResourceType => typeof(T);
    static QueryAccess IResourceParameter.Access => QueryAccess.Write;

    /// <summary>Borrow the resource's value for reading and writing.</summary>
    /// <exception cref="InvalidOperationException">
    /// No ECS system is running on this thread, the system declared only read
    /// access, or the world holds no value for this resource yet.
    /// </exception>
    public ref T Value
    {
        [MethodImpl(MethodImplOptions.AggressiveInlining)]
        get => ref ResourceAccess.Borrow<T>(QueryAccess.Write);
    }
}

// =============================================================================
// Native Resolution
// =============================================================================

/// <summary>
/// Turns a resource type plus a requested access into a reference into the
/// engine's own storage.
/// </summary>
internal static unsafe class ResourceAccess
{
    /// <summary>
    /// Resolve one resource's storage and return a reference to it.
    /// </summary>
    /// <remarks>
    /// The pointer is re-fetched on every call rather than cached, so it cannot
    /// outlive the invocation that was allowed to hold it. The scope token is
    /// still validated in debug builds, because a view handed to one invocation
    /// could otherwise be read by the next through a captured local.
    /// </remarks>
    internal static ref T Borrow<T>(QueryAccess access) where T : unmanaged
    {
        if (ResourceTypeMetadata<T>.IsNative)
            throw new InvalidOperationException(
                $"{typeof(T).Name} is a Rust resource ({ResourceTypeMetadata<T>.Name}); its value stays " +
                "in Rust. Call its generated methods on the Res/ResMut parameter instead of .Value.");
        StableComponentId id = ResourceTypeMetadata<T>.StableId;
        NativeResourceView view;
        byte status = Engine.GetResourceView(id, (byte)access, &view);
        if (status != 0)
            throw Failure<T>(status, access);
        Engine.ValidateResourceScope(view.ScopeToken, ResourceTypeMetadata<T>.Name);
        // The host serves the live allocation, so a length that disagrees with
        // the managed struct means the manifest and this build have drifted -
        // reading on would stride into the bytes after the value.
        if (view.Length != (uint)ResourceTypeMetadata<T>.Size)
            throw new InvalidOperationException(
                $"Resource {ResourceTypeMetadata<T>.Name} is {view.Length} bytes in the engine " +
                $"and {ResourceTypeMetadata<T>.Size} bytes in this assembly.");
        return ref Unsafe.AsRef<T>((void*)view.Data);
    }

    /// <summary>Describe a refused resource request in the caller's terms.</summary>
    internal static InvalidOperationException Failure<T>(byte status, QueryAccess access)
        where T : unmanaged
    {
        string name = ResourceTypeMetadata<T>.Name;
        return ((ResourceViewStatus)status) switch
        {
            ResourceViewStatus.NotRegistered => new InvalidOperationException(
                $"Resource {name} is not registered with the engine. Mark it [EcsResource] " +
                "so the manifest declares it."),
            ResourceViewStatus.AccessNotDeclared => new InvalidOperationException(
                $"This system did not declare {access} access to resource {name}. Add a " +
                $"{(access == QueryAccess.Write ? "ResMut" : "Res")}<{typeof(T).Name}> parameter."),
            ResourceViewStatus.NoInvocation => new InvalidOperationException(
                $"No ECS system is running on this thread, so resource {name} cannot be reached."),
            ResourceViewStatus.NoValue => new InvalidOperationException(
                $"The world holds no value for resource {name} yet."),
            _ => new InvalidOperationException(
                $"Resource {name} could not be reached (status {status})."),
        };
    }
}

/// <summary>Per-resource-type constants resolved once by the JIT.</summary>
/// <remarks>
/// Field order is load-bearing: static initialisers run in declaration order,
/// so the name has to be resolved before the identity hashed from it.
/// </remarks>
internal static class ResourceTypeMetadata<T> where T : unmanaged
{
    /// <summary>
    /// The resource's identity: its declared name, or its full type name.
    /// </summary>
    /// <remarks>
    /// Also what every diagnostic about this resource says, because it is the
    /// name the host, a save file and any other artifact all know it by - the
    /// C# type name would only be recognisable from inside this assembly.
    /// </remarks>
    internal static readonly string Name = ResourceNames.Of(typeof(T));

    /// <summary>Stable 128-bit identity, hashed from the declared name.</summary>
    internal static readonly StableComponentId StableId = Engine.StableIdOf(Name);

    /// <summary>Native size of the resource's layout, in bytes.</summary>
    internal static readonly int Size = TracyLive.Loader.NativeLayout.SizeOf(typeof(T));

    /// <summary>Whether the type is a marker for a Rust-owned resource.</summary>
    internal static readonly bool IsNative = ResourceNames.IsNative(typeof(T));
}

/// <summary>Resolves one resource type to its declared identity.</summary>
/// <remarks>
/// Shared by the manifest builder and the access path so the name a resource is
/// registered under and the name it is asked for can never diverge.
/// </remarks>
internal static class ResourceNames
{
    /// <summary>The declared name of a resource type, or its full type name.</summary>
    /// <remarks>
    /// A marker for a Rust-owned resource (<see cref="NativeResourceAttribute"/>)
    /// is named by the Rust type's shared name, which is what the engine keys
    /// the resource by.
    /// </remarks>
    internal static string Of(Type type)
    {
        var native = (NativeResourceAttribute?)Attribute.GetCustomAttribute(
            type, typeof(NativeResourceAttribute), inherit: false);
        if (native is not null)
            return native.SharedName;
        var declaration = (EcsResourceAttribute?)Attribute.GetCustomAttribute(
            type, typeof(EcsResourceAttribute), inherit: false);
        return declaration?.Name ?? type.FullName ?? type.Name;
    }

    /// <summary>Whether a type is a marker for a Rust-owned resource.</summary>
    internal static bool IsNative(Type type) =>
        type.IsDefined(typeof(NativeResourceAttribute), inherit: false);

    /// <summary>The access-list kind a resource parameter of this type declares.</summary>
    internal static byte KindOf(Type type) =>
        IsNative(type) ? Loader.ManagedAccess.NativeResourceKind : Loader.ManagedAccess.ResourceKind;
}
