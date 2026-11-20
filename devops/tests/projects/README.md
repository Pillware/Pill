# Test projects

A **test project** is a small Pill project that exists only so the suites in
`devops/tests` have something to run: a suite starts a host with `PROJECT_PATH`
pointing at one, edits its files, and checks the host's log.

Three projects, one per kind of anchor. Every suite drives one of them, and no
suite spells a path to one: the roots live in `devops/core/projects.py` as the
role groups `NATIVE_PROJECT_*`, `SCENE_PROJECT_*` and `MANAGED_PROJECT_*`
(`test_suite_standards.py`'s rule 5 refuses a path written inside a suite).

A test project is test-owned rather than an example: a suite edits its sources,
asserts its log lines and restores them, so a clean checkout can run the gate
without a developer's `examples/local` checkout.

| Project    | Kind                     | What it anchors                                                                                                                                                                                                                                                                                                                                                 |
| ---------- | ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `native/`  | Rust (`cdylib` + `rlib`) | Everything that mutates and reloads: hot reload and hot patching, schema migration, a module's data surviving a reload, a forgotten type's column being dropped, the committed render chain, and the shipping bundle. Loads `pill_spline`. A module's own demo belongs in that module's `examples/` folder, which `devops/tests/test_extension_examples.py` runs. |
| `scene/`   | Rust (`cdylib` + `rlib`) | Everything read-mostly: the editor, scene picking, extra surfaces, the browser build, asset reimport, and the UI theme pipeline. Four entities (camera, three rotating models), one mesh drawn three times, one committed material, a sample map no material binds, a themed UI layer, one committed render pass and an editor project layer. Loads no modules. |
| `managed/` | C#                       | The Rust/C# bridge, the scripting analyzers and the managed iteration benchmark. One project: the bridge surface (components, a resource, systems, asset imports), the extension mirrors, and three gated query shapes that measure only under `PILL_BENCH_ENTITIES`.                                                                                           |

Together the three weigh 2.3 MB in 96 files - `native` 110 KB, `scene` 1.1 MB,
`managed` 1.0 MB - against the 37.4 MB the seven projects they replaced held.
Build output (`target/`, `build/`) is not counted and is gitignored.

The `scene` test project's models are sized for the editor's default orbit view,
which looks at the origin from sixteen units away: at that distance the row
fills enough of a scene viewport that a click at its centre reaches a mesh
rather than the entity badge drawn over it. Change the scale and the editor
suite's pick probe has to change with it.

Both models past the first carry the entity reference (`scene::
ShowcaseReferenceComponent`), each following the first one. Two carriers rather
than one because the editor suite's reference step needs a model it has not
selected yet to open the row, and which model a scene pick answers depends on
the view; the first model carries none, so the reference section does not push
its own mesh rows out of the Inspector's body.

## Conventions

- A test project is named after its directory (`native`, `scene`, `managed`), because
  the host keys staged DLLs by package name: a project named after something
  else would share a DLL with whichever test project carried that name.
- `res` is the project's asset root. `res/editor/` is never packed into a game
  build, so an editor theme stays a development-time document.
- Assets that have a `.meta` sidecar keep it; the store resolves their identity
  through it, and the asset suites edit, corrupt, move and restore those files.
- A managed test project's `modules:` list drives C# mirror generation. List only
  modules that expose components or mirrored value types.
