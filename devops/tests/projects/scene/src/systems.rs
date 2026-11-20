//! Animation systems used by the scene.
//!
//! # Responsibilities
//!
//! - Spin tagged models around their local Y axis each frame.

use crate::TagAlphaComponent;
use pill_engine::{system, Query, Res, SystemError, Time};
use pill_master_renderer_data::TransformComponent;

/// Rotates each tagged model around its local Y axis at 90 degrees per second.
#[system(name = "scene_rotation", order = 510)]
pub(crate) fn rotation_system(
    time: Res<Time>,
    mut models: Query<(&mut TransformComponent, &TagAlphaComponent)>,
) -> Result<(), SystemError> {
    let Some(time) = time.get() else {
        return Ok(());
    };
    let step = glam::Quat::from_rotation_y(90.0_f32.to_radians() * time.delta_seconds());
    for (mut transform, _) in models.iter_mut() {
        let current = glam::Quat::from_array(pill_engine::common_components::rotation_or_identity(
            transform.rotation,
        ));
        transform.rotation = (step * current * 25.0).normalize().to_array();
    }
    Ok(())
}
