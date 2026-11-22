// The audio scene, managed: one sound from res/, a listener and a playing source.
//
// Responsibilities
// - Load `res/audio/blip.wav` as a `Sound` and spawn a listener and an emitter.
// - Ask the emitter to play on its first frame by setting the component's fields.
//
// Design
// `pill_audio` owns the output device and the system that plays; a project only
// decides what exists. The source is a plain struct: set `Sound` and
// `PlayOnAwake`, and the extension's system turns that flag into a play request
// on its next run. `Enabled` on the listener is set explicitly because a C#
// `default` struct has every field false, while the Rust default is true.
//
// The renderer stays at its default in this project's settings: a managed
// project cannot register a component, and `TransformComponent` - which the
// audio system reads both positions from - comes from the renderer's data
// crate. A headless run still opens no window and no GPU device.

using System.Runtime.InteropServices;
using pill_audio;
using pill_audio.audio_listener_component;
using pill_audio.audio_source_component;
using pill_rendering;
using TracyLive;

namespace AudioSceneCSharp;

/// <summary>Builds the scene once, on the project's first load.</summary>
public static class AudioSceneStartup
{
    /// <summary>Where the example's sound lives, below the project's <c>res</c>.</summary>
    private const string SoundPath = "audio/blip.wav";

    /// <summary>Imports the sound and spawns the listener and the emitter.</summary>
    [Startup]
    public static void Start(Commands commands, ResMut<AssetManager> assets)
    {
        Handle<Sound> sound = assets.Load<Sound>(SoundPath);

        AudioListenerComponent listener = default;
        listener.Enabled = 1;
        commands.CreateEntity()
            .With(listener)
            .With(TransformComponent.Identity)
            .Build();

        AudioSourceComponent source = default;
        source.Sound = sound;
        source.PlayOnAwake = 1;
        commands.CreateEntity()
            .With(source)
            .With(TransformComponent.At(1.0f, 0.0f, 0.0f, 1.0f))
            .Build();
    }
}

/// <summary>Whether the playback line has been written, kept where a reload cannot lose it.</summary>
/// <remarks>
/// A resource rather than a static: the analyzer refuses mutable static state in
/// a type that declares systems, because a reload replaces the loaded assembly
/// and would restart it.
/// </remarks>
[EcsResource]
[StructLayout(LayoutKind.Sequential)]
public struct PlaybackReportState
{
    /// <summary>Non-zero once the playing line has been written.</summary>
    public byte Reported;
}

/// <summary>Reports once when the extension has a sink playing the sound.</summary>
public static class PlaybackReport
{
    /// <summary>Logs the frame a source reports playback, so a headless run can be checked.</summary>
    [System]
    public static void Run(
        ResMut<PlaybackReportState> report,
        Query<Read<AudioSourceComponent>> query)
    {
        if (report.Value.Reported != 0)
        {
            return;
        }
        foreach (var row in query.Rows())
        {
            if (row.AudioSourceComponent.IsPlaying == 0)
            {
                continue;
            }
            report.Value.Reported = 1;
            System.Console.WriteLine("[audio_scene_csharp] playing");
            return;
        }
    }
}
