//! What a shader asset becomes on the GPU: its render pipeline and the
//! layouts of the groups its draws bind.
//!
//! # Responsibilities
//!
//! - Build the render pipeline and the group layouts from a shader's WGSL,
//!   and report a shader wgpu refuses against the asset that named it instead
//!   of taking the host down ([`RendererShader::new`]).
//! - Create the parameters group layout (set 2) and the textures group layout
//!   (set 3). Each one is absent when the shader declares no slots of that
//!   kind.
//! - Carry the flags that say whether the shader reads the engine's parameters
//!   or the camera's, so a drawer binds those groups only when the shader
//!   reads them.
//!
//! # Design
//!
//! A shader is built during asset sync, when its asset is loaded or its
//! content version changes, and once at startup for the built-in default
//! shader. It is not rebuilt per frame.
//!
//! The four group slots are fixed by the engine's convention: engine
//! parameters at 0, camera parameters at 1, a material's or pass's parameters
//! at 2, and its textures at 3. A slot the shader does not declare still gets
//! an empty layout, so every pipeline has the same shape the drawers bind
//! against.
//!
//! The WGSL is not written by hand. The data crate's build script compiles its
//! HLSL sources to it, and in development the host's shader watcher compiles
//! an edited source again and sends the new text to the shader asset.

// External crates
use indexmap::IndexMap;
use pill_core::debug;
use pill_core::telemetry::telemetry_target;

// Current crate
use crate::{
    assets::{ShaderParameterSlot, ShaderTextureKind, ShaderTextureSlot},
    error::{RendererError, Result},
};

// =============================================================================
// Handle
// =============================================================================

pill_core::define_slot_key!(RendererShaderHandle);

// =============================================================================
// RendererShader
// =============================================================================

/// One shader's GPU pipeline and the group layouts its draws bind.
///
/// The pipeline is what a draw switches to. A material or pass built against
/// this shader binds its parameters and textures through these layouts.
pub struct RendererShader {
    /// The shader asset's name, used in wgpu object labels, logs and error
    /// messages.
    pub name: String,
    /// The pipeline a draw through this shader binds.
    ///
    /// A pass that owns a pipeline draws through its own instead, because only
    /// the pass knows the target's format, its depth settings and its culling.
    pub render_pipeline: wgpu::RenderPipeline,

    /// The uniforms the shader declares, by name, in declaration order.
    pub parameter_slots: IndexMap<String, ShaderParameterSlot>,
    /// Layout of the parameters group (set 2), or `None` when the shader
    /// declares no parameters.
    pub parameters_bind_group_layout: Option<wgpu::BindGroupLayout>,

    /// The textures the shader declares, by name, each with its bindings.
    pub texture_slots: IndexMap<String, ShaderTextureSlot>,
    /// Layout of the textures group (set 3), or `None` when the shader
    /// declares no textures.
    pub textures_bind_group_layout: Option<wgpu::BindGroupLayout>,

    /// Whether the shader reads the engine's parameters, in which case a
    /// drawer binds that group before the draw.
    pub reads_engine_parameters: bool,
    /// Whether the shader reads the camera's parameters, in which case a
    /// drawer binds that group before the draw.
    pub reads_camera_parameters: bool,
}

impl RendererShader {
    /// Builds a shader's render pipeline and group layouts from its WGSL, for
    /// the given color and depth formats.
    ///
    /// The parameters and textures layouts exist only when the shader declares
    /// slots of that kind. A missing one is padded with an empty layout, so the
    /// shader keeps the engine's fixed group convention.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Other`] when wgpu reports an error while the
    /// pipeline is created: a validation error, which means the shader asset is
    /// wrong, or an out-of-memory or internal error. The error is captured
    /// while the pipeline is built, so the refusal names the shader instead of
    /// panicking.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
        vertex_layouts: &[wgpu::VertexBufferLayout],
        vertex_wgsl: &str,
        fragment_wgsl: &str,
        parameter_slots: &IndexMap<String, ShaderParameterSlot>,
        texture_slots: &IndexMap<String, ShaderTextureSlot>,
        engine_bind_group_layout: &wgpu::BindGroupLayout,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        uniform_groups: UniformGroups,
    ) -> Result<Self> {
        // Step 1: Create the two shader modules. The WGSL is not written by
        // hand: the data crate's build script compiles its HLSL sources to it,
        // and a shader edited in development is compiled again by the host's
        // shader watcher. wgpu parses the WGSL when the module is created.
        let vertex_shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("master_vertex_shader"),
            source: wgpu::ShaderSource::Wgsl(vertex_wgsl.into()),
        });
        let fragment_shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("master_fragment_shader"),
            source: wgpu::ShaderSource::Wgsl(fragment_wgsl.into()),
        });

        // Step 2: Build the group layouts. A layout is built only when the
        // shader declares slots of that kind.
        let parameters_bind_group_layout = {
            if !parameter_slots.is_empty() {
                let bind_group_layout_entry = wgpu::BindGroupLayoutEntry {
                    binding: 0, // (set = 2, binding = 0)
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                };

                Some(
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some(&format!("{}_parameters_bind_group_layout", name)),
                        entries: &[bind_group_layout_entry],
                    }),
                )
            } else {
                None
            }
        };

        // Textures bind at group 3, one texture and one sampler per slot.
        let textures_bind_group_layout = {
            if !texture_slots.is_empty() {
                let mut bind_group_entries = Vec::new();

                for texture_slot in texture_slots.values() {
                    // A depth slot binds a depth view, sampled without filtering.
                    // wgpu refuses a pipeline that treats a depth buffer as a
                    // filterable colour texture.
                    let (sample_type, sampler_type) = match texture_slot.kind {
                        ShaderTextureKind::Depth => (
                            wgpu::TextureSampleType::Depth,
                            wgpu::SamplerBindingType::NonFiltering,
                        ),
                        ShaderTextureKind::Color
                        | ShaderTextureKind::Normal
                        | ShaderTextureKind::Equirect
                        | ShaderTextureKind::Cubemap => (
                            wgpu::TextureSampleType::Float { filterable: true },
                            wgpu::SamplerBindingType::Filtering,
                        ),
                    };

                    // The texture half of the slot. A cubemap slot takes a cube
                    // view, which is what an HLSL `TextureCube` declares.
                    bind_group_entries.push(wgpu::BindGroupLayoutEntry {
                        binding: texture_slot.texture_binding,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: slot_view_dimension(texture_slot.kind),
                            sample_type,
                        },
                        count: None,
                    });

                    bind_group_entries.push(wgpu::BindGroupLayoutEntry {
                        binding: texture_slot.sampler_binding,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Sampler(sampler_type),
                        count: None,
                    });
                }

                Some(
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some(&format!("{}_textures_bind_group_layout", name)),
                        entries: &bind_group_entries,
                    }),
                )
            } else {
                None
            }
        };

        // Step 3: Create the render pipeline. The four group slots are fixed by
        // the engine's convention, and a shader that declares neither
        // parameters nor textures gets an empty layout in that slot, so every
        // pipeline has the same shape to bind against.
        let pipeline_layout = {
            let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shader_empty_bind_group_layout"),
                entries: &[],
            });

            let bind_group_layouts = [
                engine_bind_group_layout,
                camera_bind_group_layout,
                parameters_bind_group_layout
                    .as_ref()
                    .unwrap_or(&empty_layout),
                textures_bind_group_layout.as_ref().unwrap_or(&empty_layout),
            ];

            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(&format!("{}_pipeline_layout", name)),
                bind_group_layouts: &bind_group_layouts
                    .iter()
                    .map(|layout| Some(*layout))
                    .collect::<Vec<_>>(),
                immediate_size: 0,
            })
        };

        let color_target_states = &[Some(wgpu::ColorTargetState {
            format: color_format,
            blend: Some(wgpu::BlendState {
                alpha: wgpu::BlendComponent::REPLACE,
                color: wgpu::BlendComponent::REPLACE,
            }),
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let render_pipeline_descriptor = wgpu::RenderPipelineDescriptor {
            label: Some(&format!("{}_render_pipeline", name)),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex_shader_module,
                entry_point: Some("vs_main"),
                // The buffer layouts of the vertex data this shader reads.
                buffers: &vertex_layouts
                    .iter()
                    .map(|layout| Some((*layout).clone()))
                    .collect::<Vec<_>>(),
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader_module,
                entry_point: Some("fs_main"),
                targets: color_target_states,
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                // Every three vertices form one triangle.
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                // A triangle faces the camera when its vertices run counter-clockwise.
                front_face: wgpu::FrontFace::Ccw,
                // Drop the triangles that face away.
                cull_mode: Some(wgpu::Face::Back),
                // Any other polygon mode needs the NON_FILL_POLYGON_MODE feature.
                polygon_mode: wgpu::PolygonMode::Fill,
                // `true` needs the CONSERVATIVE_RASTERIZATION feature.
                conservative: false,
                unclipped_depth: false,
            },
            depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: Some(true),
                // Keep a new pixel only when it is closer than the one already
                // drawn there.
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        };

        // Capture wgpu's error so a refused shader is reported against the
        // asset that named it, and not only through the device's
        // uncaptured-error handler.
        let render_pipeline = crate::error::capturing_validation(device, || {
            device.create_render_pipeline(&render_pipeline_descriptor)
        })
        .map_err(|detail| RendererError::Other { detail })?;

        // Step 4: Record the shader, then log what it declares.
        let renderer_shader = Self {
            name: name.to_string(),
            render_pipeline,
            parameter_slots: parameter_slots.clone(),
            textures_bind_group_layout,
            texture_slots: texture_slots.clone(),
            parameters_bind_group_layout,
            reads_engine_parameters: uniform_groups.engine,
            reads_camera_parameters: uniform_groups.camera,
        };

        // One line per shader, hidden unless `engine::rendering` is at debug:
        // the slots it declares and the two uniform-group flags.
        let joined_slot_names = |names: Vec<&String>| {
            names
                .into_iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        };
        debug!(
            target: telemetry_target::RENDERING,
            shader = name,
            parameters = %joined_slot_names(parameter_slots.keys().collect()),
            textures = %joined_slot_names(texture_slots.keys().collect()),
            engine_parameters = uniform_groups.engine,
            camera_parameters = uniform_groups.camera,
            "shader created"
        );

        Ok(renderer_shader)
    }
}

// =============================================================================
// UniformGroups
// =============================================================================

/// Says which of the renderer's shared uniform groups a shader reads.
///
/// The two flags travel together because `RendererShader::new` already takes
/// twelve arguments, and two bare booleans at the end of the call would not
/// say which group each one turns on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UniformGroups {
    /// The shader reads group 0, the engine's parameters.
    pub engine: bool,
    /// The shader reads group 1, the camera's parameters.
    pub camera: bool,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the view shape a texture slot of `kind` binds: a cube for a cubemap,
/// a 2D image for every other kind, depth included.
pub(crate) fn slot_view_dimension(kind: ShaderTextureKind) -> wgpu::TextureViewDimension {
    match kind {
        ShaderTextureKind::Cubemap => wgpu::TextureViewDimension::Cube,
        _ => wgpu::TextureViewDimension::D2,
    }
}

/// Checks that `texture` fits a slot of `slot_kind`, and names both when it
/// does not.
///
/// A cubemap bound where a shader reads a 2D image (or the other way round)
/// would fail wgpu's bind group validation with a message about view
/// dimensions. This check runs while the names are still known, so the error
/// can say which slot and which texture.
///
/// # Errors
///
/// Returns [`RendererError::Other`] when the texture's view shape differs from
/// the shape the slot declares.
pub(crate) fn check_slot_shape(
    owner: &str,
    slot_name: &str,
    slot_kind: ShaderTextureKind,
    texture: &crate::resources::RendererTexture,
) -> Result<()> {
    let expected_view_dimension = slot_view_dimension(slot_kind);
    if texture.view_dimension == expected_view_dimension {
        return Ok(());
    }
    Err(RendererError::Other {
        detail: format!(
            "{owner} binds a {:?} texture to slot `{slot_name}`, which its shader declares as {slot_kind:?}",
            texture.view_dimension
        ),
    })
}
