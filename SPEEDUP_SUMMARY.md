# Pill Engine — Startup Speedup Summary

## Overview

Two independent startup delays were investigated and reduced:

| Phase | Before | After | Reduction |
|---|---|---|---|
| Launcher overhead (cargo null-build) | ~1.6s | ~12ms | **−99%** |
| Engine window startup (`resumed()`) | ~1.45s | ~0.56s | **−61%** |

---

## Phase 1 — Launcher: Cargo Null-Build Overhead

### Problem

Every `PillLauncher.exe run` invocation ran `cargo build`, even when no source files had changed. Cargo's own fingerprinting scan (reading every `.d` dep-info file, hashing sources) took **~1.4s** even for a zero-change build.

### Root Cause

The launcher unconditionally spawned `cargo build` before launching the executable. Cargo's null-build time was dominated by its fingerprint/dep-info scan — it can't be avoided once cargo runs.

### Solution — mtime Pre-Check (`compute_skip_cargo`)

Added a pre-check in `engine/pill_launcher/src/utils/native_target.rs` that runs **before** calling `prepare_workspace_for_project` (and therefore before cargo is ever invoked). It compares the modification times of each artifact against the sources it depends on:

| Artifact | Checked against |
|---|---|
| `project.dll` | `<project>/src/**` |
| `pill_runtime.dll` | `pill_runtime/src/**` + `pill_runtime/Cargo.toml` |
| `pill_native.exe` | `pill_native/src/**` + `pill_native/Cargo.toml` |

**Exclusions**: `engine/Cargo.toml` and `<project>/Cargo.toml` are excluded because `WorkspaceGuard` rewrites them on drop, bumping their mtime every run.

If all artifacts are newer than all their sources, cargo is skipped entirely:

```
Sources unchanged, skipping cargo build.
[TIMING] cargo build: 0.000s
```

### Supporting Fix — Sentinel File for Project Switch Detection

**Problem**: The "last linked project" was read from the engine `Cargo.toml` workspace member list. But `WorkspaceGuard` restores the original `Cargo.toml` on drop (removing the injected member), so the next run always saw `switching_project=true` and triggered slow artifact cleanup.

**Fix**: A sentinel file `engine/.pill_last_project` is written after each successful injection. On the next run it is read as a fallback when the manifest has no marker, so `switching_project` is correctly `false` for repeated runs of the same project.

### Supporting Fix — Per-Crate mtime Checks

**Problem**: An initial combined mtime check compared all sources against all artifacts together. This failed when only `project.rs` was edited — `pill_native.exe` is older than `project.rs` (cargo doesn't rebuild native when only project sources change), so `skip_cargo` was incorrectly `false`.

**Fix**: Each artifact is checked only against its own crate's sources (see table above).

### Supporting Fix — Workspace Injection Always Runs

**Problem**: Workspace member injection was gated on `switching_project=true`. When the same project ran again (switching=false), injection was skipped, but `WorkspaceGuard` had already restored the manifest (removing the member line). This caused `"package 'project' not found"` errors.

**Fix**: Injection always runs unconditionally. Only the expensive artifact cleanup is gated on `switching_project`.

### Result

```
[TIMING] mtime pre-check result: skip_cargo=true
[TIMING] cargo build: 0.000s
[TIMING] post-cargo steps TOTAL: 0.004s
```

No-change run: **~1.6s → ~12ms**.

---

## Phase 2 — Engine: Window Startup Lag

### Problem

After the launcher finished (total ~12ms), there was a ~1–1.5s lag before the game window appeared.

### Instrumentation Added

Timing counters (`println!("[TIMING] ...")`) were added to:

- `pill_native/src/main.rs` — `run_app()` and `resumed()` callback
- `pill_runtime/src/lib.rs` — `create()` and `build_engine()`
- `pill_engine/src/engine.rs` — `Engine::initialize()`
- `pill_renderer/src/renderer.rs` — `State::new()` sub-steps

### Diagnosis

Initial full breakdown (with `wgpu::Backends::all()`):

```
resumed() TOTAL: 1.450s
├─ make_window_init:               0.001s
├─ EventLoop::new:                 0.013s
├─ create_window:                  0.037s
├─ RuntimeHost::load (dylib):      0.004s
├─ RuntimeHost::create:            1.051s  ← 72% of total
│    └─ build_engine:              0.937s
│         ├─ load_project.dll:     0.001s
│         ├─ Renderer::new:        0.857s  ← 83% of create
│         │    └─ (wgpu::Backends::all() probing Vulkan+DX12+DX11+GL)
│         └─ engine.initialize:    0.079s
│               ├─ AudioManager:   0.043s
│               ├─ lit shader:     0.017s
│               ├─ unlit shader:   0.015s
│               └─ start_project:  0.001s
└─ set_visible:                    0.356s
```

**83% of the startup time was inside `Renderer::new`** — specifically wgpu initialization.

### Root Cause — `wgpu::Backends::all()` Backend Probing

`wgpu::Backends::all()` probes **all** available backends on Windows: Vulkan, DX12, DX11, and GL. This multi-backend enumeration was expensive. A comparison of adapter/device creation times per backend:

| Backend | `request_adapter` | `request_device` | shader compile (each) | `Renderer::new` total |
|---|---|---|---|---|
| All backends | — | — | 0.017s | 0.857s |
| DX12 only | 0.409s | 0.226s | **0.077s** | 0.766s |
| **Vulkan only** | **0.017s** | **0.104s** | **0.017s** | **0.381s** |

Key findings:
- DX12 adapter enumeration is slow (0.409s) vs Vulkan (0.017s)
- DX12 DXIL shader compilation is 4–5x slower than Vulkan SPIR-V (0.077s vs 0.017s)
- With `Backends::all()`, wgpu was selecting Vulkan anyway (fast shaders confirmed), but still spending time probing the other backends

### Solution — Vulkan-First Default on Windows

`pill_renderer/src/renderer.rs` — `State::new()`:

```rust
_ => {
    // Default to the primary native backend for faster startup.
    // Probing all backends adds ~600ms on Windows.
    // Override with WGPU_BACKENDS env var if needed.
    #[cfg(target_os = "windows")]
    { wgpu::Backends::VULKAN }
    #[cfg(target_os = "macos")]
    { wgpu::Backends::METAL }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    { wgpu::Backends::VULKAN | wgpu::Backends::GL }
}
```

The `WGPU_BACKENDS` env var still overrides the default (e.g. `WGPU_BACKENDS=DX12`, `WGPU_BACKENDS=ALL`).

### Result

```
resumed() TOTAL: 0.564s
├─ make_window_init:               0.001s
├─ EventLoop::new:                 0.011s
├─ create_window:                  0.027s
├─ RuntimeHost::load (dylib):      0.002s
├─ RuntimeHost::create:            0.459s
│    └─ build_engine:              0.456s
│         ├─ load_project.dll:     0.001s
│         ├─ Renderer::new:        0.381s
│         │    ├─ instance+surface: 0.213s
│         │    ├─ request_adapter:  0.017s
│         │    └─ request_device:   0.104s
│         └─ engine.initialize:    0.075s
│               ├─ AudioManager:   0.038s
│               ├─ lit shader:     0.017s
│               ├─ unlit shader:   0.015s
│               └─ start_project:  0.001s
└─ set_visible:                    0.075s
```

Window startup: **1.450s → 0.564s** (−61%).

---

## Combined Result

| Scenario | Total time from launch to running game |
|---|---|
| Before (no-change run) | ~1.6s (cargo) + ~1.45s (window) = **~3.1s** |
| After (no-change run) | ~0.012s (cargo skip) + ~0.56s (window) = **~0.57s** |

**~5.4× faster** end-to-end for repeated runs with no source changes.

---

## Files Modified

| File | Change |
|---|---|
| `engine/pill_launcher/src/utils/native_target.rs` | `compute_skip_cargo()`, timing counters, `t_post` timer fix |
| `engine/pill_launcher/src/utils/workspace.rs` | Sentinel file logic, injection always runs, timing counters |
| `engine/pill_launcher/src/utils/files.rs` | `newest_mtime_recursive()`, `file_mtime()`, `artifacts_up_to_date()` |
| `engine/pill_launcher/src/utils/common.rs` | `use_verbose_timing()`, `extract_json_str()`, `extract_json_bool()` |
| `engine/pill_renderer/src/renderer.rs` | Vulkan-first backend default, wgpu sub-step timing |
| `engine/pill_runtime/src/lib.rs` | Sub-timing in `create()` and `build_engine()` |
| `engine/pill_engine/src/engine.rs` | Sub-timing in `initialize()`, audio init, shader pipeline creation |
| `engine/pill_native/src/main.rs` | Timing in `run_app()` and `resumed()`, `println!` instead of filtered `info!` |

---

## Remaining Potential Improvements

### High Impact

**1. Vulkan instance creation (0.213s)**
The largest remaining cost in wgpu init. Vulkan instance creation enumerates all Vulkan layers and extensions. Possible approaches:
- Set `WGPU_VULKAN_NO_VALIDATION=1` (or equivalent) in dev builds — disables the validation layer which contributes significantly to instance creation time
- Use `wgpu::InstanceFlags::empty()` in release/dev mode instead of `from_build_config()` which enables debug flags automatically

**2. `request_device` (0.104s)**
Device creation involves driver-side resource allocation. Limited options:
- Could potentially cache a pre-warmed wgpu device across hot-reloads (complex, requires architectural changes)

**3. Audio stream initialization (`AudioManagerComponent::new`, 0.038s)**
`rodio::OutputStream::try_default()` opens the OS audio output device synchronously. Could be moved to a background thread and initialized lazily on first sound play.

### Medium Impact

**4. Sink pool pre-allocation in audio**
`AudioManagerComponent::new` creates N `Sink` and `SpatialSink` objects upfront. The count is controlled by `MAX_CONCURRENT_2D_SOUNDS` / `MAX_CONCURRENT_3D_SOUNDS` config keys. Reducing defaults or making it lazy would help.

**5. Shader pipeline caching (wgpu Pipeline Cache)**
wgpu supports `wgpu::PipelineCache` (via `PipelineCacheDescriptor`) which serializes compiled pipeline state to disk and reloads it on subsequent runs, skipping GPU-side shader compilation. This could eliminate the 0.017s–0.077s per shader pipeline after the first run. Requires saving/restoring the cache blob between sessions.

**6. Async wgpu initialization**
`Renderer::new` blocks the main thread via `pollster::block_on`. The window could be shown earlier (with a loading screen) while wgpu initializes on a separate thread using proper async. This wouldn't reduce total init time but would eliminate the perceived black-screen delay.

### Low Impact / Maintenance

**7. DLL copy overhead (0.184–0.265s per DLL)**
`pill_runtime.dll` and `pill_native.exe` are copied to `build/dev/` on every build. The copy is only skipped when mtime hasn't changed (via `copy_file_if_newer`). For large DLLs this is measurable. Could use hard links instead of copies on NTFS to make it near-instantaneous.

**8. Remove timing instrumentation from release builds**
All `[TIMING]` `println!` statements are currently always active. They should be gated behind an env var check (like the existing `PILL_LAUNCHER_TIMING=1` pattern) or compiled out in release builds to avoid console noise in shipped games.
