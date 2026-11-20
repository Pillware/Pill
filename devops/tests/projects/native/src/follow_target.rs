//! The camera's follow target: an entity reference the Inspector can point.
//!
//! # Responsibilities
//!
//! - Declare [`FollowTarget`], a component whose field names another entity.
//! - Read that reference in a system, which is what makes it live data rather
//!   than a stored number the engine never uses.
//! - Draw the reference as a line, as the component's own gizmo, so the
//!   editor's gizmo pass has a subject in this project.
//!
//! The component exists as the editor's test subject for entity references
//! (W-0048 Stage 6): it is the only field in the example that holds an
//! `Entity`, so a picture of the Inspector shows what a reference row draws.

use pill_engine::*;
use pill_master_renderer_data::TransformComponent;

/// The colour the follow line is drawn in.
const FOLLOW_COLOR: [f32; 4] = [1.0, 0.85, 0.25, 1.0];

/// Follows the entity it names: the camera drifts a fraction of the way to the
/// target's transform every frame.
///
/// `#[repr(C)]` because the host copies the row across hot-reload generations,
/// and the field layout the derive registers is what the Inspector draws. The
/// gizmo draws a line to the entity the reference names.
#[component(gizmo = crate::follow_target::draw_follow_target_gizmo)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FollowTarget {
    /// The entity this one follows.
    ///
    /// Tagged `entity`, so the Inspector draws a reference row - the referenced
    /// entity's label, a picker, and a clear button - rather than two opaque
    /// numbers.
    #[pill(tooltip("The entity this one follows"))]
    pub target: Entity,
    /// How much of the distance to close each frame, from 0 to 1.
    #[pill(range(0.0, 1.0), tooltip("Fraction of the distance closed each frame"))]
    pub weight: f32,
}

/// Registers the component together with its field layout.
///
/// The derive's `__pill_register_*` is what records the layout; registering the
/// storage alone would leave the component an opaque blob to the Inspector.
pub(crate) fn register(world: &mut World) {
    __pill_register_FollowTarget(world);
}

/// Draws the follow reference: a line from the follower to the entity it names.
///
/// The engine's gizmo pass calls this once per entity that carries a
/// [`FollowTarget`]. A target that does not exist draws a short spike in the
/// follower's own position, so a cleared reference is visible rather than
/// silent.
pub fn draw_follow_target_gizmo(
    follow: &FollowTarget,
    context: &GizmoContext<'_>,
    draw: &mut GizmoDraw,
) {
    let origin = context.translation();
    let target = context
        .get_component_of::<TransformComponent>(follow.target)
        .map_or(origin, |transform| transform.translation);
    draw.line(origin, target, FOLLOW_COLOR);
    draw.label(origin, format!("follow {:.2}", follow.weight), FOLLOW_COLOR);
}

/// Drifts every follower toward the transform its target carries.
///
/// A target that has been destroyed leaves the follower where it is: the
/// reference is a handle, and a handle to nothing is a no-op rather than an
/// error the frame would report.
#[system(name = "follow_target", order = 540)]
pub(crate) fn follow_target_system(
    mut followers: Query<(&FollowTarget, &mut TransformComponent)>,
    mut targets: Query<(&TransformComponent, Entity)>,
) -> Result<(), SystemError> {
    let target_rows: Vec<(Entity, [f32; 3])> = targets
        .iter_mut()
        .map(|(transform, entity)| (entity, transform.translation))
        .collect();
    for (follow, mut transform) in followers.iter_mut() {
        let Some((_, target_translation)) = target_rows
            .iter()
            .find(|(entity, _)| *entity == follow.target)
        else {
            continue;
        };
        let weight = follow.weight.clamp(0.0, 1.0);
        for (axis, target_axis) in target_translation.iter().enumerate() {
            transform.translation[axis] += (target_axis - transform.translation[axis]) * weight;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference field is tagged `entity`, so the field layer reads it as
    /// a reference and the Inspector draws a row for it.
    #[test]
    fn the_target_is_an_entity_reference_field() {
        let layout = FollowTarget::FIELD_LAYOUT;
        let target = layout
            .iter()
            .find(|field| field.name == "target")
            .expect("the component declares a target field");
        assert_eq!(target.type_tag, "entity");
        assert_eq!(target.size, std::mem::size_of::<Entity>());
    }

    /// An unset reference is the null handle, not entity 0 - slot 0 is a real
    /// slot the world hands out.
    #[test]
    fn a_defaulted_reference_is_the_null_handle() {
        assert!(FollowTarget::default().target.is_null());
    }
}
