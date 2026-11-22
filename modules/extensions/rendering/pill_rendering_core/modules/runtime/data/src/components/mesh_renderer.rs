//! The mesh renderer component, and the builder that assembles one.
//!
//! # Responsibilities
//!
//! - Define [`MeshRendererComponent`], the pair of handles that makes an
//!   entity drawable, and [`MeshRendererComponentBuilder`], which fills in
//!   those two handles.
//!
//! # Design
//!
//! Shared and persistable: the managed mirror binds the same two handles, and
//! the reload migration keeps their values. The shared name is pinned rather
//! than derived from the module path; see [`crate::components`].

// External crates
use pill_engine::{component, Handle, World};

// Current crate
use crate::assets::{Material, Mesh};

// =============================================================================
// MeshRendererComponentBuilder
// =============================================================================

/// Assembles a [`MeshRendererComponent`] one handle at a time.
///
/// It starts from the component's `Default`, so a caller sets only the handles
/// it has, and `build` returns the finished component.
pub struct MeshRendererComponentBuilder {
    component: MeshRendererComponent,
}

impl MeshRendererComponentBuilder {
    /// Sets the mesh the entity draws with.
    pub fn mesh(mut self, mesh: &Handle<Mesh>) -> Self {
        self.component.mesh = *mesh;
        self
    }

    /// Sets the material that shades the mesh.
    pub fn material(mut self, material: &Handle<Material>) -> Self {
        self.component.material = *material;
        self
    }

    /// Finishes the builder and returns the assembled component.
    pub fn build(self) -> MeshRendererComponent {
        self.component
    }
}

// =============================================================================
// MeshRendererComponent
// =============================================================================

/// What an entity draws: a mesh paired with the material that shades it.
///
/// The renderer's post-update `rendering` system collects every entity
/// carrying this component and emits one instance for each pair whose handles
/// still resolve in the asset manager. A dangling handle therefore draws
/// nothing instead of failing the frame.
#[component(shared = "pill_master_renderer::MeshRendererComponent", persistable)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MeshRendererComponent {
    /// Mesh this entity draws.
    pub mesh: Handle<Mesh>,
    /// Material its instances are shaded with; each batch takes its pipeline
    /// from this material's shader.
    pub material: Handle<Material>,
}

impl MeshRendererComponent {
    /// Starts a builder for an entity that should draw.
    pub fn builder() -> MeshRendererComponentBuilder {
        MeshRendererComponentBuilder {
            component: Self::default(),
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Registers [`MeshRendererComponent`] with the world, field layout included.
///
/// The derive generates `__pill_register_MeshRendererComponent` as a private
/// item of this module, so [`crate::components::register_components`] cannot
/// call it directly and goes through this function instead.
pub(crate) fn register(world: &mut World) {
    __pill_register_MeshRendererComponent(world);
}
