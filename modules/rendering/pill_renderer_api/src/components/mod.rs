//! The scene types that are part of the renderer contract.
//!
//! # Responsibilities
//!
//! - Declare [`CameraComponent`], the camera [`crate::frame::RenderFrame`]
//!   carries for each surface, and [`RenderViewport`], the rectangle a frontend
//!   points a renderer at.
//! - Register the contract's components through
//!   [`register_contract_components`], field layout included.
//!
//! # Design
//!
//! Only what every renderer must understand lives here. A renderer's own
//! components - the master renderer's mesh renderer and directional light, for
//! instance - live in that renderer's data crate, which registers these first
//! and its own after.
//!
//! **The camera sets its shared name explicitly.** The `PillComponent` derive
//! would otherwise build the name from `module_path!()`. A name built that way
//! registers a *different* component, and the world would no longer find the
//! columns it already holds for this one.

mod camera;
mod viewport;

// External crates
use pill_engine::World;

// Current crate
pub use camera::CameraComponent;
pub use viewport::RenderViewport;

/// Registers the contract's components - the engine's transform and the
/// camera - with the world, and records each component's field layout.
///
/// A renderer's data crate calls this before registering its own components.
pub fn register_contract_components(world: &mut World) {
    pill_engine::common_components::register_transform_component(world);
    camera::register(world);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::common_components::TransformComponent;
    use pill_engine::{Component, ComponentId};

    /// Registering the contract's components records their field layouts, and
    /// the camera reports its declared shared name.
    #[test]
    fn the_contract_components_register_with_their_layouts() {
        let mut world = World::new();
        register_contract_components(&mut world);

        let field_count = |component_id| {
            world
                .component_field_layout(component_id)
                .map(<[_]>::len)
                .unwrap_or(0)
        };
        assert_eq!(field_count(ComponentId::of::<TransformComponent>()), 3);
        assert_eq!(field_count(ComponentId::of::<CameraComponent>()), 5);
        assert_eq!(
            CameraComponent::shared_name(),
            Some("pill_rendering::CameraComponent")
        );
        assert_eq!(
            CameraComponent::former_shared_names(),
            ["pill_master_renderer::CameraComponent"]
        );
    }
}
