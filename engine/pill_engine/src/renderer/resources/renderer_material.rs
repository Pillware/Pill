use crate::{
    config::{
        DEFAULT_COLOR_TEXTURE_NAME, DEFAULT_EMISSIVE_TEXTURE_NAME,
        DEFAULT_METALLIC_ROUGHNESS_TEXTURE_NAME, DEFAULT_NORMAL_TEXTURE_NAME,
    },
    graphics::{RendererMaterialHandle, RendererTextureHandle},
    renderer::resources::RendererTexture,
    resources::{Resource, ResourceManager, ResourceStorage},
};
use pill_core::{debug, LogContext, PillError, PillStyle, PillTypeMapKey, RendererError, Result};

/// Material parameter uniform (bind group 2). Mirrors `MaterialParams` in pbr_opaque_fragment.hlsl:
/// three fully packed 16-byte std140 rows.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct MaterialParams {
    pub(crate) base_color: [f32; 3],
    pub(crate) roughness: f32,
    pub(crate) emissive: [f32; 3],
    pub(crate) metallic: f32,
    pub(crate) clearcoat: f32,
    pub(crate) clearcoat_roughness: f32,
    pub(crate) opacity: f32,
    pub(crate) _padding: f32,
}

/// Renderer textures of a material; `None` falls back to the engine default texture of that slot.
#[derive(Clone, Copy, Default)]
pub(crate) struct MaterialTextures {
    pub(crate) base_color: Option<RendererTextureHandle>,
    pub(crate) normal: Option<RendererTextureHandle>,
    pub(crate) metallic_roughness: Option<RendererTextureHandle>,
    pub(crate) emissive: Option<RendererTextureHandle>,
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// Bind group 1 layout: (texture, sampler) pairs at bindings 0-7 for
/// base_color, normal, metallic_roughness, emissive. Shared by RendererMaterial and PassPBROpaque.
pub(crate) fn material_textures_layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    (0..4)
        .flat_map(|slot| [texture_entry(slot * 2), sampler_entry(slot * 2 + 1)])
        .collect()
}

/// Bind group 2 layout: one MaterialParams uniform buffer.
pub(crate) fn material_params_layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }]
}

pub struct RendererMaterial {
    pub name: String,
    /// Opacity < 1: drawn by the translucent PBR pass (blended, back to front) instead of the opaque one.
    pub translucent: bool,
    pub parameters_bind_group: wgpu::BindGroup,
    pub textures_bind_group: wgpu::BindGroup,
    parameters_uniform_buffer: wgpu::Buffer,
}

impl RendererMaterial {
    /// Creates the parameter uniform buffer and both bind groups of a material.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resource_manager: &ResourceManager,
        name: &str,
        parameters: &MaterialParams,
        textures: &MaterialTextures,
    ) -> Result<Self> {
        debug!(LogContext::Rendering => "Creating material {}", name.name_style());

        let parameters_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("{}_material_parameters_buffer", name)),
            size: std::mem::size_of::<MaterialParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &parameters_uniform_buffer,
            0,
            bytemuck::bytes_of(parameters),
        );

        // wgpu deduplicates structurally identical layouts, so these are compatible with the
        // layouts PassPBROpaque builds its pipeline from.
        let parameters_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("material_parameters_bind_group_layout"),
                entries: &material_params_layout_entries(),
            });
        let parameters_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{}_material_parameters_bind_group", name)),
            layout: &parameters_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: parameters_uniform_buffer.as_entire_binding(),
            }],
        });

        let textures_bind_group =
            Self::create_textures_bind_group(device, resource_manager, name, textures)?;

        Ok(Self {
            name: name.to_string(),
            translucent: parameters.opacity < 1.0,
            parameters_bind_group,
            textures_bind_group,
            parameters_uniform_buffer,
        })
    }

    /// Uploads new parameter values; bind groups stay valid.
    pub(crate) fn write_parameters(&mut self, queue: &wgpu::Queue, parameters: &MaterialParams) {
        self.translucent = parameters.opacity < 1.0;
        queue.write_buffer(
            &self.parameters_uniform_buffer,
            0,
            bytemuck::bytes_of(parameters),
        );
    }

    /// Builds the texture bind group, substituting the engine default texture for empty slots.
    pub(crate) fn create_textures_bind_group(
        device: &wgpu::Device,
        resource_manager: &ResourceManager,
        name: &str,
        textures: &MaterialTextures,
    ) -> Result<wgpu::BindGroup> {
        let slots = [
            (textures.base_color, DEFAULT_COLOR_TEXTURE_NAME),
            (textures.normal, DEFAULT_NORMAL_TEXTURE_NAME),
            (
                textures.metallic_roughness,
                DEFAULT_METALLIC_ROUGHNESS_TEXTURE_NAME,
            ),
            (textures.emissive, DEFAULT_EMISSIVE_TEXTURE_NAME),
        ];

        // Resolve handles first so their borrows outlive the texture references below
        let mut handles = Vec::with_capacity(slots.len());
        for (handle, default_name) in slots {
            handles.push(match handle {
                Some(handle) => handle,
                None => resource_manager
                    .get_resource_handle::<RendererTexture>(default_name)
                    .map_err(|_| PillError::from(RendererError::RendererResourceNotFound))?,
            });
        }

        let mut renderer_textures = Vec::with_capacity(handles.len());
        for handle in &handles {
            renderer_textures.push(
                resource_manager
                    .get_resource::<RendererTexture>(handle)
                    .map_err(|_| PillError::from(RendererError::RendererResourceNotFound))?,
            );
        }

        let mut entries = Vec::with_capacity(renderer_textures.len() * 2);
        for (slot, texture) in renderer_textures.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: slot as u32 * 2,
                resource: wgpu::BindingResource::TextureView(&texture.texture_view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: slot as u32 * 2 + 1,
                resource: wgpu::BindingResource::Sampler(&texture.sampler),
            });
        }

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material_textures_bind_group_layout"),
            entries: &material_textures_layout_entries(),
        });
        Ok(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{}_textures", name)),
            layout: &layout,
            entries: &entries,
        }))
    }
}

impl PillTypeMapKey for RendererMaterial {
    type Storage = ResourceStorage<RendererMaterial>;
}

impl Resource for RendererMaterial {
    type Handle = RendererMaterialHandle;

    fn get_name(&self) -> String {
        self.name.clone()
    }
}
