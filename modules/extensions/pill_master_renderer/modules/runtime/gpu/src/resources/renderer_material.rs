//! What one material becomes on the GPU: the bind groups its draws bind.
//!
//! # Responsibilities
//!
//! - Build the two bind groups a material's draws bind, following the slots
//!   its shader declares ([`RendererMaterial`]). Parameters go to group 2 and
//!   textures to group 3.
//! - Pack the material's parameter values, one 16-byte slot each, in
//!   declaration order. A pass packs through the same two functions, so a
//!   material and a pass cannot hand a shader different layouts.
//! - Fall back to the renderer's default texture for a slot's type, and
//!   refuse a depth slot: depth belongs to a pass, the only thing that can
//!   supply it. A texture whose view shape (2D or cube) differs from its
//!   slot's is refused with the slot named in the error.
//!
//! # Design
//!
//! A material is built during asset sync, when its own content version moves,
//! and never per frame: a frame only binds what is already built. When the
//! shader or a texture a material names is rebuilt, the material is rebuilt
//! too, because the sync drops the version record of every material whose
//! shader or texture changed. The struct keeps the bind groups and never the
//! uniform buffer: a bind group holds what it references, so the buffer lives
//! exactly as long as the group that reads it.

// Standard library
use std::collections::BTreeMap;

// External crates
use pill_core::debug;
use pill_core::telemetry::telemetry_target;

// Current crate
use crate::{
    assets::MaterialParameter,
    error::{RendererError, Result},
    resources::{
        shader_bindings, RendererResourceStorage, RendererShaderHandle, RendererTextureHandle,
    },
};

// --- Handle ---

pill_core::define_slot_key!(RendererMaterialHandle);

// --- Material ---

/// One material's GPU resources: the bind groups its draws bind.
///
/// The game-facing [`Material`](crate::Material) asset says what a surface
/// looks like; this is what that description becomes on the GPU once the
/// shader is known. One is built per material during asset sync and stored
/// beside every other GPU resource.
pub struct RendererMaterial {
    /// Label used in logs and error messages, taken from the material asset.
    pub name: String,
    /// Shader the bind groups were built against.
    ///
    /// Kept because the frame's queue packs it into the sort key, so draws
    /// that share a shader within one rendering order end up adjacent, and
    /// the drawer rebinds pipeline state once per run instead of once per
    /// draw.
    pub shader_handle: RendererShaderHandle,
    /// Values the shader reads, bound at group 2.
    ///
    /// Built from the shader's declared parameter slots, so a shader that
    /// declares none leaves this `None` rather than an empty group.
    pub parameters_bind_group: Option<wgpu::BindGroup>,
    /// Textures the shader samples, bound at group 3.
    ///
    /// One view and one sampler per declared slot. A slot the material left
    /// unbound holds the renderer's default texture for that slot's type. A
    /// shader that declares no texture slot leaves this `None`.
    pub textures_bind_group: Option<wgpu::BindGroup>,
}

impl RendererMaterial {
    /// Builds one material's bind groups against the shader it names.
    ///
    /// `name` is the material's name. It labels the bind groups and names the
    /// material in every error message.
    ///
    /// Both groups follow the slots the shader declares, not the material's
    /// own parameter and texture maps. A value or texture bound under a name
    /// the shader never declares is ignored. A declared parameter with no
    /// value packs as zero, and a declared texture with none gets the
    /// renderer's default.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::RendererResourceNotFound`] when `shader_handle`
    /// names no loaded shader. Every later failure is [`RendererError::Other`]:
    /// a texture the material names that is not loaded, a texture whose view
    /// shape does not match its slot's, a depth texture slot (only a pass can
    /// supply one), a shader that declares slots but has no layout for them,
    /// and a bind group the GPU refuses.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rendering_resource_storage: &RendererResourceStorage,
        name: &str,
        shader_handle: RendererShaderHandle,
        textures: &[(String, RendererTextureHandle)],
        parameters: &BTreeMap<String, MaterialParameter>,
    ) -> Result<Self> {
        let shader = rendering_resource_storage
            .shaders
            .get(shader_handle)
            .ok_or(RendererError::RendererResourceNotFound)?;

        // A material has no texture source beyond the store, so the resolver
        // passed to the shared builder always answers `None`.
        let (parameters_bind_group, textures_bind_group) = shader_bindings::shader_bind_groups(
            device,
            queue,
            rendering_resource_storage,
            &format!("material `{name}`"),
            shader,
            parameters,
            textures,
            |_, _| Ok(None),
        )?;

        let renderer_material = Self {
            name: name.to_string(),
            shader_handle,
            parameters_bind_group,
            textures_bind_group,
        };

        // One line per material built, visible only when the `engine::rendering`
        // target logs at debug.
        let defaulted_texture_names = shader
            .texture_slots
            .keys()
            .filter(|slot_name| {
                !textures
                    .iter()
                    .any(|(bound_name, _)| bound_name == *slot_name)
            })
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        debug!(
            target: telemetry_target::RENDERING,
            material = name,
            shader = shader.name.as_str(),
            defaulted_textures = %defaulted_texture_names,
            "material created"
        );

        Ok(renderer_material)
    }
}
