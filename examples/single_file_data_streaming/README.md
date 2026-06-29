# Single File Data Streaming

Demonstrates **partial file reading with O(1) random access** — a 100×100 cube terrain grid streamed from a human-readable fixed-width text file, loading only the cells within camera range.

## Quick Start

```bash
PillLauncher run -p ./examples/single_file_data_streaming
```

## Controls

| Key   | Action                                 |
| ----- | -------------------------------------- |
| W / S | Move forward / backward (camera-local) |
| A / D | Strafe left / right                    |
| Q / E | Move down / up (world Y)               |
| ← →   | Rotate yaw                             |
| ↑ ↓   | Rotate pitch                           |

## What You See

- **10,000 orange cubes** in a 100×100 grid on the XZ plane (200×200 world units, 2.0 spacing)
- Cubes within a **100-unit circle** around the camera rise to their terrain height
- Cubes outside the circle snap to Y=0
- A floating pill model for reference
- Two egui debug windows: **Camera Debug** and **Stream Data**

## Architecture

```
src/
├── project.rs      ← Crate root, PillProject::start()
├── constants.rs    ← Grid dimensions, streaming parameters
├── components.rs   ← ECS component definitions
├── systems.rs      ← Camera movement, position streaming
├── ui.rs           ← Egui debug panels
└── utils.rs        ← Height map decoding, grid math, helpers
```

## How Partial Streaming Works

### The Problem
You have a 100×100 grid of heights stored on disk. Loading all 10,000 values into RAM is wasteful — at any moment, only ~7,850 cells are visible within the 100-unit radius. How do you read *only those cells* without parsing the entire file?

### Requirement: O(1) Random Access
To jump directly to cell (x=47, z=32) without scanning, you need to know its exact byte offset. This requires **fixed-width records**.

### The File Format (`res/data/height_data.json`)
```
  0.5000
  1.2345
 60.1234
  3.7890
  ...
```

- One value per line
- Each line is exactly **9 bytes**: 8-character float (`{:>8.4}`) + newline (`\n`)
- Cell at grid index `i = z * 100 + x` lives at byte offset `i * 9`
- 10,000 lines × 9 bytes = **90,000 bytes** (~88 KB)
- Human-readable: open in any text editor

### Per-Frame Streaming Logic

```rust
// 1. Find camera position in grid coordinates
let (cam_gx, cam_gz) = world_to_grid(camera.x, camera.z);

// 2. For each of 10,000 cube entities:
for (transform, cube_data) in entities {
    let i = cube_data.z * 100 + cube_data.x;

    if distance_to_camera <= STREAM_RADIUS {
        if cache[i] == 0.0 {                        // not yet loaded?
            file.seek(i * 9);                       // ← O(1): jump to exact byte
            file.read_exact(&mut 9_byte_buffer);    // ← read only this cell
            let height = parse_f32(buffer);         // ← parse " 60.1234\n"
            cache[i] = height;                      // ← mark as loaded
        }
        set_cube_y(cache[i]);                       // ← apply height
    } else {
        set_cube_y(0.0);                            // ← out of range → ground
        cache[i] = 0.0;                             // ← will reload on re-entry
    }
}
```

### Why This Is Efficient

|                          | What happens                                                   |
| ------------------------ | -------------------------------------------------------------- |
| **File opened**          | Once per frame                                                 |
| **Bytes read from disk** | Only uncached cells in range (0 when stationary)               |
| **Bytes skipped**        | All out-of-range cells + all already-cached cells              |
| **OS syscalls**          | One `lseek` + one `read` per newly-visible cell                |
| **RAM allocated**        | Fixed 40 KB cache (`10,000 × f32`) — allocated once at startup |

### The OS Perspective
```
height_data.json on disk:
┌────────┬────────┬────────┬────────┬─────┐
│ line 0 │ line 1 │ line 2 │ line 3 │ ... │  9 bytes each
└────────┴────────┴────────┴────────┴─────┘
     ↑                ↑
  offset 0        offset 2×9=18

seek(18)  → OS moves file cursor to byte 18
read(9)   → OS fetches the disk block containing bytes 18-26
```

The filesystem translates the byte offset to physical disk blocks. Only those blocks are fetched — typically 4 KB at a time. Once read, the OS page cache keeps them in memory, so subsequent reads of nearby cells are instant.

### Fixed-Width vs Variable-Width

|                | Variable-width JSON            | Fixed-width text                 |
| -------------- | ------------------------------ | -------------------------------- |
| Example        | `[0.5, 1.2, 60.123]\n`         | `  0.5000\n  1.2000\n 60.1234\n` |
| Line length    | Varies (11–900+ bytes)         | Always 9 bytes                   |
| Find cell N    | Scan from start, counting `\n` | `seek(N × 9)`                    |
| Human-readable | Yes                            | Yes                              |
| File size      | ~80 KB                         | 90 KB                            |

### Data Flow

```
height.png (embedded at compile time)
     │
     ▼  decode_height_map()
Normalized heights (0.0–1.0)
     │
     ▼  sample_height() × 10,000
Vertical values (0.5–60.5)
     │
     ▼  format!("{:>8.4}\n")
height_data.json  ←  written once at startup
     │
     ▼  position_streaming_system (every frame)
     │
     ├─ in range + uncached → seek + read + parse + cache → set Y
     ├─ in range + cached   → use cache → set Y
     └─ out of range        → zero cache + set Y=0
```

## Egui Debug Panels

### Camera Debug
Shows live camera position (X, Y, Z) and rotation (pitch, yaw, roll) in real-time.

### Stream Data
| Field              | Description                                        |
| ------------------ | -------------------------------------------------- |
| Data Source        | Path to the data file (or "Not found")             |
| Stream Radius      | Current radius in world units                      |
| Cells In Range     | How many cubes are inside the circle this frame    |
| Active Data Size   | `cells_in_range × 4 bytes` in KB                   |
| Cache Size         | Total cache allocation (39.1 KB)                   |
| Bytes Read (frame) | Data read from disk this frame (0 when stationary) |
| Bytes Read (total) | Cumulative since startup                           |
| Frames Streamed    | Frame counter                                      |

### Console Log
Prints only when new cells are loaded:
```
[Stream] frame     45 | in_range= 7850 | loaded=  320 cells | read=2.8 KB | total_read=30.7 KB
```

## Key Design Decisions

1. **Text, not binary** — the file is human-readable. You can inspect, edit, or generate it with any tool.
2. **Fixed-width, not delimited** — enables O(1) seek without scanning. Variable-width JSON/CSV cannot do this.
3. **Per-cell, not per-row** — `seek` jumps to individual values, not entire rows. This is the finest granularity possible with a text file.
4. **Cache with zero-sentinel** — `0.0` means "unloaded" (all real heights are > 0.5). No separate boolean mask needed.
5. **Out-of-range = zero** — cells outside the circle are reset to ground level IMMEDIATELY, not left at their last height. This keeps the cache and transforms consistent.
