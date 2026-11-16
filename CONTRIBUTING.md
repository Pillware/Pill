# How to write commit messages

We follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/)
with the rules below. Commits made before these rules stay as they are.

```
<type>(<scope>): <description>

<body>

<footers>
```

Only the first line is required.

## First line

```
fix(extension:physics): use run_in_edit_mode on edit-mode systems
```

- **type**: one of the [types](#types).
- **scope**: one of the [scopes](#scopes). If none fits, leave it out along
  with its parentheses.
- **description**: completes "This commit will ...". Imperative ("add", not
  "added"), lowercase, no period, at most 10 words. If you need more, the
  commit is probably two commits.
- **`!`** before the colon marks a [breaking change](#breaking-changes).

### Types

| Type       | The commit                                                        |
| ---------- | ----------------------------------------------------------------- |
| `feat`     | adds something projects, extensions, the editor or tools can use  |
| `fix`      | corrects wrong behaviour                                          |
| `perf`     | makes something faster or smaller, with the same behaviour        |
| `refactor` | restructures code, with the same behaviour                        |
| `test`     | only changes tests                                                |
| `docs`     | only changes documentation or comments                            |
| `build`    | changes Cargo manifests, features, build scripts or the toolchain |
| `ci`       | changes `devops/ci_cd/` or `.github/workflows/`                   |
| `chore`    | changes no code, build or tool (ignore files, editor settings)    |
| `revert`   | reverts an earlier commit                                         |

If several fit, pick the one an engine user would notice. A fix that also
tidies the code around it is a `fix`.

### Scopes

A scope names the **feature** a change is about, not the folder it touches.
Hot reloading spans the engine core, the development host, the patch scanner
and the editor, and a change to any of them is `hot_reload`.

#### General scopes

| Scope        | Covers                                                                                  |
| ------------ | --------------------------------------------------------------------------------------- |
| `ecs`        | world, entities, components, queries, systems, resources, commands, persistence, macros |
| `scripting`  | script components and their hooks                                                       |
| `play_mode`  | Play and Stop: what a session copies, runs and restores                                 |
| `hot_reload` | watchers, module reloads, function patching, layout fingerprints, generated host crates |
| `assets`     | asset store, guids and `.meta` files, importing and cooking, asset packs, scenes        |
| `host`       | starting and running a project: project settings, module loading, the frame loop        |
| `telemetry`  | logging, profiling, metrics, analytics                                                  |
| `renderer`   | renderer contract, master renderer, shaders, materials, debug and gizmo drawing         |
| `ui`         | in-game UI, themes, the UI kit                                                          |
| `editor`     | editor shell, tabs, panels, gizmo, selection, undo, the scripted test channel           |
| `csharp`     | anything only C# projects notice: bridge, runtime, compiler, analyzers                  |
| `input`      | input devices and events                                                                |
| `web`        | browser builds, the web frontends, the native/web platform layer                        |
| `shipping`   | shipping builds, bundles, static linking                                                |
| `devops`     | test suites, tools, setup scripts                                                       |
| `examples`   | example projects                                                                        |

#### Engine extension scopes

An extension has the scope `extension:<name>`, where `<name>` is its folder
under `modules/extensions/` without `pill_`: `extension:physics`,
`extension:audio`, `extension:spline`. The scope covers all of the
extension's crates, including its `_editor` crate and its examples. A new
extension gets its scope automatically.

- The test-support extensions (`extension:test`, `extension:dummy_color`,
  `extension:dummy_math`, `extension:dummy_random`) usually go with the
  `test` type.
- The master renderer and the editor shell live in `modules/extensions/`,
  but the host loads them itself, so they use `renderer` and `editor`.

#### Choosing a scope

- **Go by the subject.** "Rebuild the rapier world on reload" is
  `extension:physics`. "Keep resources across reloads" is `hot_reload`.
- **Use `csharp` for C# only.** A change that only C# projects notice is
  `csharp`, even when it is about reloading or assets.
- **Leave it out when nothing owns the change**, such as a rename across the
  workspace or a dependency bump.
- **Give one scope and never invent one.** A new area adds its row to the
  table in the same commit.

## Body

The body is optional. It lists the changes, so someone reading `git log`
knows what is in the commit without opening the diff. Leave it out when the
first line says everything:

```
feat(editor): add a Redock entry to the dock tab menu
```

- Write one bullet per change a reader cares about, not one per file.
- Start each bullet with a past-tense verb: Added, Changed, Removed, Fixed,
  Moved.
- Name what changed. "Updated shaders to accommodate new features" says
  nothing.
- Keep bullets short, one line where possible.
- Add a reason only when it isn't obvious, as a short clause: what broke,
  why this approach, or what to watch out for.
- Put a reason that covers the whole commit in a short paragraph after the
  bullets. Don't start that paragraph with a word and a colon, such as
  `Note:`, or tools will read it as a [footer](#footers).
- Leave a blank line after the first line, wrap at 72 characters, and indent
  continuation lines by two spaces.

Generated messages use this style but tend to be vague. Check every bullet.

### Examples

**Name the changes.**

Needs work:

```
feat(editor): add play controls to the menu bar

- Implemented play mode controls in the editor.
- Added keyboard shortcut support.
- Enhanced the UI to reflect the play state.
- Updated tests to cover the new functionality.
```

Better:

```
feat(editor): add play controls to the menu bar

- Added Play, Pause and Step buttons to the centre of the menu bar.
- Bound Play, Pause and Step to Ctrl+P, Ctrl+Alt+P and F10.
- Tinted the menu bar and tab strips while a session runs.
- Showed the Game tab on Play and the previous tab again on Stop.
```

"Implemented", "support" and "enhanced" could describe any commit. The
better version says what a user will see.

**Say what broke.**

Needs work:

```
fix(extension:physics): use run_in_edit_mode on edit-mode systems

- Changed edit_mode attribute to run_in_edit_mode for physics debug draw
  and restore systems to ensure they run correctly in edit mode.
```

Better:

```
fix(extension:physics): use run_in_edit_mode on edit-mode systems

- Changed `edit_mode` to `run_in_edit_mode = true` on the debug draw and
  restore systems, because `#[system]` no longer accepts `edit_mode`.
```

"To ensure they run correctly" only repeats the first line. The better
version gives the cause.

**Explain choices, and put the main reason in a paragraph.**

Needs work:

```
feat(extension:physics): drop the simulation when Play stops

- Added physics_restore system to handle simulation restoration.
- Implemented restore epoch comparison logic.
- Updated system ordering to ensure correct behaviour.
```

Better:

```
feat(extension:physics): drop the simulation when Play stops

- Added the physics_restore system to clear the rapier world after Stop.
- Made it a separate system, because physics_sync only runs during Play.
- Ordered it before stepping, so the next Play uses restored components.

Without this, a reused entity handle could still map to a body from the
play session.
```

Each choice gets a short clause. The reason for the whole commit goes in the
paragraph.

## Footers

Footers are `Token: value` lines at the end of the message, after a blank
line. Tools read them: `git log --format='%(trailers)'` lists them, and
GitHub shows co-authors. Most commits have none. We use two:

- `BREAKING CHANGE: <what breaks and what to do>`: goes with the `!`. See
  [Breaking changes](#breaking-changes). It is the only token with a space,
  and it must be uppercase.
- `Co-authored-by: Name <email>`: for a human co-author. GitHub links the
  person when the email belongs to their account.

```
feat(editor): add a Redock entry to the dock tab menu

Co-authored-by: Jane Doe <jane.doe@example.com>
```

Without the blank line above it, a footer is read as part of the text before
it.

Never credit an AI tool. Delete any `Co-Authored-By` line naming an assistant,
or "Generated with" line, that a tool adds.

## Breaking changes

A change is breaking when an existing project, extension or saved file stops
working until someone edits it. That covers changes to:

- the `pill_engine` facade API, including its macros (`#[system]`,
  `#[script]`, ...),
- the C# API, or the C#/Rust ABI between the bridge and the runtime,
- a saved format: `.scene`, `.material`, `.theme`, settings assets, `.meta`
  files, the editor layout,
- a `project_settings.yaml` or `editor_settings.yaml` key, or a `PILL_*`
  environment variable.

Add a `!` and a footer that says what to do:

```
refactor(ecs)!: rename the system edit_mode flag

BREAKING CHANGE: `#[system(edit_mode)]` is refused. Write
`#[system(run_in_edit_mode = true)]` instead.
```

## First lines, before and after

| Before                                                                  | After                                                                     | Why                                 |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------- | ----------------------------------- |
| `fix(notices): clarify build time reporting in host notices`            | `docs(hot_reload): build_ms covers cargo and C# builds`                   | only a comment changed              |
| `feat(analytics): record managed assembly reload events`                | `feat(telemetry): record managed assembly reloads`                        | `analytics` is not a scope          |
| `feat(config): enhance project configuration with directory resolution` | `feat(host): store the canonical project directory`                       | not a scope; "enhance" is vague     |
| `feat(hot_scan): classify system declarations without explicit markers` | `feat(hot_reload): treat every #[system] function as patchable`           | scope by feature, not by crate      |
| `fix(tests): adjust module test processor order`                        | `test: reorder the module test processor system`                          | it only changes test support        |
| `Physics: colliders without rigid body are static`                      | `feat(extension:physics): treat colliders without a rigid body as static` | type and scope in the standard form |

## Copilot

To make GitHub Copilot in VS Code follow these rules when it writes a
commit message, add this to your settings:

```json
"github.copilot.chat.commitMessageGeneration.instructions": [
  { "file": "CONTRIBUTING.md" }
]
```
