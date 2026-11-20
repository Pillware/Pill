# headless

An ECS-only simulation: 256 entities counting the frames they live through, and
a report through the engine log about once a second.

`project_settings.yaml` selects `renderer: none`, which keeps both the GPU
module and its data crate out of the load order. No device is opened, no
surface exists, and nothing is drawn - what runs is the engine's core: world,
scheduler and hot reload. That is the posture a server-side or test build has.

| Project           | What it uses                                                                                 |
| ----------------- | -------------------------------------------------------------------------------------------- |
| `headless_rust`   | `pill_engine` only: one component, one resource, two systems                                 |
| `headless_csharp` | the managed runtime with `PillExtensionMirrors=false`: the same tickers, no renderer mirrors |

## Run it

```powershell
cd modules
$env:PROJECT_PATH = "../examples/headless/headless_rust"
cargo run --package pill_standalone
```

No `--features rendering`: the host binary built without the renderer is the
headless one. The C# twin runs the same way.

## What to watch

```
[headless] tickers=256 total_ticks=30720
```

One line a second, on wall-clock time: a headless host has no vsync, so it runs
the frame loop as fast as the machine allows. Edit `TICKER_COUNT` (Rust) or
`HeadlessConfig.TickerCount` (C#) and the reload restarts the world at the new
count - `init` fills the world up to a target rather than spawning fresh
entities, which is what keeps a hot reload from duplicating it.
