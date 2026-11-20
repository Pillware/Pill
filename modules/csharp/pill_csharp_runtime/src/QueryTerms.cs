// Query terms and the descriptor a closed query builds from them.
//
// Responsibilities:
// - Declares the term vocabulary a system signature is written in.
// - Validates a query's terms once, into the descriptor discovery and
//   iteration both read.
//
// Design:
// - A term carries its own metadata as static abstracts, so a typed row can
//   resolve an accessor against the query's ordered term list at JIT time.
// - The descriptor is the only place a term list is validated; both the
//   shape-erased and the typed enumerators read that one array.

using System.Runtime.InteropServices;

namespace TracyLive;

// =============================================================================
// Query Terms and Descriptors
// =============================================================================

/// <summary>Compile-time metadata every query term carries.</summary>
/// <remarks>
/// The static abstract members let a typed row resolve an accessor against the
/// query's own ordered term list instead of searching the joined columns: for
/// a closed value-type query both sides of every comparison are compile-time
/// constants, so the JIT folds every non-matching branch.
/// </remarks>
public interface IQueryTerm
{
    /// <summary>Declared component type; null for padding slots.</summary>
    static abstract Type? DataType { get; }

    /// <summary>Scheduler access the term declares.</summary>
    static abstract QueryAccess Access { get; }

    /// <summary>Whether the term tolerates archetypes without the component.</summary>
    static abstract bool Optional { get; }

    /// <summary>Whether the term exposes the entity column.</summary>
    static abstract bool IsEntity { get; }
}

/// <summary>Required read-only component term.</summary>
public readonly struct Read<T> : IQueryTerm where T : unmanaged
{
    static Type? IQueryTerm.DataType => typeof(T);
    static QueryAccess IQueryTerm.Access => QueryAccess.Read;
    static bool IQueryTerm.Optional => false;
    static bool IQueryTerm.IsEntity => false;
}

/// <summary>Required writable component term.</summary>
public readonly struct Write<T> : IQueryTerm where T : unmanaged
{
    static Type? IQueryTerm.DataType => typeof(T);
    static QueryAccess IQueryTerm.Access => QueryAccess.Write;
    static bool IQueryTerm.Optional => false;
    static bool IQueryTerm.IsEntity => false;
}

/// <summary>Optional read-only component term.</summary>
public readonly struct OptionalRead<T> : IQueryTerm where T : unmanaged
{
    static Type? IQueryTerm.DataType => typeof(T);
    static QueryAccess IQueryTerm.Access => QueryAccess.Read;
    static bool IQueryTerm.Optional => true;
    static bool IQueryTerm.IsEntity => false;
}

/// <summary>Optional writable component term.</summary>
public readonly struct OptionalWrite<T> : IQueryTerm where T : unmanaged
{
    static Type? IQueryTerm.DataType => typeof(T);
    static QueryAccess IQueryTerm.Access => QueryAccess.Write;
    static bool IQueryTerm.Optional => true;
    static bool IQueryTerm.IsEntity => false;
}

/// <summary>Term exposing the current entity without scheduler component access.</summary>
public readonly struct EntityTerm : IQueryTerm
{
    static Type? IQueryTerm.DataType => typeof(Entity);
    static QueryAccess IQueryTerm.Access => QueryAccess.Read;
    static bool IQueryTerm.Optional => false;
    static bool IQueryTerm.IsEntity => true;
}

/// <summary>Padding term filling the unused slots of a closed query's row.</summary>
/// <remarks>Never matches an accessor: it declares no component type.</remarks>
public readonly struct None : IQueryTerm
{
    static Type? IQueryTerm.DataType => null;
    static QueryAccess IQueryTerm.Access => QueryAccess.Read;
    static bool IQueryTerm.Optional => false;
    static bool IQueryTerm.IsEntity => false;
}

/// <summary>Native access mode declared to the Rust scheduler.</summary>
public enum QueryAccess : byte
{
    Read = 0,
    Write = 1,
}

/// <summary>Immutable metadata for one ordered query term.</summary>
public readonly record struct QueryTermDescriptor(
    Type? ComponentType,
    ulong ComponentKey,
    ulong ComponentKeyHigh,
    int ComponentSize,
    QueryAccess Access,
    bool Optional,
    bool IsEntity);

/// <summary>Validated metadata shared by discovery and iteration.</summary>
public sealed class QueryDescriptor
{
    private readonly QueryTermDescriptor[] _terms;

    internal QueryDescriptor(params Type[] termTypes)
    {
        if (termTypes.Length == 0)
            throw new InvalidOperationException("An ECS query must contain at least one term.");

        _terms = termTypes.Select(CreateTerm).ToArray();
        var components = new HashSet<UInt128>();
        var hasEntity = false;
        foreach (var term in _terms)
        {
            if (term.IsEntity)
            {
                if (hasEntity)
                    throw new InvalidOperationException("An ECS query cannot contain EntityTerm more than once.");
                hasEntity = true;
            }
            else if (!components.Add(((UInt128)term.ComponentKeyHigh << 64) | term.ComponentKey))
            {
                throw new InvalidOperationException(
                    $"An ECS query cannot contain component {term.ComponentType!.FullName} more than once.");
            }
        }
    }

    /// <summary>Ordered terms exactly as written in the system signature.</summary>
    public IReadOnlyList<QueryTermDescriptor> Terms => _terms;

    internal QueryTermDescriptor[] TermArray => _terms;

    private static QueryTermDescriptor CreateTerm(Type termType)
    {
        if (termType == typeof(EntityTerm))
            return new QueryTermDescriptor(null, 0, 0, Marshal.SizeOf<Entity>(), QueryAccess.Read, false, true);
        if (!termType.IsGenericType)
            throw UnsupportedTerm(termType);

        Type definition = termType.GetGenericTypeDefinition();
        Type component = termType.GetGenericArguments()[0];
        QueryAccess access;
        bool optional;
        if (definition == typeof(Read<>))
            (access, optional) = (QueryAccess.Read, false);
        else if (definition == typeof(Write<>))
            (access, optional) = (QueryAccess.Write, false);
        else if (definition == typeof(OptionalRead<>))
            (access, optional) = (QueryAccess.Read, true);
        else if (definition == typeof(OptionalWrite<>))
            (access, optional) = (QueryAccess.Write, true);
        else
            throw UnsupportedTerm(termType);

        var stableId = Engine.ComponentStableId(component);
        return new QueryTermDescriptor(
            component,
            stableId.Low,
            stableId.High,
            TracyLive.Loader.NativeLayout.SizeOf(component),
            access,
            optional,
            false);
    }

    private static InvalidOperationException UnsupportedTerm(Type termType) => new(
        $"{termType} is not a query term. Use Read<T>, Write<T>, OptionalRead<T>, " +
        "OptionalWrite<T>, or EntityTerm.");
}

/// <summary>Implemented by every composable closed query type.</summary>
public interface IQueryDescriptor
{
    /// <summary>Return cached ordered query metadata.</summary>
    QueryDescriptor Descriptor { get; }
}
