# PillLauncher — Build & Run Pipeline

`PillLauncher` is the CLI tool that builds and launches Pill Engine projects. It handles workspace setup, compilation, artifact staging, and process launch in a single command.

---

## Quick start

```powershell
# Build and run (hot-reload mode)
.\engine\pill_launcher\target\release\PillLauncher.exe run -p .\examples\cube -c hot-reload

# Build only (release)
.\engine\pill_launcher\target\release\PillLauncher.exe build -p .\examples\cube -c release
```

### Compile modes (`-c`)

| Mode | Flag | Description |
|------|------|-------------|
| Debug | *(default)* | Unoptimised, full debug info |
| Release | `release` | Optimised, no debug info |
| Hot-reload | `hot-reload` | DLL hot-swap on source change |

---

## What happens when you `run`

The pipeline has two phases: **workspace preparation** and **build + launch**.

### Phase 1 — Workspace preparation (~6 ms)

1. **`check_project_validity`** — confirms the project directory has `Cargo.toml`, `src/`, `res/`, and `res/config.ini`.
2. **`patch_project_manifest`** — rewrites any `NO_PATH` placeholders in the project's `Cargo.toml` to absolute paths (needed for projects created outside the launcher).
3. **`find_engine_workspace`** — locates the `engine/` directory by walking up from the executable or `cwd`, or via the `PILL_ENGINE_WORKSPACE_DIR` env var.
4. **`normalize_path`** — converts the project path to an absolute forward-slash string for injection into `engine/Cargo.toml`.
5. **`read_workspace_manifest`** — reads `engine/Cargo.toml` and checks which project is currently linked.
6. **`switching_project` check** — compares canonicalized paths to detect a project switch (case-insensitive on Windows/macOS).
7. If switching: **`remove_stale_artifacts`** — deletes old `project.*`, `pill_native.*`, `pill_runtime.*` artifacts so Cargo does not reuse incompatible binaries.
8. **`inject_workspace_member`** — writes the project path into the `members = [...]` array in `engine/Cargo.toml`, tagged with a marker comment so it can be found and restored later.
9. **`update_project_workspace_line`** — ensures the project's own `Cargo.toml` has `workspace = "<engine_path>"`.

A `WorkspaceGuard` is created that **restores both manifests to their originals on drop**, so the repository is left clean whether the build succeeds or fails.

### Phase 2 — Build + launch

#### Mtime pre-check (~3 ms)

Before doing anything else — **before** `prepare_workspace_for_project` is called — the launcher performs a source-file mtime comparison:

```
watched sources:
  project/src/**         pill_native/src/**         pill_runtime/src/**
  pill_native/Cargo.toml pill_runtime/Cargo.toml

checked artifacts  (in target_projects/<Title>/<profile>/):
  project.dll / libproject.so
  pill_runtime.dll / libpill_runtime.so
  pill_native.exe  (non-hot-reload modes)
```

If **every artifact is newer than every source file** → `skip_cargo = true` and the entire `cargo build` invocation is skipped. This reduces a no-change launch from **~1.5 s** to **~8 ms**.

> **Why before `prepare_workspace_for_project`?**  
> The `WorkspaceGuard` writes back `project/Cargo.toml` on drop at the end of every run, bumping its mtime to "now". Checking *after* would always see the project manifest as newer than the artifacts and never skip cargo.

> **Why not watch `engine/Cargo.toml` or `project/Cargo.toml`?**  
> Both are rewritten by the launcher on every invocation, so they carry the launcher's own mtime, not the developer's. Real dependency changes always also touch `pill_native/Cargo.toml` or `pill_runtime/Cargo.toml`, which are watched.

#### Cargo build (skipped when up-to-date, otherwise ~1.5 s)

```
cargo build -p project -p pill_native -p pill_runtime [--profile hot-reload | --release]
```

All three crates are compiled in the **engine workspace** so that generic type IDs are consistent between the host executable and the project DLL.

The target directory is `engine/target_projects/<ProjectTitle>/` (or overridden via `PILL_TARGET_DIR`).

#### Post-build artifact staging (~3 ms)

- **`copy_file_if_newer`** — copies `pill_native.exe`, `project.dll`, and `pill_runtime.dll` from the cargo target dir into the project's `build/<mode>/` output directory, skipping files that are already up-to-date by mtime + size.
- In **release** mode: `stage_packaged_resource_files` copies `res/` into `build/release/data/res/` for distribution.
- In **hot-reload** mode: also copies `project_hot_reloaded.dll` and `pill_runtime_hot_reloaded.dll` as the live-swap targets.

#### Launch

```
<build_dir>/<Title>.exe  [project args]
```

Env vars passed to the child process:

| Variable | Value |
|----------|-------|
| `PILL_LAUNCHER_BIN` | Path to the launcher executable (for hot-reload child spawning) |
| `PILL_ENGINE_WORKSPACE_DIR` | Absolute path to `engine/` |
| `PROJECT_DIR` | Absolute path to the project directory |
| `PILL_COMPILE_MODE` | `debug` / `release` / `hot-reload` |
| `PILL_STANDALONE_LAYOUT` | `development` or `packaged` |
| `PILL_ENABLE_HOT_RELOAD` | `1` in hot-reload mode, `0` otherwise |

---

## Timing breakdown (typical, no-change run)

```
prepare_workspace TOTAL      ~6 ms
  check_project_validity       <1 ms
  patch_project_manifest        2 ms
  find_engine_workspace         1 ms
  normalize_path               <1 ms
  read_workspace_manifest      <1 ms
  switching_project check      <1 ms
  inject_workspace_member       1 ms
  update_project_workspace_line 1 ms

mtime pre-check               ~3 ms  → skip_cargo=true
cargo build                    0 ms  (skipped)
post-cargo steps              ~3 ms
─────────────────────────────────────
Total "Building project..."   ~12 ms
```

When sources have changed, `cargo build` replaces the 0 ms line with a full incremental compile (~1.5 s null-build overhead + actual compile time).

---

## Environment variables

| Variable | Effect |
|----------|--------|
| `PILL_LAUNCHER_TIMING=1` | Print per-step `[TIMING]` lines and switch cargo to `--message-format=json` to show per-crate compile events with relative timestamps |
| `PILL_LAUNCHER_EXPERIMENTAL_LOGS=1` | Parse cargo stderr and suppress noisy lines, showing only errors and key status messages |
| `PILL_TARGET_DIR=<path>` | Override the cargo target directory (useful for shared build caches or RAM disks) |
| `PILL_ENGINE_WORKSPACE_DIR=<path>` | Override engine workspace discovery |
| `PILL_HOT_RELOAD_CHILD=1` | Set by the hot-reload host; tells the launcher it is running as a child rebuild process and should skip copying the standalone executable |

---

## Project structure requirements

```
my_project/
  Cargo.toml          must declare [package] name = "project"
  res/
    config.ini        must contain TITLE = <ProjectName>
  src/
    *.rs
```

The `TITLE` value (spaces stripped) becomes the output executable name, e.g. `TITLE = My Game` → `MyGame.exe`.
