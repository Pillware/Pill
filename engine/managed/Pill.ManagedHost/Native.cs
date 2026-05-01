using System;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;

namespace Pill.ManagedHost;

internal static partial class Native
{
    private const string RuntimeLibraryName = "pill_runtime";

    static Native()
    {
        NativeLibrary.SetDllImportResolver(
            typeof(Native).Assembly,
            ResolveRuntimeLibrary
        );
    }

    private static IntPtr ResolveRuntimeLibrary(
        string libraryName,
        Assembly assembly,
        DllImportSearchPath? searchPath
    )
    {
        if (libraryName != RuntimeLibraryName)
        {
            return IntPtr.Zero;
        }

        var runDir = Directory.GetCurrentDirectory();

        if (string.IsNullOrWhiteSpace(runDir))
        {
            throw new InvalidOperationException(
                $"PILL_RUN_DIR is not set. " +
                $"CurrentDirectory='{Directory.GetCurrentDirectory()}', " +
                $"AppContext.BaseDirectory='{AppContext.BaseDirectory}', " +
                $"Assembly.Location='{typeof(Native).Assembly.Location}'"
            );
        }

        //var runDir = Directory.GetParent(managedDir)?.FullName
        //    ?? throw new InvalidOperationException(
        //            $"Could not find run dir from managedDir {managedDir}"
        //    );

        var runtimePath = Path.Combine(
                runDir,
                "data",
                $"lib{RuntimeLibraryName}.so"
            );

        if (!File.Exists(runtimePath)) {
            throw new DllNotFoundException(
                $"Could not find runtime native library at {runtimePath}"
            );
        }

        return NativeLibrary.Load(runtimePath);
    }

    [LibraryImport(RuntimeLibraryName)]
    [return: MarshalAs(UnmanagedType.I1)]
    internal static partial bool pill_get_transform(
            ulong entity,
            out Transform Transform
    );

    [LibraryImport(RuntimeLibraryName)]
    [return: MarshalAs(UnmanagedType.I1)]
    internal static partial bool pill_set_transform(
            ulong entity,
            in Transform Transform
    );

    // TODO: add a logger?
}
