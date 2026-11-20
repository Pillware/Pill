//! GPU-side storage for every resource the renderer holds.
//!
//! # Responsibilities
//!
//! - Own the slot maps that every GPU resource handle points into.
//! - Install the stand-in textures a shader slot falls back to when a material
//!   or pass binds nothing.
//!
//! # Design
//!
//! One struct carries all five maps and the engine parameters, so asset sync,
//! pass building, and drawing reach every GPU resource through the same
//! reference. The stand-in textures are created at construction and their
//! handles stay in this struct, so a slot that binds nothing falls back to the
//! same white, flat-normal, or black environment texture.

// External crates
use pill_core::slot_map::SlotMap;
use pill_core::telemetry::telemetry_target;

// Current crate
use crate::{
    assets::{ShaderTextureKind, TextureType},
    error::Result,
    resources::{
        EngineParameters, RendererCamera, RendererCameraHandle, RendererMaterial,
        RendererMaterialHandle, RendererMesh, RendererMeshHandle, RendererShader,
        RendererShaderHandle, RendererTexture, RendererTextureHandle,
    },
};

// =============================================================================
// Constants
// =============================================================================

/// Starting capacity of the shader slot map.
///
/// The map starts with room for this many entries and grows when more
/// resources are added; this is not a limit.
pub const INITIAL_SHADER_CAPACITY: usize = 10;
/// Starting capacity of the texture slot map.
pub const INITIAL_TEXTURE_CAPACITY: usize = 10;
/// Starting capacity of the material slot map.
pub const INITIAL_MATERIAL_CAPACITY: usize = 10;
/// Starting capacity of the mesh slot map.
pub const INITIAL_MESH_CAPACITY: usize = 10;
/// Starting capacity of the camera slot map.
pub const INITIAL_CAMERA_CAPACITY: usize = 10;

// =============================================================================
// RendererResourceStorage
// =============================================================================

/// Every GPU resource the renderer holds, keyed by the handles it hands out.
///
/// One storage carries all five maps plus the engine parameters, so asset
/// sync, pass building, and drawing reach any resource through the same
/// borrow. The stand-in handles sit beside the maps that own their textures,
/// so a slot that binds nothing still finds one.
pub struct RendererResourceStorage {
    /// Shader pipelines and their bind group layouts, one per shader handle.
    pub(crate) shaders: SlotMap<RendererShaderHandle, RendererShader>,
    /// Material bind groups, one per material handle.
    pub(crate) materials: SlotMap<RendererMaterialHandle, RendererMaterial>,
    /// Uploaded textures, each with its view and sampler.
    pub(crate) textures: SlotMap<RendererTextureHandle, RendererTexture>,
    /// Mesh vertex and index buffers, ready to draw.
    pub(crate) meshes: SlotMap<RendererMeshHandle, RendererMesh>,
    /// Camera uniform buffers and the bind groups that carry them.
    pub(crate) cameras: SlotMap<RendererCameraHandle, RendererCamera>,
    /// The engine-wide uniform buffer and bind group, refilled every frame.
    pub(crate) engine_parameters: EngineParameters,
    /// The stand-in bound when a colour texture slot binds nothing.
    pub(crate) default_color_texture: RendererTextureHandle,
    /// The stand-in bound when a normal texture slot binds nothing.
    pub(crate) default_normal_texture: RendererTextureHandle,
    /// The stand-in bound when an equirect slot binds nothing: one black texel,
    /// so a sky with no environment map renders black instead of glaring
    /// white.
    pub(crate) default_equirect_texture: RendererTextureHandle,
    /// The stand-in bound when a cubemap slot binds nothing: six black faces.
    pub(crate) default_cubemap_texture: RendererTextureHandle,
}

impl RendererResourceStorage {
    /// Creates the storage with each map sized to its starting capacity, and
    /// installs the colour, normal, equirect and cubemap stand-in textures.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::RendererError::Other`] naming the stand-in when
    /// the driver refuses its texture, upload, or sampler: `default_color`,
    /// `default_normal`, `default_equirect` or `default_cubemap`. Nothing else
    /// in the constructor returns an error.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self> {
        // Step 1: Build each slot map at its starting capacity. The four
        // stand-in handles are placeholders that Step 2 replaces with the
        // textures it creates.
        let mut storage = RendererResourceStorage {
            shaders: SlotMap::<RendererShaderHandle, RendererShader>::with_capacity_and_key(
                INITIAL_SHADER_CAPACITY,
            ),
            textures: SlotMap::<RendererTextureHandle, RendererTexture>::with_capacity_and_key(
                INITIAL_TEXTURE_CAPACITY,
            ),
            materials: SlotMap::<RendererMaterialHandle, RendererMaterial>::with_capacity_and_key(
                INITIAL_MATERIAL_CAPACITY,
            ),
            meshes: SlotMap::<RendererMeshHandle, RendererMesh>::with_capacity_and_key(
                INITIAL_MESH_CAPACITY,
            ),
            cameras: SlotMap::<RendererCameraHandle, RendererCamera>::with_capacity_and_key(
                INITIAL_CAMERA_CAPACITY,
            ),
            engine_parameters: EngineParameters::new(device),
            default_color_texture: RendererTextureHandle::new(
                0,
                std::num::NonZeroU32::new(1).unwrap(),
            ),
            default_normal_texture: RendererTextureHandle::new(
                0,
                std::num::NonZeroU32::new(1).unwrap(),
            ),
            default_equirect_texture: RendererTextureHandle::new(
                0,
                std::num::NonZeroU32::new(1).unwrap(),
            ),
            default_cubemap_texture: RendererTextureHandle::new(
                0,
                std::num::NonZeroU32::new(1).unwrap(),
            ),
        };

        // Step 2: Create the stand-in textures and point the handles at them.
        storage.install_default_textures(device, queue)?;
        Ok(storage)
    }

    /// Returns the stand-in texture a slot of `kind` falls back to when it
    /// binds nothing. Returns `None` for a depth slot, because only a pass's
    /// own input can supply one.
    pub(crate) fn default_texture_for(
        &self,
        kind: ShaderTextureKind,
    ) -> Option<RendererTextureHandle> {
        match kind.texture_type()? {
            TextureType::Color => Some(self.default_color_texture),
            TextureType::Normal => Some(self.default_normal_texture),
            TextureType::Equirect => Some(self.default_equirect_texture),
            TextureType::Cubemap => Some(self.default_cubemap_texture),
        }
    }

    /// Installs the stand-in textures a texture slot falls back to when it
    /// binds nothing: white for colour slots, the flat normal map for normal
    /// slots, black for equirect and cubemap slots.
    ///
    /// The colour and normal stand-ins are files the data module ships, read
    /// through its store. A file that cannot be read falls back to the texel
    /// built here, so a missing file cannot stop the renderer from starting.
    ///
    /// # Errors
    ///
    /// Returns the error from [`RendererTexture::new_texture`] when the driver
    /// refuses one of the textures.
    fn install_default_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<()> {
        self.default_color_texture = self.textures.insert(Self::create_stand_in_texture(
            device,
            queue,
            "default_color",
            Some(pill_master_renderer_data::DEFAULT_COLOR_TEXTURE),
            TextureType::Color,
            &[255, 255, 255, 255],
        )?);
        self.default_normal_texture = self.textures.insert(Self::create_stand_in_texture(
            device,
            queue,
            "default_normal",
            Some(pill_master_renderer_data::DEFAULT_NORMAL_TEXTURE),
            TextureType::Normal,
            &[128, 128, 255, 255],
        )?);
        // Half-float zero is stored as zero bytes: one black texel, and six of
        // them for the cube's faces. The data module ships no environment
        // stand-in file, so these two are built here.
        self.default_equirect_texture = self.textures.insert(Self::create_stand_in_texture(
            device,
            queue,
            "default_equirect",
            None,
            TextureType::Equirect,
            &[0; 8],
        )?);
        self.default_cubemap_texture = self.textures.insert(Self::create_stand_in_texture(
            device,
            queue,
            "default_cubemap",
            None,
            TextureType::Cubemap,
            &[0; 48],
        )?);
        Ok(())
    }

    /// Creates one stand-in texture.
    ///
    /// `name` labels the texture and names it in any error. When `path` names a
    /// file the module ships and the file can be read, the texture is built
    /// from it; otherwise it is a 1x1 texture built from `texel_bytes`. A file
    /// that cannot be read is logged and replaced by that built-in texel.
    ///
    /// # Errors
    ///
    /// Returns the error from [`RendererTexture::new_texture`] when the driver
    /// refuses the texture, the upload, or the sampler.
    fn create_stand_in_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        path: Option<&str>,
        texture_type: TextureType,
        texel_bytes: &[u8],
    ) -> Result<RendererTexture> {
        if let Some(path) = path {
            match pill_master_renderer_data::load_shipped_texture(path, texture_type) {
                Ok(texture) => {
                    return RendererTexture::new_texture(
                        device,
                        queue,
                        Some(name),
                        &texture.rgba,
                        texture.width,
                        texture.height,
                        texture.texture_type,
                    );
                }
                Err(error) => pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    "stand-in texture `{path}` is not readable; using the built-in texel: {error}"
                ),
            }
        }
        RendererTexture::new_texture(device, queue, Some(name), texel_bytes, 1, 1, texture_type)
    }
}
