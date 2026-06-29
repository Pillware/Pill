use crate::components::{CameraDebugComponent, CubeSceneData};
use crate::constants::{DATA_PATH, GRID_SIZE, STREAM_RADIUS};
use crate::utils::{format_bytes, lock};
use pill_engine::project::*;

// ── Stream debug ─────────────────────────────────────────────────────────

pub fn register_stream_debug_ui(engine: &mut Engine) -> Result<()> {
    let inner = engine
        .get_global_component::<CubeSceneData>()?
        .inner
        .clone();

    engine
        .get_global_component_mut::<EguiManagerComponent>()?
        .register_ui("stream.debug", move |ctx| {
            egui::Window::new("Stream Data")
                .default_open(true)
                .show(ctx, |ui| {
                    let d = lock(&inner);
                    let cache_kb = (GRID_SIZE * GRID_SIZE * 4) as f64 / 1024.0;

                    ui.label(format!(
                        "Data Source: {}",
                        if d.file_exists {
                            DATA_PATH
                        } else {
                            "Not found"
                        }
                    ));
                    ui.label(format!("Stream Radius: {:.0} world units", STREAM_RADIUS));
                    ui.separator();
                    ui.label(format!("Cells In Range: {}", d.cells_in_range));
                    ui.label(format!(
                        "Active Data Size: {:.1} KB",
                        (d.cells_in_range * 4) as f64 / 1024.0
                    ));
                    ui.label(format!("Cache Size: {:.1} KB (full grid)", cache_kb));
                    ui.separator();
                    ui.label(format!(
                        "Bytes Read (frame): {}",
                        format_bytes(d.bytes_read_this_frame)
                    ));
                    ui.label(format!(
                        "Bytes Read (total): {}",
                        format_bytes(d.total_bytes_read)
                    ));
                    ui.label(format!("Frames Streamed: {}", d.frame_count));
                    ui.separator();
                    ui.label("Mode: Fixed-width per-cell streaming");
                });
        });

    Ok(())
}

// ── Camera debug ─────────────────────────────────────────────────────────

pub fn register_camera_debug_ui(engine: &mut Engine) -> Result<()> {
    let info = engine
        .get_global_component::<CameraDebugComponent>()?
        .info
        .clone();

    engine
        .get_global_component_mut::<EguiManagerComponent>()?
        .register_ui("camera.debug", move |ctx| {
            egui::Window::new("Camera Debug")
                .default_open(true)
                .show(ctx, |ui| {
                    let i = lock(&info);
                    ui.label(format!(
                        "Position: ({:.2}, {:.2}, {:.2})",
                        i.pos_x, i.pos_y, i.pos_z
                    ));
                    ui.label(format!(
                        "Rotation: ({:.2}°, {:.2}°, {:.2}°)",
                        i.pitch, i.yaw, i.roll
                    ));
                });
        });

    Ok(())
}
