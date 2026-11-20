# Audio scene - a `pill_audio` example

A project that exercises the audio extension end to end: one sound loaded from
`res/`, one listener, one emitter, and the frame on which the emitter starts
playing.

| | |
| --- | --- |
| Extension | `pill_audio` |
| Renderer | none - the example opens no window |
| Logs | `[audio_scene] ready sound=audio/blip.wav`, then `[audio_scene] playing sink=<n>` |

The managed twin, [`audio_scene_csharp`](../audio_scene_csharp/project_settings.yaml),
plays the same scene from C#: it imports the same sound and writes
`[audio_scene_csharp] playing`.

## Run it

From `modules/`:

```powershell
$env:PROJECT_PATH = "extensions/pill_audio/examples/audio_scene"
cargo run --package pill_standalone

# The managed twin, on the same invocation.
$env:PROJECT_PATH = "extensions/pill_audio/examples/audio_scene_csharp"
cargo run --package pill_standalone
```

The host builds the extension and the project on start, then runs them. On a
machine with no audio output the extension logs a warning, the source never
starts, and `ready` is still written - that line is what the CI suite waits
for.

## What it shows

- **A sound is an asset.** `Sound` is loaded through the asset store
  (`Engine::load_asset::<Sound>(path)`) and reached by `Handle<Sound>`; nothing
  holds the decoded audio.
- **A component records the request; the extension performs it.** The source is
  built with `AudioSourceComponent::playing(sound)`, which sets the flag the
  audio system turns into a play request on its next run. `is_playing` and
  `sink` are the system's answers, and the example only reads them.
- **Positions come from the transform.** The listener and the emitter both carry
  a `TransformComponent`. The extension registers it when it loads, because its
  system reads both positions from there and a project with no renderer has no
  data crate to do it.

## Files

| Path | What it is |
| --- | --- |
| `project_settings.yaml` | The project's manifest: one module, no renderer. |
| `Cargo.toml` | A `cdylib` project outside the Cargo workspace. |
| `src/lib.rs` | `init` builds the scene; `register` registers the report system. |
| `res/audio/blip.wav` | A tenth of a second of a quiet 220 Hz tone, 844 bytes. |

The host generates `projects_generated/host_project_audio_scene` when it runs
this project; that crate is not committed.
