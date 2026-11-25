//! Game-facing renderer assets stored in the world's `AssetManager`.
//!
//! # Responsibilities
//!
//! - Own the asset types a project draws from - [`Mesh`], [`Texture`],
//!   [`Shader`], [`Material`], [`RenderPass`], [`RenderingPipeline`] - and
//!   re-export them flat, so a game names them without their submodule.
//! - Carry each type's builder and binding declarations: [`MaterialBuilder`],
//!   [`ShaderBuilder`], the parameter and texture slot types, and the helpers
//!   that key slot lists by the names they carry.
//! - Register every asset type behind `dyn Asset`, the bound every
//!   `AssetManager` method asks for, so the store holds as many of each as a
//!   game wants.
//! - Provide [`asset_key`], which packs a handle's index and generation into
//!   the `u64` the renderer's caches use as their key.
//!
//! # Design
//!
//! Each asset type is plain data plus a declaration. A game builds it, the
//! `AssetManager` owns it, and the renderer reads it while building a frame.
//! The GPU side - caches, pipelines and bind groups - lives in the GPU module
//! (`pill_renderer`), which links this crate. [`asset_key`] puts a
//! handle's generation above its index, so one `u64` tells a cache which asset
//! is meant and which version of it a resource was built from.

mod material;
mod mesh;
mod render_pass;
mod rendering_pipeline;
mod shader;
mod texture;

// External crates
use pill_engine::{Asset, Handle};
// The pass vocabulary belongs to the renderer contract
// (`pill_renderer_api::frame`), which also carries it on the frame's resolved
// pass. It is re-exported here so `assets::` reaches it beside the types that
// use it.
pub use pill_renderer_api::frame::{CullMode, MaterialParameter, PassKind, PassTarget};

// Current crate
pub use material::{
    neutral_material_parameters, new_material_parameters, Material, MaterialBuilder,
    MaterialDocument, MaterialTexture, DEFAULT_MATERIAL_PATH,
};
pub use mesh::{Mesh, MeshImportSettings, MeshVertex};
pub use render_pass::RenderPass;
pub use render_pass::RenderPassDocument;
pub use rendering_pipeline::{
    PipelinePass, PipelinePassDocument, RenderingPipeline, RenderingPipelineDocument,
};
pub use shader::{slots_by_name, SlotName};
pub use shader::{
    Shader, ShaderBuilder, ShaderImportSettings, ShaderParameterSlot, ShaderParameterType,
    ShaderTextureKind, ShaderTextureSlot,
};
pub use texture::{
    cube_face_direction, equirect_uv, load_shipped_texture, project_equirect_to_cubemap, Texture,
    TextureImportSettings, TextureType, DEFAULT_COLOR_TEXTURE, DEFAULT_NORMAL_TEXTURE,
};

// =============================================================================
// Free Functions
// =============================================================================

/// Turns a typed asset handle into the key the renderer's caches use.
///
/// The generation is packed above the index, so two versions of one asset
/// never share a key.
pub fn asset_key<T: Asset>(asset_handle: Handle<T>) -> u64 {
    (u64::from(asset_handle.generation()) << 32) | u64::from(asset_handle.index())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared names of the asset types are a fixed contract. They are what
    /// lets this crate, the GPU module and a project reach the same column for
    /// a type, so the exact strings are asserted here.
    #[test]
    fn the_asset_types_keep_their_pinned_shared_names() {
        let shared_names = [
            Mesh::shared_name(),
            Texture::shared_name(),
            Shader::shared_name(),
            Material::shared_name(),
            RenderPass::shared_name(),
            RenderingPipeline::shared_name(),
        ];
        assert_eq!(
            shared_names,
            [
                Some("pill_rendering::Mesh"),
                Some("pill_rendering::Texture"),
                Some("pill_rendering::Shader"),
                Some("pill_rendering::Material"),
                Some("pill_rendering::RenderPass"),
                Some("pill_rendering::RenderingPipeline"),
            ]
        );
    }

    /// Every rendering asset keeps the name it was published under before the
    /// crate was renamed, so a `.meta` file or a standalone document written
    /// then still loads as the same type.
    #[test]
    fn the_asset_types_declare_the_names_they_were_stored_under() {
        for (former_names, former_name) in [
            (Mesh::former_shared_names(), "pill_master_renderer::Mesh"),
            (
                Texture::former_shared_names(),
                "pill_master_renderer::Texture",
            ),
            (
                Shader::former_shared_names(),
                "pill_master_renderer::Shader",
            ),
            (
                Material::former_shared_names(),
                "pill_master_renderer::Material",
            ),
            (
                RenderPass::former_shared_names(),
                "pill_master_renderer::RenderPass",
            ),
            (
                RenderingPipeline::former_shared_names(),
                "pill_master_renderer::RenderingPipeline",
            ),
        ] {
            assert_eq!(former_names, [former_name]);
        }
    }

    /// An asset type's identity is the hash of its pinned name, not the
    /// `TypeId` that only means something inside one binary. That is what
    /// makes each of the six columns one column in this crate, the GPU module
    /// and a project.
    ///
    /// The attribute that declares the name also stores the hash in a `const`,
    /// so every call returns the same number and nothing is hashed at runtime.
    #[test]
    fn an_asset_identity_is_the_hash_of_its_pinned_name() {
        for (identity, name) in [
            (Mesh::shared_identity(), Mesh::shared_name()),
            (Texture::shared_identity(), Texture::shared_name()),
            (Shader::shared_identity(), Shader::shared_name()),
            (Material::shared_identity(), Material::shared_name()),
            (RenderPass::shared_identity(), RenderPass::shared_name()),
            (
                RenderingPipeline::shared_identity(),
                RenderingPipeline::shared_name(),
            ),
        ] {
            let name = name.expect("every renderer asset pins a name");
            assert_eq!(
                identity,
                Some(pill_engine::component::shared_component_identity(name)),
                "`{name}`"
            );
        }
    }
}
