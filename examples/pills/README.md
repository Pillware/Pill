# pills

Two hundred thousand pills drifting through a curl noise field, watched through
a free flying camera, with an in-game tuning overlay.

The field is Bridson, Hourihan and Nordenstam's curl noise ("Curl Noise for
Procedural Fluid Flow", SIGGRAPH 2007): three decorrelated noise fields form a
vector potential, and each pill moves along its curl, which is divergence free -
so the pills swirl instead of bunching into sinks. A tuning overlay retunes the
field live, and the camera flies with WASD while a right-button drag looks
around.

`pills_rust` and `pills_csharp` run the same field, the same system layout and
the same tuning keys against the same `res` tree:

```
res/models/      the pill every entity draws
res/textures/    the map the pill material binds
res/materials/   one standalone material, naming its shader by guid
res/fonts/       the face the in-game overlay uses
res/ui/          the overlay's theme override
```

Nothing else is in the scene: no ground, no pillars and no sky, so the swarm is
what the camera flies through and what the renderer draws. `examples/local/pills_awakens/`
is the full level this was distilled from.

The four assets are also what the theme pipeline is tested against
(`devops/tests/test_ui_theme.py`): the code default is `Theme::brutalist()`,
`res/ui/accent.theme` overrides only the accent and the heading size, and the
body font is the Silkscreen face under `res/fonts`.

## Run it

```powershell
cd modules
$env:PROJECT_PATH = "../examples/pills/pills_rust"
cargo run --package pill_standalone --features rendering
```

## Keys

| Keys                              | Does                      |
| --------------------------------- | ------------------------- |
| `W` `A` `S` `D`, `E`/`Q`, `Shift` | Fly, rise or sink, sprint |
| Right-drag mouse                  | Look around               |
| `T` / `G`                         | Narrow or widen the view  |
| `O` / `P`                         | Curl scale                |
| `L` / `K`                         | Attraction                |
| `M` / `N`                         | Damping                   |
| `V` / `B`                         | Curl epsilon              |
| `F3`                              | Debug draw                |

## What to try

- Change `PILL_COUNT` in `src/game.rs` (Rust) or `ProjectConstants.PillCount`
  (C#) and watch the reload rebuild the scene up to the new count.
- Tune the field with the keys above, then read the same values off the
  overlay; they are the resource the systems read.
- Edit `res/ui/accent.theme` and watch the overlay restyle without a restart.
