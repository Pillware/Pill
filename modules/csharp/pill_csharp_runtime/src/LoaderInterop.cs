// Stable unmanaged exports consumed by the Rust C# backend.
//
// Responsibilities:
// - Initializes the managed Engine facade and collectible project loader.
// - Exposes discovered system counts and access declarations to Rust.
// - Dispatches scheduled system calls and polls assembly hot reload.
// - Converts managed exceptions into diagnostics and failure status codes.
//
// Design:
// - Methods are marked UnmanagedCallersOnly and resolved through hostfxr.
// - No exception may cross the native ABI boundary; every exported operation
//   catches failures locally and returns a neutral status where applicable.
// - Each byte payload gets its own named length/copy pair rather than one
//   generic pair taking a payload-kind number. This is deliberate and has
//   been proposed and declined: two exports per payload is the price of a
//   wrong name failing at load, in the host's get_unmanaged_fn, naming the
//   export it could not find. Under a numeric kind the same mistake becomes
//   a runtime mis-dispatch on a live ABI that AOT projects forward by hand.
//   Payloads are added rarely, and each one is an ABI change that bumps
//   INTEROP_CONTRACT_VERSION and is reviewed anyway, so the per-payload cost
//   buys load-time diagnosis at the point where the two languages meet.

using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;

namespace TracyLive.Loader;

// =============================================================================
// Native Access Layout
// =============================================================================

/// <summary>ABI mirror of the Rust scheduler-access record.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct NativeSystemAccess
{
    /// <summary>Low half of the stable 128-bit component ID.</summary>
    public ulong ComponentKey;

    /// <summary>High half of the stable 128-bit component identifier.</summary>
    public ulong ComponentKeyHigh;

    /// <summary>Access mode: zero for read, one for write.</summary>
    public byte Mode;

    /// <summary>What the key names: zero a component, one a resource.</summary>
    /// <remarks>
    /// Both are 128-bit name hashes from one space, so only this tells them
    /// apart. Without it the host resolves a resource access against the
    /// component table and reports the key as unregistered.
    /// </remarks>
    public byte Kind;
}

// =============================================================================
// Unmanaged Entry Points
// =============================================================================

/// <summary>
/// Captures any continuation an <c>[System]</c> method tries to schedule.
/// </summary>
/// <remarks>
/// A system runs inside a scheduled scope that ends when it returns. An
/// <c>await</c> suspends the method, the scope is torn down, and the
/// continuation would resume with no world and possibly a stale chunk pointer.
///
/// Unity solves the same problem by posting continuations back to the main
/// thread, because a Unity object stays valid across the gap. That does not
/// transfer: a continuation here resuming on the right thread but after the
/// scope is gone still has no world. So this context records the violation and
/// drops the continuation - the async method simply never resumes, which is
/// the only outcome that cannot corrupt anything. The dropped state machine is
/// unreferenced afterwards, so it neither leaks nor roots the load context.
///
/// Installed for the duration of one invocation rather than once at startup,
/// because <see cref="SynchronizationContext.Current"/> is per-thread and
/// systems run on native worker threads the managed side never sees created.
/// </remarks>
internal sealed class EcsFrameContext : SynchronizationContext
{
    private readonly string _description;

    internal EcsFrameContext(string description) => _description = description;

    public override void Post(SendOrPostCallback d, object? state) => Report();

    public override void Send(SendOrPostCallback d, object? state) => Report();

    private void Report() => LoaderInterop.ReportAwaitOutsideFrame(_description);
}

/// <summary>Stable native entry points used by the Rust scheduler bridge.</summary>
/// <summary>
/// Marks a <see cref="LoaderInterop"/> method a NativeAOT shipping build must
/// export under <see cref="EntryPoint"/>.
/// </summary>
/// <remarks>
/// NativeAOT exports <c>[UnmanagedCallersOnly]</c> methods only from the root
/// assembly of a library publish - the game project - so the loader's own
/// attributes are compiled out under <c>PILL_AOT</c> and the AOT registry
/// generator emits a forwarder into the project for every method marked with
/// this, in memory, at build time. The development-only reload exports carry
/// no mark: a shipping build never reloads.
/// </remarks>
[AttributeUsage(AttributeTargets.Method)]
public sealed class ShippedExportAttribute : Attribute
{
    /// <summary>Mark one method for export under <paramref name="entryPoint"/>.</summary>
    public ShippedExportAttribute(string entryPoint) => EntryPoint = entryPoint;

    /// <summary>The symbol the Rust host resolves.</summary>
    public string EntryPoint { get; }
}

public static unsafe class LoaderInterop
{
    /// <summary>
    /// Unmanaged ABI contract version shared with the Rust host.
    /// </summary>
    /// <remarks>
    /// Bump when an export signature, the <c>EngineApi</c> layout or an
    /// exchanged struct's shape changes. The runtime copies the table field by
    /// field, so a stale version must fail <see cref="Engine.Bind(EngineApi*)"/>'s
    /// version-and-size check instead of misreading every slot after a new one.
    /// New slots go at the end of the table; no existing slot moves.
    /// </remarks>
    public const uint InteropContractVersion = 19;

    /// <summary>Return the unmanaged ABI contract version for host validation.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_interop_version")]
#endif
    [ShippedExport("pill_interop_version")]
    public static uint InteropVersion() => InteropContractVersion;

    /// <summary>Tell the loader a new project assembly is already on disk.</summary>
    ///
    /// <remarks>
    /// Called by the host after it compiles the project in-process, which is the
    /// one case where the assembly's completeness is known rather than sampled.
    /// It only clears the poll interval; the very next <c>PollReload</c> does
    /// the real work and reports the outcome as usual.
    /// </remarks>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_notify_assembly_replaced")]
#endif
    public static void NotifyAssemblyReplaced()
    {
        try
        {
            _host?.RequestImmediatePoll();
        }
        catch (Exception e)
        {
            Log.Error($"[csharp_runtime] NotifyAssemblyReplaced failed: {e}");
        }
    }

    // The stable runtime owns exactly one active collectible project loader.
    private static ProjectHost? _host;

    /// <summary>Bind the native API and load the initial gameplay assembly.</summary>
    /// <returns>One on success; zero after reporting an initialization error.</returns>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_init")]
#endif
    [ShippedExport("pill_init")]
    public static byte Init(IntPtr api)
    {
        try
        {
            Engine.Bind(api);
            InstallThreadFailureHandlers();
            WaitForManagedDebuggerIfRequested();
            var dir = Environment.GetEnvironmentVariable("ECS_CSHARP_PROJECT_DIR")
                ?? AppContext.BaseDirectory;
            var assembly = Environment.GetEnvironmentVariable("ECS_CSHARP_PROJECT_ASSEMBLY")
                ?? "project_cs.dll";
            _host = new ProjectHost(Path.Combine(dir, assembly));
            _host.Init();
            return 1;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] Init failed: {e}");
            return 0;
        }
    }

    /// <summary>Opt-in variable that makes <see cref="Init"/> wait for a managed debugger.</summary>
    private const string WaitForManagedDebuggerVariable = "PILL_WAIT_FOR_MANAGED_DEBUGGER";

    /// <summary>Longest <see cref="Init"/> waits before continuing without a debugger.</summary>
    private static readonly TimeSpan ManagedDebuggerWaitLimit = TimeSpan.FromMinutes(2);

    /// <summary>
    /// Block until a managed debugger attaches, when the opt-in variable is set.
    /// </summary>
    /// <remarks>
    /// The host boots the runtime itself, so a managed debugger can only attach
    /// to a process that is already running - by which point the gameplay
    /// assembly is loaded and its startups have run. Waiting here, before the
    /// first load, is what lets breakpoints in startup code bind. The wait is
    /// bounded so a forgotten variable costs two minutes, not a hung host.
    /// </remarks>
    private static void WaitForManagedDebuggerIfRequested()
    {
        if (string.IsNullOrEmpty(Environment.GetEnvironmentVariable(WaitForManagedDebuggerVariable))
            || System.Diagnostics.Debugger.IsAttached)
            return;

        Console.Error.WriteLine(
            $"[csharp_runtime] {WaitForManagedDebuggerVariable} is set; waiting up to " +
            $"{ManagedDebuggerWaitLimit.TotalSeconds:0} s for a managed debugger to attach " +
            $"to process {Environment.ProcessId}");
        var waited = System.Diagnostics.Stopwatch.StartNew();
        while (!System.Diagnostics.Debugger.IsAttached && waited.Elapsed < ManagedDebuggerWaitLimit)
            Thread.Sleep(100);

        Console.Error.WriteLine(System.Diagnostics.Debugger.IsAttached
            ? "[csharp_runtime] managed debugger attached; continuing"
            : "[csharp_runtime] no managed debugger attached; continuing without one");
    }

    /// <summary>Whether the process-wide failure handlers are installed.</summary>
    private static bool _threadFailureHandlersInstalled;

    /// <summary>
    /// Name failures that happen on threads the boundary does not wrap.
    /// </summary>
    /// <remarks>
    /// Every export catches before the native ABI, but a thread a script
    /// started has no such wrapper: an unhandled exception there terminates
    /// the process with a .NET stack and no engine context, and an unobserved
    /// task exception is dropped silently. Neither can be prevented from here,
    /// so both are at least attributed to the rule they broke.
    /// </remarks>
    private static void InstallThreadFailureHandlers()
    {
        if (_threadFailureHandlersInstalled)
            return;
        _threadFailureHandlersInstalled = true;
        AppDomain.CurrentDomain.UnhandledException += (_, args) =>
            Console.Error.WriteLine(
                "[csharp_runtime] unhandled exception on a script-owned thread. The ECS API is " +
                "valid only on the thread the scheduler called you on, and only before you " +
                "return." + Environment.NewLine + args.ExceptionObject);
        TaskScheduler.UnobservedTaskException += (_, args) =>
        {
            Console.Error.WriteLine(
                $"[csharp_runtime] unobserved exception on a script-started task: {args.Exception}");
            args.SetObserved();
        };
    }

    /// <summary>Return the number of systems in the active project version.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_system_count")]
#endif
    [ShippedExport("pill_system_count")]
    public static uint SystemCount() => (uint)(_host?.SystemCount ?? 0);

    /// <summary>Return the number of one-shot startup methods.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_startup_count")]
#endif
    [ShippedExport("pill_startup_count")]
    public static uint StartupCount() => (uint)(_host?.StartupCount ?? 0);

    /// <summary>Return whether a system declared the Commands parameter.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_system_uses_commands")]
#endif
    [ShippedExport("pill_system_uses_commands")]
    public static byte SystemUsesCommands(uint systemIndex)
    {
        try
        {
            return _host?.UsesCommands(checked((int)systemIndex)) == true ? (byte)1 : (byte)0;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] SystemUsesCommands failed: {e}");
            return 0;
        }
    }

    /// <summary>Return a system's declared order within its phase.</summary>
    /// <remarks>
    /// A failed read answers <see cref="int.MinValue"/>, which the host refuses
    /// rather than using as a sort key: a system silently placed at the front
    /// of its phase would be a schedule nobody declared.
    /// </remarks>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_system_order")]
#endif
    [ShippedExport("pill_system_order")]
    public static int SystemOrder(uint systemIndex)
    {
        try
        {
            return _host?.GetSystemOrder(checked((int)systemIndex)) ?? 0;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] SystemOrder failed: {e}");
            return int.MinValue;
        }
    }

    /// <summary>Return a system's declared frame phase (0 pre-project, 1 project, 2 post-project).</summary>
    /// <remarks>A failed read answers `0xFF`, which the host refuses by number.</remarks>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_system_run_phase")]
#endif
    [ShippedExport("pill_system_run_phase")]
    public static byte SystemRunPhase(uint systemIndex)
    {
        try
        {
            return _host?.GetSystemRunPhase(checked((int)systemIndex)) ?? 1;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] SystemRunPhase failed: {e}");
            return 0xFF;
        }
    }

    /// <summary>Return whether a system also runs while the editor edits.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_system_run_in_edit_mode")]
#endif
    [ShippedExport("pill_system_run_in_edit_mode")]
    public static byte SystemRunInEditMode(uint systemIndex)
    {
        try
        {
            return _host?.RunsInEditMode(checked((int)systemIndex)) == true ? (byte)1 : (byte)0;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] SystemRunInEditMode failed: {e}");
            return 0;
        }
    }

    /// <summary>Run one startup method before the first frame.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_run_startup")]
#endif
    [ShippedExport("pill_run_startup")]
    public static byte RunStartup(uint startupIndex)
    {
        try
        {
            if (_host is null)
                return 0;
            _host.RunStartup(checked((int)startupIndex));
            return 1;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] startup {startupIndex} failed: {e}");
            return 0;
        }
    }

    /// <summary>Return the UTF-8 JSON component manifest byte count.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_component_manifest_length")]
#endif
    [ShippedExport("pill_component_manifest_length")]
    public static uint ComponentManifestLength() =>
        checked((uint)(_host?.ComponentManifest.Length ?? 0));

    /// <summary>Copy the complete UTF-8 JSON component manifest.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_copy_component_manifest")]
#endif
    [ShippedExport("pill_copy_component_manifest")]
    public static byte CopyComponentManifest(byte* output, uint capacity)
    {
        try
        {
            if (_host is null || output is null || capacity < _host.ComponentManifest.Length)
                return 0;
            _host.ComponentManifest.CopyTo(new Span<byte>(output, checked((int)capacity)));
            return 1;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] CopyComponentManifest failed: {e}");
            return 0;
        }
    }

    /// <summary>Return the number of scheduler accesses for one system.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_system_access_count")]
#endif
    [ShippedExport("pill_system_access_count")]
    public static uint SystemAccessCount(uint systemIndex)
    {
        try
        {
            return (uint)(_host?.GetAccessCount(checked((int)systemIndex)) ?? 0);
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] SystemAccessCount failed: {e}");
            return 0;
        }
    }

    /// <summary>Copy one scheduler access into native-owned output storage.</summary>
    /// <returns>One on success or zero for invalid input/failure.</returns>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_get_system_access")]
#endif
    [ShippedExport("pill_get_system_access")]
    public static byte GetSystemAccess(uint systemIndex, uint accessIndex, NativeSystemAccess* output)
    {
        try
        {
            if (_host is null || output is null)
                return 0;
            var access = _host.GetAccess(checked((int)systemIndex), checked((int)accessIndex));
            output->ComponentKey = access.ComponentKey;
            output->ComponentKeyHigh = access.ComponentKeyHigh;
            output->Mode = access.Mode;
            output->Kind = access.Kind;
            return 1;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] GetSystemAccess failed: {e}");
            return 0;
        }
    }

    /// <summary>Return the UTF-8 byte count of one system's reflected name.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_system_name_length")]
#endif
    [ShippedExport("pill_system_name_length")]
    public static uint SystemNameLength(uint systemIndex)
    {
        try
        {
            return (uint)(_host?.GetSystemNameLength(checked((int)systemIndex)) ?? 0);
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] SystemNameLength failed: {e}");
            return 0;
        }
    }

    /// <summary>Copy one system's reflected UTF-8 name into a caller buffer.</summary>
    /// <returns>One on success or zero for invalid input/failure.</returns>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_copy_system_name")]
#endif
    [ShippedExport("pill_copy_system_name")]
    public static byte CopySystemName(uint systemIndex, byte* output, uint capacity)
    {
        try
        {
            if (_host is null || output is null)
                return 0;
            var bytes = System.Text.Encoding.UTF8.GetBytes(
                _host.GetSystemName(checked((int)systemIndex)));
            if ((uint)bytes.Length > capacity)
                return 0;
            bytes.CopyTo(new Span<byte>(output, checked((int)capacity)));
            return 1;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] CopySystemName failed: {e}");
            return 0;
        }
    }

    /// <summary>Run one managed system selected by its stable discovery index.</summary>
    /// <returns>One on success; zero after recording the failure message for
    /// later retrieval through the error-message exports.</returns>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_run_system")]
#endif
    [ShippedExport("pill_run_system")]
    public static byte RunSystem(uint systemIndex)
    {
        int index = checked((int)systemIndex);
        // A system that awaits would return here with work still outstanding;
        // the context catches the continuation before it escapes the frame.
        SynchronizationContext? previousContext = SynchronizationContext.Current;
        SynchronizationContext.SetSynchronizationContext(
            new EcsFrameContext(_host?.DescribeSystem(index) ?? $"system {systemIndex}"));
        // Per-system allocation attribution. The counter is thread-local and a
        // system runs to completion on one thread, so the delta is exactly what
        // this system allocated, including anything it called.
        long allocatedBefore = GC.GetAllocatedBytesForCurrentThread();
        try
        {
            if (_host is null)
                return 0;
            _host.RunSystem(index);
            return 1;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] system {systemIndex} failed: {e}");
            _host?.SetSystemError(index, e.Message);
            return 0;
        }
        finally
        {
            _host?.RecordAllocation(
                index, GC.GetAllocatedBytesForCurrentThread() - allocatedBefore);
            SynchronizationContext.SetSynchronizationContext(previousContext);
        }
    }

    /// <summary>
    /// Record that a system tried to schedule work past the end of its frame.
    /// </summary>
    /// <remarks>
    /// Routed into the per-system error channel so the violation surfaces the
    /// same way a thrown exception does, naming the system, instead of
    /// vanishing onto a thread pool.
    /// </remarks>
    internal static void ReportAwaitOutsideFrame(string description)
    {
        string message =
            $"{description} scheduled a continuation (await, Task or Timer) that would run " +
            "after the system returned. The ECS API is valid only on the thread the scheduler " +
            "called you on, and only before you return, so the continuation was dropped. Keep " +
            "state in a component and advance it each frame instead.";
        Console.Error.WriteLine($"[csharp_runtime] {message}");
        _host?.SetSystemErrorByName(description, message);
    }

    /// <summary>Return the UTF-8 byte count of one system's last error message.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_system_error_message_length")]
#endif
    [ShippedExport("pill_system_error_message_length")]
    public static uint SystemErrorMessageLength(uint systemIndex)
    {
        try
        {
            return checked((uint)System.Text.Encoding.UTF8.GetByteCount(
                _host?.GetSystemError((int)systemIndex) ?? ""));
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] SystemErrorMessageLength failed: {e}");
            return 0;
        }
    }

    /// <summary>Copy one system's last UTF-8 error message into a caller buffer.</summary>
    /// <returns>One on success or zero for invalid input/failure.</returns>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_copy_system_error_message")]
#endif
    [ShippedExport("pill_copy_system_error_message")]
    public static byte CopySystemErrorMessage(uint systemIndex, byte* output, uint capacity)
    {
        try
        {
            if (_host is null || output is null)
                return 0;
            var bytes = System.Text.Encoding.UTF8.GetBytes(
                _host.GetSystemError((int)systemIndex));
            if ((uint)bytes.Length > capacity)
                return 0;
            bytes.CopyTo(new Span<byte>(output, checked((int)capacity)));
            return 1;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] CopySystemErrorMessage failed: {e}");
            return 0;
        }
    }

    /// <summary>Return the byte count of the pending version's manifest.</summary>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_pending_manifest_length")]
#endif
    public static uint PendingManifestLength() =>
        checked((uint)(_host?.PendingComponentManifest.Length ?? 0));

    /// <summary>Copy the pending version's UTF-8 JSON component manifest.</summary>
    /// <returns>One on success, zero for invalid input or no pending version.</returns>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_copy_pending_manifest")]
#endif
    public static byte CopyPendingManifest(byte* output, uint capacity)
    {
        try
        {
            if (_host is null || output is null ||
                capacity < _host.PendingComponentManifest.Length)
                return 0;
            _host.PendingComponentManifest.CopyTo(new Span<byte>(output, checked((int)capacity)));
            return 1;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[csharp_runtime] CopyPendingManifest failed: {e}");
            return 0;
        }
    }

    /// <summary>Install the pending version; the host accepted its manifest.</summary>
    /// <returns>One on success; zero after reporting a failure.</returns>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_commit_reload")]
#endif
    public static byte CommitReload()
    {
        try
        {
            if (_host is null)
                return 0;
            _host.CommitPendingReload();
            return 1;
        }
        catch (Exception e)
        {
            Log.Error($"[csharp_runtime] CommitReload failed: {e}");
            return 0;
        }
    }

    /// <summary>Discard the pending version; the host refused its manifest.</summary>
    /// <returns>One on success; zero after reporting a failure.</returns>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_abort_reload")]
#endif
    public static byte AbortReload()
    {
        try
        {
            if (_host is null)
                return 0;
            _host.AbortPendingReload();
            return 1;
        }
        catch (Exception e)
        {
            Log.Error($"[csharp_runtime] AbortReload failed: {e}");
            return 0;
        }
    }

    /// <summary>
    /// Poll and apply a behavior-compatible gameplay assembly reload.
    /// </summary>
    /// <returns>
    /// Zero when no reload is due, one after a successful swap, and two when
    /// the loader rejected the new assembly so the old version stays loaded.
    /// </returns>
#if !PILL_AOT
    [UnmanagedCallersOnly(EntryPoint = "pill_poll_reload")]
#endif
    [ShippedExport("pill_poll_reload")]
    public static byte PollReload()
    {
        try
        {
            if (_host is null)
                return (byte)PollStatus.Rejected;
            return _host.PollReload();
        }
        catch (Exception e)
        {
            Log.Error($"[csharp_runtime] PollReload failed: {e}");
            return (byte)PollStatus.Rejected;
        }
    }
}
