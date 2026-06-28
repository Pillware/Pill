# City - ECS Benchmark & Project Example

A Pill example that serves double duty: a playable city simulation in normal
mode, and a reproducible performance benchmark with two variants (windowed / headless).

## Modes

| Mode                 | Feature flag         | Description                                                          |
| -------------------- | -------------------- | -------------------------------------------------------------------- |
| Normal pill project  | *(none)*             | Full city with plane, pill, camera, spawner, citizen movement        |
| Benchmark (windowed) | `benchmark_window`   | 10 000 citizens with PBR rendering, auto-exits after N frames        |
| Benchmark (headless) | `benchmark_headless` | 10 000 citizens pure ECS, no GPU / window, auto-exits after N frames |

The normal pill project and both benchmarks share zero code - `lib.rs` selects which file to
compile at build time via `#[path]`.

## Quick start

```bash
# Normal pill project
PillLauncher -a run -p examples/city

# Windowed benchmark (needs GPU / display)
PillLauncher -a run -p examples/city --features benchmark_window

# Headless benchmark (CI-friendly, no GPU)
cargo run -p pill_project --features benchmark_headless --bin city-bench-headless
```

## Normal pill project controls

| Key     | Action                                 |
| ------- | -------------------------------------- |
| `Space` | Spawn a new pill-citizen at the center |

Citizens wander the plane autonomously - each follows a queue of random waypoints
with acceleration-based movement.

## Benchmark

### How it works

1. 10 000 citizens are spawned immediately on startup.
2. The simulation runs for **1000 frames** (configurable via `BENCHMARK_FRAMES` in `config.ini`).
3. The first 50 frames are discarded (ECS warmup).
4. Frame times (ms) for the remaining 950 frames are collected.
5. A JSON statistics report is printed to stdout and the engine exits automatically.

### Statistics output

```json
{
  "mode": "windowed",
  "total_frames": 1000,
  "measured_frames": 950,
  "warmup_frames": 50,
  "entity_count": 10000,
  "stats": {
    "average_ms": 2.34,
    "median_ms": 2.12,
    "min_ms": 1.89,
    "max_ms": 5.67,
    "range_ms": 3.78,
    "variance": 0.45,
    "stddev_ms": 0.67
  }
}
```

### Running via tests.sh

```bash
# 5 iterations, aggregates results
./devops/tests/tests.sh --local benchmark-city
```

### Configuring frame count

Add to `res/config.ini`:
```ini
BENCHMARK_FRAMES=2000
```

## Architecture

```
src/
├── lib.rs           # selects project.rs or bench.rs via #[path] + feature gates
├── project.rs          # normal city pill project (no cfg gates)
└── bench.rs         # unified benchmark (shared ECS, rendering gated via cfg)
```

In `bench.rs`, rendering code is gated behind `#[cfg(feature = "benchmark_window")]`.
All ECS simulation code (citizen movement, frame counting, statistics) is shared
between both benchmark variants.

### Components

| Component                     | Purpose                                     |
| ----------------------------- | ------------------------------------------- |
| `CitizenComponent`            | Path queue, movement speed, acceleration    |
| `BenchState` (benchmark only) | Frame counter, timing data, warmup tracking |

### Engine features used

- `Engine::request_exit()` - graceful auto-shutdown after N frames
- `Engine::frame_count()` - monotonic counter for reproducible RNG seeding
- `Engine::frame_delta_time()` - per-frame timing in milliseconds
- `Engine::config()` - reads `BENCHMARK_FRAMES` from `config.ini`

## CI

A dedicated `benchmark` job runs on every push to `main` and every PR. It uses
`benchmark_window` mode with `xvfb-run` for a virtual display on headless runners.
