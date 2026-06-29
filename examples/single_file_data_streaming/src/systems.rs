use crate::components::{CameraMovementComponent, CubeData, CubeSceneData};
use crate::constants::{CUBE_SPACING, DATA_PATH, GRID_SIZE, LINE_BYTES, STREAM_RADIUS};
use crate::utils::{format_bytes, in_circle, lock, world_to_grid};
use pill_engine::project::*;
use std::io::{Read, Seek, SeekFrom};

// ── Camera movement ──────────────────────────────────────────────────────

pub fn camera_movement_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;

    let input = engine.get_global_component::<InputComponent>()?;
    let w = input.get_key(KeyboardKey::KeyW);
    let s = input.get_key(KeyboardKey::KeyS);
    let d = input.get_key(KeyboardKey::KeyD);
    let a = input.get_key(KeyboardKey::KeyA);
    let q = input.get_key(KeyboardKey::KeyQ);
    let e = input.get_key(KeyboardKey::KeyE);
    let left = input.get_key(KeyboardKey::ArrowLeft);
    let right = input.get_key(KeyboardKey::ArrowRight);
    let up = input.get_key(KeyboardKey::ArrowUp);
    let down = input.get_key(KeyboardKey::ArrowDown);
    drop(input);

    for (_entity, transform, mov) in
        engine.iterate_two_components_mut::<TransformComponent, CameraMovementComponent>()?
    {
        let forward = transform.get_forward_direction();
        let right_dir = transform.get_right_direction();

        let speed = mov.move_speed * dt;
        let mut delta = Vector3f::ZERO;
        if w {
            delta -= forward;
        }
        if s {
            delta += forward;
        }
        if d {
            delta += right_dir;
        }
        if a {
            delta -= right_dir;
        }
        if q {
            delta -= Vector3f::Y;
        }
        if e {
            delta += Vector3f::Y;
        }

        if delta != Vector3f::ZERO {
            transform.translate_world(delta.normalize() * speed);
        }

        let rot = mov.rotate_speed * dt;
        if left {
            transform.rotate_around_axis(rot, Vector3f::Y);
        }
        if right {
            transform.rotate_around_axis(-rot, Vector3f::Y);
        }
        if up {
            transform.rotate_around_axis(-rot, Vector3f::X);
        }
        if down {
            transform.rotate_around_axis(rot, Vector3f::X);
        }
    }

    Ok(())
}

// ── Position streaming ───────────────────────────────────────────────────

pub fn position_streaming_system(engine: &mut Engine) -> Result<()> {
    let inner = engine
        .get_global_component::<CubeSceneData>()?
        .inner
        .clone();

    // Locate camera
    let (cam_gx, cam_gz) = {
        let mut pos = None;
        for (_, t, _) in
            engine.iterate_two_components_mut::<TransformComponent, CameraComponent>()?
        {
            pos = Some(t.position);
            break;
        }
        match pos {
            Some(p) => world_to_grid(p.x, p.z),
            None => return Ok(()),
        }
    };

    let radius = STREAM_RADIUS / CUBE_SPACING;

    // Open file
    let mut file = match std::fs::File::open(DATA_PATH) {
        Ok(f) => f,
        Err(_) => {
            lock(&inner).file_exists = false;
            return Ok(());
        }
    };
    lock(&inner).file_exists = true;

    let mut bytes_read: u64 = 0;
    let mut cells_in_range: usize = 0;
    let mut line_buf = [0u8; LINE_BYTES];
    let mut data = lock(&inner);

    for (_entity, _transform, cube) in
        engine.iterate_two_components_mut::<TransformComponent, CubeData>()?
    {
        let idx = cube.grid_z * GRID_SIZE + cube.grid_x;

        if in_circle(cube.grid_x, cube.grid_z, cam_gx, cam_gz, radius) {
            cells_in_range += 1;
            if data.heights[idx] == 0.0 {
                file.seek(SeekFrom::Start(idx as u64 * LINE_BYTES as u64))?;
                file.read_exact(&mut line_buf)?;
                bytes_read += LINE_BYTES as u64;
                let h: f32 = std::str::from_utf8(&line_buf[..8])?
                    .trim()
                    .parse()
                    .unwrap_or(0.0);
                data.heights[idx] = h;
                cube.target_y = h;
            } else {
                cube.target_y = data.heights[idx];
            }
        } else {
            data.heights[idx] = 0.0;
            cube.target_y = 0.0;
        }
    }

    // Stats
    data.cells_in_range = cells_in_range;
    data.bytes_read_this_frame = bytes_read;
    data.total_bytes_read += bytes_read;
    data.frame_count += 1;

    if bytes_read > 0 {
        println!(
            "[Stream] frame {:>6} | in_range={:>5} | loaded={:>5} cells | read={} | total_read={}",
            data.frame_count,
            cells_in_range,
            bytes_read / LINE_BYTES as u64,
            format_bytes(bytes_read),
            format_bytes(data.total_bytes_read),
        );
    }
    drop(data);

    Ok(())
}

// ── Cube lerp ────────────────────────────────────────────────────────────

const LERP_SPEED: f32 = 8.0;

pub fn cube_lerp_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
    let t = (LERP_SPEED * dt).min(1.0);

    for (_entity, transform, cube) in
        engine.iterate_two_components_mut::<TransformComponent, CubeData>()?
    {
        let pos = transform.position;
        let y = pos.y + (cube.target_y - pos.y) * t;
        transform.set_position(Vector3f::new(pos.x, y, pos.z));
    }

    Ok(())
}

// ── Camera debug update ──────────────────────────────────────────────────

pub fn update_camera_debug_system(engine: &mut Engine) -> Result<()> {
    let info = engine
        .get_global_component::<crate::components::CameraDebugComponent>()?
        .info
        .clone();

    for (_, transform, _) in
        engine.iterate_two_components_mut::<TransformComponent, CameraComponent>()?
    {
        let mut i = lock(&info);
        i.pos_x = transform.position.x;
        i.pos_y = transform.position.y;
        i.pos_z = transform.position.z;
        i.pitch = transform.rotation.x;
        i.yaw = transform.rotation.y;
        i.roll = transform.rotation.z;
    }

    Ok(())
}
