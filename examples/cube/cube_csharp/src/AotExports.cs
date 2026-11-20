// NativeAOT export forwarders (AOT shipping posture only).
//
// NativeAOT only emits `[UnmanagedCallersOnly]` exports from the ROOT assembly
// of a library publish - methods in referenced assemblies (here
// `csharp_runtime`, where `LoaderInterop` lives) are compiled in but never
// exported. The gameplay project is the AOT root, so it re-declares every
// loader export here and forwards to `LoaderInterop`'s public implementation.
//
// This file compiles only when `PILL_AOT` is defined (the `EnablePillAot`
// build flag), which is also when `AllowUnsafeBlocks` is switched on for the
// project and `LoaderInterop` drops its own `[UnmanagedCallersOnly]`
// attributes (so these forwarders may call the methods directly). The
// reloadable JIT build never sees this file, so the "no `unsafe` in the
// reloadable assembly" trust boundary is preserved: this shim exists solely in
// the static, non-reloadable shipping image.

#if PILL_AOT
using System;
using System.Runtime.InteropServices;

namespace CubeExample.Loader;

/// <summary>
/// Root-assembly re-exports of the loader ABI, forwarding to
/// <see cref="LoaderInterop"/>. The Rust host resolves these `pill_*`
/// symbols directly from the AOT native library.
/// </summary>
public static unsafe class AotExports
{
    [UnmanagedCallersOnly(EntryPoint = "pill_interop_version")]
    public static uint InteropVersion() => LoaderInterop.InteropVersion();

    [UnmanagedCallersOnly(EntryPoint = "pill_init")]
    public static byte Init(IntPtr api) => LoaderInterop.Init(api);

    [UnmanagedCallersOnly(EntryPoint = "pill_system_count")]
    public static uint SystemCount() => LoaderInterop.SystemCount();

    [UnmanagedCallersOnly(EntryPoint = "pill_startup_count")]
    public static uint StartupCount() => LoaderInterop.StartupCount();

    [UnmanagedCallersOnly(EntryPoint = "pill_system_uses_commands")]
    public static byte SystemUsesCommands(uint systemIndex) =>
        LoaderInterop.SystemUsesCommands(systemIndex);

    [UnmanagedCallersOnly(EntryPoint = "pill_run_startup")]
    public static byte RunStartup(uint startupIndex) =>
        LoaderInterop.RunStartup(startupIndex);

    [UnmanagedCallersOnly(EntryPoint = "pill_component_manifest_length")]
    public static uint ComponentManifestLength() =>
        LoaderInterop.ComponentManifestLength();

    [UnmanagedCallersOnly(EntryPoint = "pill_copy_component_manifest")]
    public static byte CopyComponentManifest(byte* output, uint capacity) =>
        LoaderInterop.CopyComponentManifest(output, capacity);

    [UnmanagedCallersOnly(EntryPoint = "pill_system_access_count")]
    public static uint SystemAccessCount(uint systemIndex) =>
        LoaderInterop.SystemAccessCount(systemIndex);

    [UnmanagedCallersOnly(EntryPoint = "pill_get_system_access")]
    public static byte GetSystemAccess(uint systemIndex, uint accessIndex, NativeSystemAccess* output) =>
        LoaderInterop.GetSystemAccess(systemIndex, accessIndex, output);

    [UnmanagedCallersOnly(EntryPoint = "pill_system_name_length")]
    public static uint SystemNameLength(uint systemIndex) =>
        LoaderInterop.SystemNameLength(systemIndex);

    [UnmanagedCallersOnly(EntryPoint = "pill_copy_system_name")]
    public static byte CopySystemName(uint systemIndex, byte* output, uint capacity) =>
        LoaderInterop.CopySystemName(systemIndex, output, capacity);

    [UnmanagedCallersOnly(EntryPoint = "pill_run_system")]
    public static byte RunSystem(uint systemIndex) => LoaderInterop.RunSystem(systemIndex);

    [UnmanagedCallersOnly(EntryPoint = "pill_system_error_message_length")]
    public static uint SystemErrorMessageLength(uint systemIndex) =>
        LoaderInterop.SystemErrorMessageLength(systemIndex);

    [UnmanagedCallersOnly(EntryPoint = "pill_copy_system_error_message")]
    public static byte CopySystemErrorMessage(uint systemIndex, byte* output, uint capacity) =>
        LoaderInterop.CopySystemErrorMessage(systemIndex, output, capacity);

    [UnmanagedCallersOnly(EntryPoint = "pill_poll_reload")]
    public static byte PollReload() => LoaderInterop.PollReload();
}
#endif
