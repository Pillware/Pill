# devops

All development tooling for Pill.

```
devops/
├── tests/        things that assert    - each runs standalone
├── core/         shared code every script imports
├── ci_cd/        the CI gate (verify.py), shipping builds and shell entry points
├── tools/        builders, probes and one-off repository tools
└── setup/        host environment setup scripts
```

`ci_cd/verify.py` is the CI gate: CI runs its steps, and a local run is the
same gate. The shell scripts in `ci_cd/` hold no check logic of their own; each
locates the repository root and runs Python scripts in `tests/` or `tools/`.

The dependency direction is one-way: `tests/` and `tools/` import `core/`.
Nothing in `core/` imports upward.

Performance measurement (the Pill Lab CLI, the benchmark scripts and their
stored measurements) lives in the untracked `local/devops/` tree. It is not
part of this repository, so nothing here imports it and no tracked suite
asserts on it.

## tests/

Pass/fail suites for the hot-reload and persistence architecture, plus the
`project/` Rust test project they hot-reload against. Each runs standalone:

```powershell
# Host-driving regression suites
python devops/tests/test_hot_reload_suite.py
python devops/tests/test_hot_reload_migration.py --cycles 2
python devops/tests/test_csharp_bridge.py
python devops/tests/test_ui_theme.py            # the UI theme pipeline, `pills_rust`
python devops/tests/test_render_chains.py       # a committed .pipeline runs the frame, the native test project
python devops/tests/test_shader_recompile.py    # an edited project .hlsl reaches the GPU, the native test project
python devops/tests/test_resource_layout_fingerprint.py  # a declared resource is kept, dropped or rebuilt, the native test project
python devops/tests/test_module_resource_assert.py  # a module's declared resource is missing
python devops/tests/test_shipping_smoke.py
python devops/tests/test_web_smoke.py          # browser build in headless Chrome/Edge

# Static, build and CI checks (no host launch)
python devops/tests/test_coding_standards.py
python devops/tests/test_examples.py
python devops/ci_cd/verify.py --quick           # the CI gate's inner loop
```

A module ships its own `res` as a read-only asset root: the host lists it
beside the project's files, the editor shows it under the `extensions` root and
offers no create, rename, move, delete or settings write there, and a file the
module declares that goes missing stops the host naming the module and the
path (`test_module_resource_assert.py` drives exactly that). The engine's
default material, `pill_master_renderer_data`'s
`res/materials/pill.pbr.material`, is the resource the editor and the suite both
show.

`test_coding_standards.py` lints every `.rs` file against the Pill comment and
layout rules, and `test_examples.py` builds each example with the release
profile and reports artifact sizes. The lint's eighth check refuses work-item ids, stage numbers,
plan decisions, audit-finding ids, audit opportunity numbers, plan sections and
construction-site paths in `modules/` comments, with no allowance: a
reintroduced reference fails the check. The same check covers the managed
sources under `modules/csharp/`, the three example projects' `src/` folders and
the `devops/` Python tree, and it is strict there too - there is no baseline
file, so every scope holds zero allowance and any reference fails. The
`--write-comment-baseline` flag still exists for a future sweep that needs to
record debt before paying it down; a missing baseline allows zero. The `devops/`
tree is covered the same way - full-line `#` comments and module, class and
function docstrings - while `TIER:` and `PROTECTS:` docstring lines stay
untouched. Formatting, clippy and the
cargo test lanes are `verify.py`
steps; those two lanes run through `cargo-nextest` (`modules/.config/nextest.toml`)
when it is installed and through `cargo test` when it is not, and always run
the doctests with `cargo test --doc`. See `tests/README.md` for what each suite
pins down.

`tools/build_web.py` builds a Rust project for the browser (bundle and web app
generation, `wasm-pack`, the page from `tools/web/index.html`); the web smoke
test drives it. `tools/wasm_size_report.py` explains a web build's size: the
shipped module's sections, its gzip size, the embedded asset pack, and (with
`twiggy` on PATH) the code by crate - engine, project, standard library and
third-party families - from the binary wasm-opt has not stripped yet.
`tools/generate_third_party_licenses.py` writes a build's `licenses/`
directory: the license notices of every crate compiled in (through
cargo-about, checked against the accepted list in `modules/about.toml`), the
Rust standard library's notice file and, for a NativeAOT C# build, the .NET
runtime's. `ci_cd/build_shipping.py` and `tools/build_web.py` run it after
every build; an unaccepted license fails the build, a missing cargo-about only
warns.
`tools/comment_audit.py` ranks the Rust sources by comment debt for a cleanup
pass: process references (work-item ids, stage numbers, plan decisions),
history wording, overlong doc blocks and TODO markers, weighted by the file's
public surface. It prints the ranking and writes a JSON or Markdown report on
request; `--self-test` checks the analyzers after a heuristic change.
`tools/sync_local_devops_core.py` keeps the shared `core` modules copied into
the gitignored `local/devops/core` in step with `devops/core`; it rewrites the
copy's root derivation so the moved measurement tooling still resolves the
repository, and `--check` fails when the copy is stale.

### Styling a project or the editor

A project's UI is the brutalist `Theme` its code default builds, sparsely
overridden by one `.theme` file under `res`. The document names only what it
changes:

```yaml
format_version: 1
asset_type: pill_ui_theme::ThemeOverrides
guid: 000000000000000000000000000000a1
asset:
  palette:
    accent: '#33d17a'
  text_styles:
    heading:
      size: 24.0
  fonts:
    body: fonts/Silkscreen-Regular.ttf
```

The editor's own look is three layers - its code default, the user's file named
by the `theme` row in `preferences.yaml`, and the project's `editor/` layer.
A project configures the last one with `editor_settings.yaml` beside its
`project_settings.yaml`:

```yaml
editor_theme: "editor/my_theme.theme"   # below res; default: editor/editor.theme
focus_game_on_play: true                # Play brings the Game tab forward; default: true
```

`res/editor/` is editor-only content: game packs never carry it. The pipeline
is described in `AGENTS.md` ("The theme pipeline"), and
`tests/test_ui_theme.py` proves the game half end to end while
`tests/test_editor_egui.py` proves the editor half. That suite's play-mode area
(`tests/editor_steps_play_mode.py`) drives a whole session through the scripted
channel: Play, an edit that Stop discards, the restore epoch, the Game tab focus,
the history's depth and the pause/step controls.

## core/

Shared code, imported by everything above. Nothing here executes a measurement
or asserts anything.

| Module                 | Role                                                                 |
| ---------------------- | -------------------------------------------------------------------- |
| `paths.py`             | devops/repository layout, plus `ensure_devops_on_path()`             |
| `projects.py`          | the project catalogue: each suite's `PROJECT_PATH` and edited files  |
| `environment.py`       | process queries and timestamps                                       |
| `editor_log_tokens.py` | the editor log tokens the editor suites and the contract share       |
| `editor_session.py`    | one editor run: sandbox, launch environment and the anchored waits   |
| `scenario.py`          | the shared edit / wait / assert / restore loop                       |
| `test_report.py`       | pass/fail/skip tally, summary block, size reports, cargo test totals |
| `suite_common.py`      | host process plumbing: log tokens, output monitor, backup/restore    |
| `win_clipboard.py`     | the editor suite's side of the Windows clipboard                     |
| `workspace_graph.py`   | the Cargo-manifest reader the architecture guards check              |

A standalone script reaches it by putting `devops/` on `sys.path` first:

```python
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from core import criterion
```

## ci_cd/

The CI gate, the shipping build and the shell entry points.

| File                        | What it is                                                                       |
| --------------------------- | -------------------------------------------------------------------------------- |
| `verify.py`                 | the CI gate: every check CI runs, as named steps (`--list`, `--only`, `--quick`) |
| `check_relevant.py`         | runs only the checks a change set needs, by repository area                      |
| `build_shipping.py` / `.sh` | shipping builds (they clear `RUSTFLAGS`; a plain `cargo build --release` fails)  |
| `run_hot_reload_tests.sh`   | the hot-reload regression net: a fixed set of suites in `tests/`                 |
| `generate_documentation.sh` | shell entry point for the Markdown documentation pipeline in `tools/`            |

`check_relevant.py` is the loop to use after a change: it reads the working
tree's change set (or `--paths`), matches every file against a per-area rule
table, and runs just that area's checks - formatting and the coding-standards
lint for Rust changes, the crate's unit tests, the named `tests/` suites and
the few `verify.py` steps that guard the area. An editor suite gets the
editor build (the uplift) prepended; a file no rule recognizes falls back to
`verify.py --quick`, so nothing passes unchecked by accident. The full gate
and the tier runs stay for stage boundaries and milestones.

A step whose commands rewrite tracked files declares `cleanup` commands, which
run even when the step fails, so a red step cannot leave a build artifact where
a later commit could pick it up. `check-shipping` is the one step that needs
it: it generates `build/pill_shipping_bundle` for the shipping clippy run and
puts the committed stub back afterwards, and the quick `bundle-stub` step
refuses a tree that holds a generation.

The gate itself carries the editor end to end: its `editor` step runs the
host-tier revision suite, builds `pill_editor` (the uplift), and then
runs `test_editor_egui.py` - that order is required, because the revision
suite's narrowed build re-splits the shared DLLs and the editor launches a
pre-built binary. It is the slowest step - about fifteen minutes on a warm
cache - and it never runs under `--quick`.

```powershell
python devops/ci_cd/check_relevant.py --dry-run   # what would run, and why
python devops/ci_cd/check_relevant.py             # run only that
python devops/ci_cd/check_relevant.py --list-rules
python devops/ci_cd/check_relevant.py --base origin/main
```

```powershell
python devops/ci_cd/verify.py --list
bash devops/ci_cd/run_hot_reload_tests.sh
bash devops/ci_cd/generate_documentation.sh
```
