using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;

namespace Pill.ManagedHost;

public static class ScriptHost
{
    private static readonly Dictionary<ulong, PillScript> Instances = new();
    private static readonly Dictionary<string, Type> ScriptTypes = new();

    private static Assembly? ScriptsAssembly = null;
    private static bool AssemblyResolveInstalled = false; // TODO: later we will have to re-resolve that on hot-reload

    [StructLayout(LayoutKind.Sequential)]
    private struct LoadScriptsArgs
    {
        public IntPtr ScriptAssemblyPtr;
        public int ScriptAssemblyLen;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct EntityArgs
    {
        public ulong Entity;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct UpdateScriptArgs
    {
        public ulong Entity;
        public float Dt;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct CreateScriptArgs
    {
        public ulong Entity;
        public IntPtr ScriptTypePtr;
        public int ScriptTypeLen;
    }

    private static void InstallAssemblyResolver()
    {
        if (AssemblyResolveInstalled)
            return;

        AssemblyResolveInstalled = true;

        AppDomain.CurrentDomain.AssemblyResolve += (_, args) =>
        {
            var requested = new AssemblyName(args.Name);

            if (requested.Name == "Pill.ManagedHost")
            {
                return typeof(ScriptHost).Assembly;
            }

            return null;
        };
    }

    public static int LoadScripts(IntPtr args, int sizeBytes)
    {
        Engine.Log("ScriptHost.LoadScripts");

        InstallAssemblyResolver();

        var data = Marshal.PtrToStructure<LoadScriptsArgs>(args);
        var assemblyName = Marshal.PtrToStringUTF8(data.ScriptAssemblyPtr, data.ScriptAssemblyLen);

        if (string.IsNullOrWhiteSpace(assemblyName))
        {
            Engine.Log($"LoadScripts failed: assembly name was null");
            return -1;
        }

        if (!File.Exists(assemblyName))
        {
            Engine.Log($"LoadScripts failed: file does not exist {assemblyName}");
            return -2;
        }

        ScriptTypes.Clear();

        ScriptsAssembly = Assembly.LoadFrom(assemblyName);

        foreach (var type in ScriptsAssembly.GetTypes())
        {
            Engine.Log(
                $"Found type: {type.FullName}, " +
                $"abstract={type.IsAbstract}, " +
                $"base={type.BaseType?.FullName}, " +
                $"assignableToPillScript={typeof(PillScript).IsAssignableFrom(type)}"
            );
        }

        foreach (var type in ScriptsAssembly.GetTypes().Where(t => !t.IsAbstract && typeof(PillScript).IsAssignableFrom(t)))
        {
            ScriptTypes[type.FullName!] = type;
            Engine.Log($"Registered script type: {type.FullName}");
        }

        return 0;
    }

    public static int Shutdown(IntPtr args, int sizeBytes)
    {
        Engine.Log("ScriptHost.Shutdown");

        foreach (var instance in Instances.Values)
        {
            try
            {
                instance.OnDestroy();
            }
            catch (Exception e)
            {
                Engine.Log($"Exception in OnDestroy: {e}");
            }
        }

        Instances.Clear();
        ScriptTypes.Clear();
        ScriptsAssembly = null;

        return 0;
    }

    public static int CreateScript(IntPtr args, int sizeBytes)
    {
        var data = Marshal.PtrToStructure<CreateScriptArgs>(args);
        var scriptType = Marshal.PtrToStringUTF8(data.ScriptTypePtr, data.ScriptTypeLen);

        if (scriptType is null)
        {
            Engine.Log($"CreateScript failed: script type was null");
            return -1;
        }

        if (!ScriptTypes.TryGetValue(scriptType, out var type))
        {
            Engine.Log($"CreateScript failed: unknown script type: {scriptType}");
            return -2;
        }

        if (Instances.ContainsKey(data.Entity))
        {
            Engine.Log($"CreateScript ignored: entity={data.Entity} already has the script instance");
            return 0;
        }

        var instance = (PillScript)Activator.CreateInstance(type)!;
        instance.Entity = data.Entity;

        Instances[data.Entity] = instance;

        Engine.Log($"Created script '{scriptType}' for entity={data.Entity}");

        return 0;
    }

    public static int StartScript(IntPtr args, int sizeBytes)
    {
        var data = Marshal.PtrToStructure<EntityArgs>(args);

        if (!Instances.TryGetValue(data.Entity, out var instance))
        {
            Engine.Log($"StartScript failed: no script for entity: {data.Entity}");
            return -1;
        }

        try
        {
            instance.OnStart();
            return 0;
        }
        catch (Exception e)
        {
            Engine.Log($"Exception in OnStart entity={data.Entity}: {e}");
            return -2;
        }
    }

    // TODO: could refactor to batch-update them
    public static int UpdateScript(IntPtr args, int sizeBytes)
    {
        var data = Marshal.PtrToStructure<UpdateScriptArgs>(args);

        if (!Instances.TryGetValue(data.Entity, out var instance))
        {
            Engine.Log($"UpdateScript failed: no script for entity: {data.Entity}");
            return -1;
        }

        try
        {
            instance.OnUpdate(data.Dt);
            return 0;
        }
        catch (Exception e)
        {
            Engine.Log($"Exception in OnUpdate entity={data.Entity}: {e}");
            return -2;
        }
    }

    public static int DestroyScript(IntPtr args, int sizeBytes)
    {
        var data = Marshal.PtrToStructure<EntityArgs>(args);

        if (!Instances.Remove(data.Entity, out var instance))
        {
            Engine.Log($"DestroyScript ignored: no script for entity: {data.Entity}");
            return 0;
        }

        try
        {
            instance.OnDestroy();
            return 0;
        }
        catch (Exception e)
        {
            Engine.Log($"Exception in OnDestroy entity={data.Entity}: {e}");
            return -1;
        }

    }
}
