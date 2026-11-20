# Examples

The official examples, tracked in this repository. Each one is a pair: the same
demo written twice, once in Rust and once in C#, so the two scripting surfaces
can be compared directly.

| Example                                      | What it shows                                                                                |
| -------------------------------------------- | -------------------------------------------------------------------------------------------- |
| [`cube`](cube/README.md)                     | The smallest drawn scene, with its mesh and material authored as files under `res`.          |
| [`cube_no_editor`](cube_no_editor/README.md) | The same cube from one OBJ file and no metadata: no `.meta`, no scene file, no editor layer. |
| [`pills`](pills/README.md)                   | 200,000 pills drifting through a curl noise field, with an in-game tuning overlay.           |
| [`headless`](headless/README.md)             | An ECS-only simulation: `renderer: none`, no window, no GPU.                                 |

## Running one

The host resolves `PROJECT_PATH` against the Cargo workspace, which is where
every command below starts.

```powershell
cd modules

# Windowed, with the renderer compiled into the host.
$env:PROJECT_PATH = "../examples/cube/cube_rust"
cargo run --package pill_standalone --features rendering

# Headless: no renderer, so `headless` is the only example it can draw nothing of.
$env:PROJECT_PATH = "../examples/headless/headless_rust"
cargo run --package pill_standalone
```

A C# project runs the same way: point `PROJECT_PATH` at its folder and the host
compiles the assembly with the .NET SDK before loading it.

```powershell
$env:PROJECT_PATH = "../examples/cube/cube_csharp"
cargo run --package pill_standalone --features rendering
```

## Every project builds on its own

`python devops/tests/test_examples.py` builds every example in Release and
reports artifact sizes; `--list` names what it found. The suite finds projects
by convention, so a folder added here is picked up without an edit.

## Extension examples

An extension ships its own examples beside its code:
`modules/extensions/<extension>/examples/<name>/`, one complete project per
folder (`project_settings.yaml` listing the extension under `modules:`, a
`Cargo.toml` or a `.csproj`, `src/`, `res/`).

They are discovered by that path alone - nothing under `devops/` lists them -
and `devops/tests/test_extension_examples.py` builds every one and runs it
headless, so an example cannot rot and a new one cannot be forgotten. The suite
waits for a line carrying the example's folder name in brackets
(`[audio_scene] ready`), so an example states its own name once and needs no
entry anywhere else.

The difference from the folders above is curation: these are the `examples/`
projects chosen to be built in CI, while an extension example exists for the
module that ships it and is always run.

## Logging from a project

A project logs through the same `tracing` macros the engine uses, re-exported
as `pill_engine::tracing`, so the `logging:` settings in its
`project_settings.yaml` decide what prints:

```rust
// The project's own target, the values in fields, logged when it changes.
pill_engine::tracing::info!(target: "project", entities = spawned, "[project] wave spawned");
```

```text
[19:11:15:858] INFO  project  [project] wave spawned | entities=12
```

- **No `println!`.** A print has no time, level or target, so no setting can
  turn it down, and it breaks the log's layout. Use the `tracing` macros.
- **Use the project's own target**, never an `engine::` one. A fixed name such
  as `"project"` works, and so does leaving the target out: the line then
  carries the module path that logged it (`cube_rust`, `cube_rust::game`).
- **The message says what happened; the values go in fields.** Write
  `error = %error, "scene creation failed"`, not `"scene creation failed: {error}"`,
  so a search for the message finds every occurrence.
- **Log once, not every frame or every input.** A line per key press or per
  frame at `info` buries everything else. Log when something changes, or use
  `debug` and raise it only while you need it.
- **Pick the level by whether someone has to act**: `error` when the project
  could not do what it set out to do, `warn` when it carried on in a degraded
  way (and the line says how), `info` for a step worth seeing, `debug` and
  `trace` for detail.
- **The `logging:` settings** choose what prints, per target, at `off`,
  `error`, `warn`, `info`, `debug` or `trace`:

  ```yaml
  logging:
    level: info          # every target at once (optional)
    targets:
      project: debug     # this project's own lines, details too
  ```

  A shipping or web build bakes the same settings into its bundle.
- **An extension example's ready line** keeps the form its suite waits for:
  `[<folder name>] ready`, once, when the example has set itself up.

A C# project's `Diagnostics.Info(...)` (and its siblings) reaches the same log
through the bridge, on the `engine::csharp` target, with the managed category
in a `managed_target` field.

## `examples/local`

`examples/local/` is a developer checkout, not repository content: the whole
folder is gitignored, and the devops suites read it from disk. Nothing in it is
shipped, and a fresh clone does not carry it.

It holds the checkout's own demos - `circus_demo`, `circus_demo_cs`,
`circus_recreation`, `cube` and `pills_awakens` - which the tracked examples
were distilled from. `pills_awakens` is the full level: the `pills` example in
this folder is the same field with the ground, the pillars and the sky removed.
