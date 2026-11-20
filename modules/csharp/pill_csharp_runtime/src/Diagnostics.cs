using System;

namespace TracyLive;

/// <summary>Managed logging routed into the engine's tracing pipeline.</summary>
public static class Log
{
    public static void Trace(string message, string target = "csharp") => Engine.Log(0, target, message);
    public static void Debug(string message, string target = "csharp") => Engine.Log(1, target, message);
    public static void Info(string message, string target = "csharp") => Engine.Log(2, target, message);
    public static void Warn(string message, string target = "csharp") => Engine.Log(3, target, message);
    public static void Error(string message, string target = "csharp") => Engine.Log(4, target, message);
}

/// <summary>Managed dynamic Tracy zones.</summary>
public static class Profiler
{
    /// <summary>
    /// Opens a zone that closes when the returned guard is disposed, or when
    /// the managed invocation that opened it returns - whichever comes first,
    /// so a leaked guard cannot outlive its system.
    /// </summary>
    public static IDisposable Zone(string name) => new ZoneGuard(Engine.BeginZone(name));

    private sealed class ZoneGuard : IDisposable
    {
        private ulong _token;
        internal ZoneGuard(ulong token) => _token = token;
        public void Dispose()
        {
            if (_token == 0)
                return;
            Engine.EndZone(_token);
            _token = 0;
        }
    }
}
