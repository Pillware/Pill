// ── Grid ─────────────────────────────────────────────────────────────────

pub const GRID_SIZE: usize = 100;
pub const CUBE_SPACING: f32 = 2.0;
pub const MAX_HEIGHT: f32 = 60.0;

// ── Streaming ────────────────────────────────────────────────────────────

pub const STREAM_RADIUS: f32 = 100.0;
pub const LINE_BYTES: usize = 9; // "{:>8.4}\n" — fixed-width for O(1) seek
pub const DATA_PATH: &str = "res/data/height_data.json";
