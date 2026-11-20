// Native status bytes: one named vocabulary per ABI slot.
//
// Responsibilities:
// - Name every status a managed call can receive back from a native slot.
// - Keep the documented numbers visible, because the bytes are the contract.
//
// Design:
// - The Rust side is the source of truth: each enum names the slot its
//   numbers come from, and the members repeat that slot's documented wording.
// - Digits shared by several slots live in `StatusCode`; digits that mean
//   different things in different slots stay in separate types, so a message
//   copied between two interpreters cannot compile against the wrong one.
// - Call sites cast the byte once, at the ABI boundary, and switch on the
//   enum from there on.

namespace TracyLive;

/// <summary>Status digits whose meaning is the same in more than one slot.</summary>
internal static class StatusCode
{
    /// <summary>No managed invocation is running on the calling thread.</summary>
    internal const byte NoInvocation = 3;

    /// <summary>The caller passed no output buffer.</summary>
    internal const byte NullOutput = 5;
}

/// <summary>
/// Status of a component chunk request (<c>GetComponentChunk</c>,
/// <c>GetArchetypeChunk</c>, <c>GetEntityChunk</c>).
/// </summary>
internal enum QueryStatus : byte
{
    /// <summary>Nothing to serve: the index is past the end, or no column matched.</summary>
    End = 0,

    /// <summary>The output buffer holds a complete chunk.</summary>
    Chunk = 1,

    /// <summary>The identity names no component this host registered.</summary>
    NotRegistered = 2,

    /// <summary>No managed system is running on this thread.</summary>
    NoInvocation = StatusCode.NoInvocation,

    /// <summary>The running system did not declare the requested access.</summary>
    AccessNotDeclared = 4,

    /// <summary>The caller passed no output buffer.</summary>
    NullOutput = StatusCode.NullOutput,
}

/// <summary>Status of a deferred command (<c>ReserveEntity</c>, <c>Queue*</c>).</summary>
internal enum CommandStatus : byte
{
    /// <summary>The command was queued.</summary>
    Ok = 1,

    /// <summary>The identity names no component this host registered.</summary>
    NotRegistered = 2,

    /// <summary>Commands was used outside an active startup or scheduled system.</summary>
    NoInvocation = StatusCode.NoInvocation,

    /// <summary>The running system did not declare a Commands parameter.</summary>
    CommandsNotDeclared = 4,

    /// <summary>The entity handle is stale, invalid, or was not reserved here.</summary>
    StaleEntity = 5,

    /// <summary>The component list or its ABI layout is invalid.</summary>
    InvalidComponentList = 6,
}

/// <summary>Status of a <c>Res&lt;T&gt;</c>/<c>ResMut&lt;T&gt;</c> view request.</summary>
internal enum ResourceViewStatus : byte
{
    /// <summary>The view was filled.</summary>
    Ok = 0,

    /// <summary>The identity names no registered resource.</summary>
    NotRegistered = 1,

    /// <summary>The running system did not declare the requested access.</summary>
    AccessNotDeclared = 2,

    /// <summary>No managed system is running on this thread.</summary>
    NoInvocation = StatusCode.NoInvocation,

    /// <summary>The world holds no value for the resource yet.</summary>
    NoValue = 4,

    /// <summary>The caller passed no output buffer.</summary>
    NullOutput = StatusCode.NullOutput,
}

/// <summary>
/// Status of a Rust-owned resource address request (the mirrored-call slot).
/// </summary>
/// <remarks>
/// The first member differs from <see cref="ResourceViewStatus"/> on purpose:
/// this slot can only be reached from inside an invocation, so it reports a
/// missing value where the view slot reports a missing registration.
/// </remarks>
internal enum NativeResourceStatus : byte
{
    /// <summary>The address was written.</summary>
    Ok = 0,

    /// <summary>The world holds no value under that name.</summary>
    NoValue = 1,

    /// <summary>The running system did not declare the requested access.</summary>
    AccessNotDeclared = 2,

    /// <summary>No managed invocation is running on this thread.</summary>
    NoInvocation = StatusCode.NoInvocation,
}

/// <summary>Status of <c>EntityCount</c>.</summary>
internal enum EntityCountStatus : byte
{
    /// <summary>The count was written.</summary>
    Ok = 0,

    /// <summary>No managed system is scheduled on this thread.</summary>
    NoInvocation = StatusCode.NoInvocation,

    /// <summary>The caller passed no output buffer.</summary>
    NullOutput = StatusCode.NullOutput,
}

/// <summary>Status of <c>EntityStableId</c>.</summary>
internal enum StableIdStatus : byte
{
    /// <summary>The stable id was written.</summary>
    Ok = 0,

    /// <summary>The entity is not live in the active world.</summary>
    NotLive = 1,

    /// <summary>No managed system is scheduled on this thread.</summary>
    NoInvocation = StatusCode.NoInvocation,

    /// <summary>One of the output halves was missing.</summary>
    NullOutput = StatusCode.NullOutput,
}
