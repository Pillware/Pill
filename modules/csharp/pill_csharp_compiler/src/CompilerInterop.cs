// Stable unmanaged exports the Rust host calls to compile a gameplay project.
//
// Responsibilities:
// - Expose the in-process Roslyn compile through the hostfxr entry-point ABI.
// - Hand the last compile's diagnostics back as UTF-8 in two steps.
// - Convert every managed failure into a status code, never an escaping exception.
//
// Design:
// The shape deliberately mirrors csharp_runtime's LoaderInterop: methods are
// UnmanagedCallersOnly and resolved by type and method name through hostfxr, and
// a string crosses the boundary as a length call followed by a copy into a
// caller-owned buffer. Reusing that idiom means the host's existing helpers and
// error handling apply unchanged.
//
// This assembly is loaded only in the hot-reload posture. A shipping build has
// no compiler and never resolves any of these.

using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;

namespace TracyLive.Compiler;

/// <summary>Native entry points for the in-process project compiler.</summary>
public static unsafe class CompilerInterop
{
    /// <summary>Version of the export contract below.</summary>
    ///
    /// <remarks>
    /// Checked by the host before any other export is resolved, exactly as the
    /// managed runtime's interop version is: a compiler assembly left over from
    /// an older checkout would otherwise be called through signatures it does
    /// not have. Bump this whenever an export's signature or meaning changes.
    /// </remarks>
    private const uint AbiVersion = 1;

    /// <summary>UTF-8 diagnostics from the last <see cref="Compile"/> call.</summary>
    private static byte[] _diagnostics = Array.Empty<byte>();

    /// <summary>Report the export contract version this assembly implements.</summary>
    [UnmanagedCallersOnly]
    public static uint CompilerAbiVersion() => AbiVersion;

    /// <summary>
    /// Start a background warmup: parse the captured command line, read the
    /// reference set, load the analyzers and run one compilation without
    /// emitting.
    /// </summary>
    ///
    /// <returns>One when the warmup was queued, zero when the argument was unusable.</returns>
    ///
    /// <remarks>
    /// Returns as soon as the work is queued rather than when it finishes. The
    /// cost being warmed - JITting Roslyn, and first-touching six megabytes of
    /// reference metadata - is close to a second, and paying it on the host's
    /// startup thread would trade the reload latency this path removes for
    /// startup latency instead. A reload that arrives mid-warmup simply waits on
    /// the compiler's own lock and then finds the caches populated.
    /// </remarks>
    [UnmanagedCallersOnly]
    public static byte Warmup(byte* responseFilePath)
    {
        try
        {
            string? path = ReadUtf8(responseFilePath);
            if (path is null)
            {
                return 0;
            }
            ThreadPool.QueueUserWorkItem(static state =>
            {
                CompileOutcome outcome = ProjectCompiler.Warmup((string)state!);
                // A warmup failure is not fatal - the host falls back to a full
                // build - but it is worth saying once, because it means every
                // reload from here on will take the slow path.
                if (outcome.Status == CompileStatus.Unavailable)
                {
                    Console.Error.WriteLine(
                        $"[csharp_compiler] warmup failed: {outcome.Detail}");
                }
            }, path);
            return 1;
        }
        catch (Exception failure)
        {
            Console.Error.WriteLine($"[csharp_compiler] Warmup failed: {failure}");
            return 0;
        }
    }

    /// <summary>Compile the project described by a captured command line.</summary>
    ///
    /// <param name="responseFilePath">UTF-8 path to the captured csc arguments.</param>
    /// <param name="outputAssemblyPath">UTF-8 path the assembly is written to.</param>
    ///
    /// <returns>
    /// Zero when the assembly was written, one when the project has errors (see
    /// <see cref="DiagnosticsLength"/>), two when the fast path could not run and
    /// the caller must fall back to a full build.
    /// </returns>
    [UnmanagedCallersOnly]
    public static int Compile(byte* responseFilePath, byte* outputAssemblyPath)
    {
        try
        {
            string? arguments = ReadUtf8(responseFilePath);
            string? output = ReadUtf8(outputAssemblyPath);
            if (arguments is null || output is null)
            {
                SetDiagnostics("the host passed a null path to Compile");
                return (int)CompileStatus.Unavailable;
            }
            CompileOutcome outcome = ProjectCompiler.Compile(arguments, output);
            SetDiagnostics(outcome.Detail);
            return (int)outcome.Status;
        }
        catch (Exception failure)
        {
            // No exception may cross the native boundary. Reporting Unavailable
            // rather than Failed matters: it sends the host to a full build
            // instead of showing the developer a compile error in their code.
            SetDiagnostics(failure.ToString());
            return (int)CompileStatus.Unavailable;
        }
    }

    /// <summary>Byte count of the last compile's UTF-8 diagnostics.</summary>
    [UnmanagedCallersOnly]
    public static uint DiagnosticsLength() => (uint)_diagnostics.Length;

    /// <summary>Copy the last compile's diagnostics into a caller buffer.</summary>
    ///
    /// <returns>One on success, zero when the buffer is null or too small.</returns>
    [UnmanagedCallersOnly]
    public static byte CopyDiagnostics(byte* output, uint capacity)
    {
        try
        {
            byte[] diagnostics = _diagnostics;
            if (output is null || capacity < (uint)diagnostics.Length)
            {
                return 0;
            }
            diagnostics.CopyTo(new Span<byte>(output, checked((int)capacity)));
            return 1;
        }
        catch (Exception failure)
        {
            Console.Error.WriteLine($"[csharp_compiler] CopyDiagnostics failed: {failure}");
            return 0;
        }
    }

    /// <summary>Store one message as the diagnostics the host may now read.</summary>
    private static void SetDiagnostics(string message) =>
        _diagnostics = string.IsNullOrEmpty(message)
            ? Array.Empty<byte>()
            : Encoding.UTF8.GetBytes(message);

    /// <summary>Decode a null-terminated UTF-8 path from the host.</summary>
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static string? ReadUtf8(byte* value) =>
        value is null ? null : Marshal.PtrToStringUTF8((IntPtr)value);
}
