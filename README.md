> **License:** by using Pill Engine you accept the
> [Pill Engine Community License](legal/licenses/COMMUNITY_LICENSE.md),
> unless you use it under the EUPL-1.2 or a Partner License. See
> [`LICENSE`](LICENSE).

- Hot-reloading (standalone console, standalone window, editor)
  - Engine
  - Project
  - Renderer
- Speed of loading (especially editor)
- Persistance/serialization works.

The standalone host is a generic launcher. It selects the project from the
`PROJECT_PATH` environment variable and reads that project's
`project_settings.yaml` for the extension list â€” there is no host-level
configuration file. Everything else â€” backend, name, watch directory, build
command, and output paths â€” is inferred from the project's manifest, so no
project identity is compiled into the host.

Rust project (headless):

- `set PROJECT_PATH=../examples/headless/headless_rust` then `cargo run --package pill_standalone`
- or, in PowerShell: `$env:PROJECT_PATH = "../examples/headless/headless_rust"; cargo run --package pill_standalone`

Rust project (windowed): the same, with `--features rendering` (try
`../examples/cube/cube_rust`).

**The host and the project must resolve the same dependency graph.** Cargo  
folds a dependency's resolved features into the dependent's `-C metadata`, and  
that hash is part of every symbol name `pill_core.dll` exports, so a host and a  
project that resolve different graphs link two incompatible engine dylibs and  
the project fails to load. That is why a project depends on  
`pill_master_renderer` with `default-features = false`: `Position` and `Color`  
come from `pill_engine`, `Sprite` needs only the renderer's data half, and wgpu  
stays out of the project's graph so a headless host and a windowed one can both  
load it. Should the two ever diverge anyway, the host names both dylibs rather  
than letting the loader report it as "The specified procedure could not be  
found".

Live per-function hot patching is on by default in dev builds: a body-only
edit to any `#[system]` is compiled on its own and swapped into the
running engine at the next frame, instead of a full module reload. The engine
gives every system a dispatch slot, so no marker of its own is needed;
`#[pill_hot_fn]` marks a plain function a system calls. Everything
a patch cannot express (signature or type changes, new components, constants)
falls back to the normal reload. To run the pure reload path without the patch
machinery - e.g. when measuring reload performance - build the host with  
`--no-default-features --features development` instead.

C# project: point `PROJECT_PATH` at the directory containing the `.csproj`,  
e.g. `../examples/cube/cube_csharp`.

## Examples

`examples/` holds the official examples, each a Rust project and a C# project
running the same demo: `cube`, `cube_no_editor`, `pills` and `headless`.
`examples/README.md` lists them and shows how to run either half;
`examples/local/` is a gitignored developer checkout that nothing ships.

The projects under `devops/tests/projects/` are the suites' own fixtures: the
hot-reload, C# bridge, renderer and editor suites drive them, and each carries
the anchors its suite edits and asserts. They are tracked so a clean checkout
can run the gate.

## Shipping builds

`devops/ci_cd/build_shipping.py` builds the shipping build: `pill_standalone`  
with the `shipping` feature, which runs the project through `pill_player` and  
does not compile the development host (`pill_development_host`) at all. It reads the  
project's scripting language from its manifest: the generated bundle carries  
the backend, and a managed C# project (`*.csproj`) also gets its assemblies  
built and copied. A project's `res` directory is packed into the binary at  
build time and read from there, so the game does not need its asset files  
beside it. A managed C# build has two shipping modes:

- **Framework-dependent (default):** `dotnet build -c Release` produces the  
project assembly, and the host boots CoreCLR through hostfxr at runtime. The  
user's machine must have the .NET 8 runtime installed.
- **NativeAOT self-contained (`--csharp-aot`):** `dotnet publish -p:PublishAot=true` merges the loader, the gameplay code, and a trimmed  
runtime into one native library; the host loads it directly with no hostfxr,  
no .NET install, and no JIT.

```bat
cd D:\Programming\Rust-Hybrid-ECS

set PROJECT_PATH=devops/tests/projects/native
python devops\ci_cd\build_shipping.py --features rendering   :: native shipping build

set PROJECT_PATH=devops/tests/projects/managed
python devops\ci_cd\build_shipping.py --features rendering   :: managed (C#), framework-dependent
python devops\ci_cd\build_shipping.py --csharp-aot           :: managed (C#), NativeAOT self-contained
```

`PROJECT_PATH` is resolved against the working directory first (the dev  
convention: from `modules`, `../devops/tests/projects/managed`), falling back to the  
repository root, so both spellings work from either place.

Build output lands under the project's `build/<timestamp>/` directory.

## Web builds

A Rust project also runs in the browser, through WebGPU. The web build is the  
shipping posture compiled for `wasm32-unknown-unknown` and run by `pill_web`:  
one module, statically linked, no hot reload. C# projects are not supported on  
the web. Its assets travel beside the page rather than inside the module: the  
build packs them into `assets.pillpack` next to `index.html`, and the module  
downloads and mounts it before the project starts (W-0105). Serve the whole  
directory, not just `pkg/`.

```bat
:: once: rustup target add wasm32-unknown-unknown  and  cargo install wasm-pack
python devops\tools\build_web.py devops/tests/projects/scene        :: release (wasm-opt)
python devops\tools\build_web.py --dev devops/tests/projects/scene  :: faster, unoptimized
python -m http.server 8000 -d devops/tests/projects/scene/build/web
```

Then open http://localhost:8000/ in Chrome or Edge 113+, Safari 18+ or Firefox  
141+. Like a shipping build, this regenerates `build/pill_shipping_bundle`;  
put the committed stub back with  
`python devops/tools/generate_shipping_bundle.py --restore-stub`.

## Extensions

Engine extensions are crates inside `modules/extensions/`, one folder per
extension (`modules/extensions/<extension>/modules/runtime/`), built into a `cdylib`
and loaded by the host next to the project. Each is watched, rebuilt and swapped
on its own, so editing one module reloads only that module and leaves the
project and every other module running.

The `modules` list in the project's `project_settings.yaml` selects which ones  
to load, in order. An absent or empty list loads none.

- `modules: ["pill_spline"]` in `project_settings.yaml` â€” load the spline module.
- `modules: ["pill_spline", "pill_physics"]` â€” several modules.
- `modules: []` (or omitting the key) â€” run with no extensions.

### Adding a module

The workspace manifest globs `extensions/*/modules/*`, so a module is discovered
by existing; nothing lists it by name.

1. Create `modules/extensions/<name>/modules/runtime/` with a `Cargo.toml` declaring
   `crate-type = ["rlib"]` and depending on `pill_engine`. The crate root is
   `src/<name>.rs`, not `src/lib.rs`, so debuggers and the hot scanner can
   tell crate roots apart.
2. Mark the registration entry point with the `#[pill_module]` attribute
   (or `#[pill_project]` for a project); it exports the module ABI entry
   points for you.
3. Add `<name>` to the `modules` list in the project's `project_settings.yaml`.

Everything else â€” watch directory, build command, output path â€” is derived from
the crate's location, so the host needs no changes. The host **generates** the
`cdylib` wrapper it actually loads under
`modules/extensions/<name>/generated/host_module_<name>/` on startup: it is a
build artifact, never written by hand. See
`modules/extensions/pill_spline/` for the reference implementation. An
extension's optional editor part is the crate in its `modules/editor` folder,
written against `pill_editor_core` (the old `pill_ui` editor part retired with
W-0043).

Modules must live in this directory rather than anywhere on disk: sharing the  
workspace lockfile is what makes a module resolve the same dependency graph as  
the host, which keeps component type identities and the mangled symbols of the  
shared `pill_core` library in agreement.

### Launching outside Cargo

The engine workspace links `pill_core` dynamically so the host and every loaded  
module share one copy of its telemetry state. That also makes the built  
executable depend on the toolchain's `std-<hash>.dll` at load time. The build  
script in `pill_standalone` (and `pill_editor`) stages that dylib next to the  
executable automatically, so `target\debug\pill_standalone.exe` runs directly  
without any `PATH` adjustment:

```powershell
.\target\debug\pill_standalone.exe
```

If the staged dylib is missing (for example the toolchain was updated and the  
binary was not rebuilt), the build script re-copies it on the next `cargo build`.

- `cargo run --package editor` â€” Run the Rust project in the editor.
- `dotnet build devops/tests/projects/managed/managed.csproj -c Release --nologo` â€” Build the C# project and its `csharp_runtime` dependency.
- `dotnet run --project modules/csharp/pill_csharp_runtime/tests/csharp_runtime_tests.csproj -c Release` â€” Run the C# system discovery and scheduler-access tests.
- `cargo test --workspace` â€” Run all Rust workspace tests.
- `cargo check --workspace` â€” Type-check the complete Rust workspace without producing release binaries.

## Contributing

Contributions are welcome. 
Read [`CONTRIBUTING`](legal/contributing/CONTRIBUTING.md) to learn more.
