use crate::constants::GRID_SIZE;
use pill_engine::{define_component, define_global_component};
use std::sync::{Arc, Mutex};

// ── Per-entity components ────────────────────────────────────────────────

define_component!(CameraMovementComponent {
    move_speed: f32,
    rotate_speed: f32,
});

define_component!(CubeData {
    grid_x: usize,
    grid_z: usize,
    target_y: f32,
});

// ── Global components ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CubeSceneDataInner {
    pub file_exists: bool,
    pub heights: Vec<f32>,
    pub cells_in_range: usize,
    pub bytes_read_this_frame: u64,
    pub total_bytes_read: u64,
    pub frame_count: u64,
}

define_global_component!(CubeSceneData {
    inner: Arc<Mutex<CubeSceneDataInner>>,
});

impl Default for CubeSceneData {
    fn default() -> Self {
        let n = GRID_SIZE * GRID_SIZE;
        CubeSceneData {
            inner: Arc::new(Mutex::new(CubeSceneDataInner {
                file_exists: false,
                heights: vec![0.0; n],
                cells_in_range: 0,
                bytes_read_this_frame: 0,
                total_bytes_read: 0,
                frame_count: 0,
            })),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CameraDebugInfo {
    pub pos_x: f32,
    pub pos_y: f32,
    pub pos_z: f32,
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

impl Default for CameraDebugInfo {
    fn default() -> Self {
        Self {
            pos_x: 0.0,
            pos_y: 0.0,
            pos_z: 0.0,
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        }
    }
}

define_global_component!(CameraDebugComponent {
    info: Arc<Mutex<CameraDebugInfo>>,
});

impl Default for CameraDebugComponent {
    fn default() -> Self {
        Self {
            info: Arc::new(Mutex::new(CameraDebugInfo::default())),
        }
    }
}
