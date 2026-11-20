//! The components a game places in the world for the master renderer.
//!
//! # Responsibilities
//!
//! - Declare this renderer's own components, one per submodule: the mesh
//!   renderer and the directional light.
//! - Re-export the scene types every renderer shares - the camera component and
//!   the viewport rectangle from the renderer contract, the transform component
//!   from the engine - so a game imports them all from here.
//! - Register the components with the world through [`register_components`],
//!   field layouts included, for the C# mirror codegen and the editor.
//!
//! # Design
//!
//! The component structs are `#[repr(C)]`, which fixes their byte layout for
//! the C# mirror. They derive `Serialize` and `Deserialize` because the reload
//! migration reads a stored component through them and writes it back under the
//! new type. Projects set them and the C# mirror binds their fields: they are
//! the renderer's public scene contract.
//!
//! **Every component pins its shared name.** Without the pinned name the
//! `PillComponent` derive builds one from `module_path!()`, so the identity
//! follows the module path instead of the type. Moving the component to another
//! module or crate would then register it as a *different* component, and the
//! next reload would stop finding the columns a live world already holds for
//! it. The pinned strings do not spell this module's path; keep them verbatim
//! instead of deriving a new one.

mod directional_light;
mod mesh_renderer;

// External crates
pub use pill_engine::common_components::TransformComponent;
use pill_engine::World;
pub use pill_renderer_api::components::{CameraComponent, RenderViewport};

// Current crate
pub use directional_light::DirectionalLightComponent;
pub use mesh_renderer::{MeshRendererComponent, MeshRendererComponentBuilder};

// =============================================================================
// Free Functions
// =============================================================================

/// Registers the renderer's scene components with the world, field layouts
/// included.
///
/// The transform and the camera are registered first, through
/// [`register_contract_components`](pill_renderer_api::components::register_contract_components).
/// The mesh renderer and the directional light follow, through the
/// `__pill_register_*` functions their `PillComponent` derives generate. Those
/// registrations give the C# mirror codegen and the editor each field's name
/// and offset. Without one, neither of them can see a field.
pub fn register_components(world: &mut World) {
    pill_renderer_api::components::register_contract_components(world);
    mesh_renderer::register(world);
    directional_light::register(world);
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::{Component, ComponentId};

    /// Adding the components to a world records their field layouts, which the
    /// C# mirror codegen and the editor read. Without a layout a component is
    /// just bytes to both, and nothing reports it except this test.
    #[test]
    fn registering_the_components_records_their_field_layouts() {
        let mut world = World::new();
        register_components(&mut world);

        let components = [
            (
                "TransformComponent",
                ComponentId::of::<TransformComponent>(),
            ),
            ("CameraComponent", ComponentId::of::<CameraComponent>()),
            (
                "MeshRendererComponent",
                ComponentId::of::<MeshRendererComponent>(),
            ),
            (
                "DirectionalLightComponent",
                ComponentId::of::<DirectionalLightComponent>(),
            ),
        ];
        for (name, component_id) in components {
            let fields = world
                .component_field_layout(component_id)
                .unwrap_or_else(|| panic!("{name} has no field layout after registration"));
            assert!(!fields.is_empty(), "{name} has an empty field layout");
        }

        // One row per declared field, in the order the struct declares them.
        // The C# mirror is generated from these rows, so an empty layout would
        // leave it with nothing to bind by name.
        let field_count = |component_id| {
            world
                .component_field_layout(component_id)
                .map(<[_]>::len)
                .unwrap_or(0)
        };
        assert_eq!(field_count(ComponentId::of::<TransformComponent>()), 3);
        assert_eq!(field_count(ComponentId::of::<CameraComponent>()), 5);
        assert_eq!(field_count(ComponentId::of::<MeshRendererComponent>()), 2);
        assert_eq!(
            field_count(ComponentId::of::<DirectionalLightComponent>()),
            2
        );
    }

    /// Each type declares the same schema hash its registration records. If the
    /// two disagreed, every query that reads the component would be refused,
    /// because the reader would use the wrong field offsets.
    #[test]
    fn declared_layouts_match_the_registered_ones() {
        let mut world = World::new();
        register_components(&mut world);
        // The registry hashes its registered descriptors with the same
        // function, so the two values are comparable.
        let registered_schema_hash = |component_id: ComponentId| {
            world
                .component_field_layout(component_id)
                .filter(|fields| !fields.is_empty())
                .map(pill_engine::component::component_schema_hash)
        };

        assert_eq!(
            TransformComponent::declared_schema_hash(),
            registered_schema_hash(ComponentId::of::<TransformComponent>())
        );
        assert_eq!(
            CameraComponent::declared_schema_hash(),
            registered_schema_hash(ComponentId::of::<CameraComponent>())
        );
        assert_eq!(
            MeshRendererComponent::declared_schema_hash(),
            registered_schema_hash(ComponentId::of::<MeshRendererComponent>())
        );
        assert_eq!(
            DirectionalLightComponent::declared_schema_hash(),
            registered_schema_hash(ComponentId::of::<DirectionalLightComponent>())
        );
        assert!(TransformComponent::declared_schema_hash().is_some());
    }

    /// The pinned shared names are an identity contract. They let the host, a
    /// loaded module and a world that outlived the generation which registered
    /// the component all find the same one. Deriving a name from
    /// `module_path!()` again would make the world silently lose track of the
    /// values it already holds, so the test asserts the exact strings.
    #[test]
    fn the_components_keep_the_shared_names_they_were_pinned_to() {
        assert_eq!(
            TransformComponent::shared_name(),
            Some("pill_master_renderer::component::TransformComponent")
        );
        assert_eq!(
            CameraComponent::shared_name(),
            Some("pill_master_renderer::component::CameraComponent")
        );
        assert_eq!(
            MeshRendererComponent::shared_name(),
            Some("pill_master_renderer::component::MeshRendererComponent")
        );
        assert_eq!(
            DirectionalLightComponent::shared_name(),
            Some("pill_master_renderer::component::DirectionalLightComponent")
        );
    }
}
