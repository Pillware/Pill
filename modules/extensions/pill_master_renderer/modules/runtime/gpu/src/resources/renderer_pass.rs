//! The GPU side of one pass of the chain.
//!
//! # Responsibilities
//!
//! - Hold what recording one pass needs: its pipeline, the bind groups for the
//!   values the pass carries itself, and the two flags that say which shared
//!   groups its draws bind.
//! - Build that pipeline for whichever shape the pass takes, geometry included.
//! - Hand its values and textures to the shared bind-group builder, so a pass
//!   and a material reach a shader's slots the same way.
//!
//! # Design
//!
//! Built when the chain changes, not every frame: a pass changes when the game
//! swaps its pipeline, not when the scene moves. One type covers the three
//! shapes a pass takes, told apart by [`PassKind`]:
//!
//! - a geometry pass that names its own shader - the mesh and instance vertex
//!   layouts, the pass's cull mode, and a depth attachment it tests, writing it
//!   unless the pass asked not to;
//! - a fullscreen pass - three vertices, no vertex buffers, no depth test, no
//!   instance data, reading what an earlier pass wrote. Every post-processing
//!   step the default pipeline declares has this shape, and it is the shape
//!   that carries its own parameters and reads earlier passes' pictures;
//! - a skybox, which is the same triangle, tested against the depth the
//!   geometry passes wrote (less-or-equal, without writing) and put at the far
//!   plane by its shader, so it draws only where no mesh did.
//!
//! A geometry pass reaches the frame through the mesh path like any other
//! drawable: the chain resolves it into a [`ResolvedPass`] and the renderer
//! records it with this pipeline. A geometry pass that names no shader has no
//! type here at all: it draws through each material's pipeline.

// Standard library
use std::collections::{HashMap, HashSet};

// External crates
use pill_core::telemetry::telemetry_target;
use pill_core::warn;

// Current crate
use crate::{
    assets::Shader,
    error::{capturing_validation, ErrorContext, RendererError, Result},
    frame::{CullMode, PassKind, ResolvedPass},
    resources::{
        check_slot_shape, shader_bindings::shader_bind_groups, RendererMesh,
        RendererResourceStorage, RendererShader, RendererTexture, RendererTextureHandle, Vertex,
    },
    Instance,
};

// =============================================================================
// Constants
// =============================================================================

/// The name a pass uses in its inputs to read the renderer's own depth buffer
/// instead of an offscreen colour target.
pub const DEPTH_INPUT: &str = "depth";

// =============================================================================
// RendererPass
// =============================================================================

/// One pass of the chain, in the shape its [`PassKind`] asks for: a geometry
/// pass drawing instances through its own shader, a fullscreen pass, or the
/// skybox.
pub struct RendererPass {
    /// Name of the pass this pipeline was built for.
    pub name: String,
    /// Pipeline a draw of this pass binds.
    pub pipeline: wgpu::RenderPipeline,
    /// Whether the pass's shader reads the engine's parameters.
    pub reads_engine_parameters: bool,
    /// Whether the pass's shader reads the camera's parameters.
    pub reads_camera_parameters: bool,
    /// The pass's own parameter values, for a shader that declares parameter
    /// slots. `None` when the shader declares none.
    pub parameters_bind_group: Option<wgpu::BindGroup>,
    /// The textures the pass samples, for a shader that declares texture
    /// slots. `None` when the shader declares none.
    pub textures_bind_group: Option<wgpu::BindGroup>,
}

impl RendererPass {
    /// Builds the pipeline, the bind groups and the flags a pass draws with.
    ///
    /// `shader` is the cooked source the pipeline's modules are created from,
    /// and `renderer_shader` is the GPU shader built from it, whose group
    /// layouts the bind groups are created against. `target_formats` holds one
    /// format per output the pass writes, in the order the shader declares
    /// them. `offscreen_targets` holds the texture created for each offscreen
    /// target name in the chain, `depth_texture` is the renderer's own depth
    /// buffer, and `defined_targets` names the targets the passes before this
    /// one declared, which are the only ones this pass may read.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Other`] when wgpu refuses the pipeline, when a
    /// texture slot cannot be resolved (a pass input no earlier pass writes is
    /// one case), or when a shader declares slots and has no layout for them.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rendering_resource_storage: &RendererResourceStorage,
        engine_bind_group_layout: &wgpu::BindGroupLayout,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        pass: &ResolvedPass,
        shader: &Shader,
        renderer_shader: &RendererShader,
        target_formats: &[wgpu::TextureFormat],
        depth_format: wgpu::TextureFormat,
        offscreen_targets: &HashMap<String, RendererTexture>,
        depth_texture: &RendererTexture,
        textures: &[(String, RendererTextureHandle)],
        defined_targets: &HashSet<String>,
    ) -> Result<Self> {
        // Step 1: Create the shader modules and the pipeline layout.
        let vertex_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pass_vertex_shader"),
            source: wgpu::ShaderSource::Wgsl(shader.vertex_wgsl.as_str().into()),
        });
        let fragment_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pass_fragment_shader"),
            source: wgpu::ShaderSource::Wgsl(shader.fragment_wgsl.as_str().into()),
        });

        // A pass may declare no parameters, or no textures, and still has to
        // fill its slot in the pipeline layout: every shader in the engine puts
        // its parameters at group 2 and its texture inputs at group 3. A
        // missing group is padded with an empty layout here, so a shader never
        // has to declare slots it does not use.
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pass_empty_bind_group_layout"),
            entries: &[],
        });
        let parameters_layout = renderer_shader
            .parameters_bind_group_layout
            .as_ref()
            .unwrap_or(&empty_layout);
        let textures_layout = renderer_shader
            .textures_bind_group_layout
            .as_ref()
            .unwrap_or(&empty_layout);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(&format!("{}_pipeline_layout", pass.name)),
            bind_group_layouts: &[
                Some(engine_bind_group_layout),
                Some(camera_bind_group_layout),
                Some(parameters_layout),
                Some(textures_layout),
            ],
            immediate_size: 0,
        });

        // Step 2: Set up the state that follows the pass's kind - its vertex
        // layouts, its culling and its depth attachment.
        //
        // The pipeline is built for the target the pass writes, not for the
        // surface: a lit frame is drawn into a half-float target and the pass
        // that tonemaps it writes the swapchain, so one pipeline cannot serve
        // both. The vertex layout follows the kind for the same reason:
        // instances for geometry, nothing at all for a fullscreen triangle,
        // which takes its corners from `SV_VertexID`.
        let vertex_layouts: Vec<wgpu::VertexBufferLayout> = match pass.settings.kind {
            PassKind::Geometry => vec![
                RendererMesh::data_layout_descriptor(),
                Instance::data_layout_descriptor(),
            ],
            PassKind::Fullscreen | PassKind::Skybox => Vec::new(),
        };
        let (cull_mode, depth_stencil) = match pass.settings.kind {
            PassKind::Geometry => (
                match pass.settings.cull {
                    CullMode::Back => Some(wgpu::Face::Back),
                    CullMode::Front => Some(wgpu::Face::Front),
                    CullMode::None => None,
                },
                Some(wgpu::DepthStencilState {
                    format: depth_format,
                    // A translucent pass reads depth without writing it, so the
                    // draws that come after it are not hidden by it.
                    depth_write_enabled: Some(pass.settings.depth_write),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
            ),
            // No depth test: a fullscreen pass overwrites every pixel it
            // covers, and the depth an earlier pass left describes other
            // geometry, so testing against it would reject the wrong pixels.
            PassKind::Fullscreen => (None, None),
            // Tested but never written: the triangle sits at the far plane
            // (depth 1.0), so less-or-equal lets it through exactly where the
            // depth buffer still holds its cleared 1.0 - the pixels no mesh
            // drew.
            PassKind::Skybox => (
                None,
                Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
            ),
        };

        // Step 3: Create one colour target per output the pass writes, then
        // build its pipeline.
        //
        // Each target takes the format the chain resolved for it, blended if
        // the pass asked for blending.
        let color_targets: Vec<Option<wgpu::ColorTargetState>> = target_formats
            .iter()
            .map(|format| {
                Some(wgpu::ColorTargetState {
                    format: *format,
                    blend: Some(if pass.settings.blend {
                        wgpu::BlendState::ALPHA_BLENDING
                    } else {
                        wgpu::BlendState::REPLACE
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
            .collect();

        // The pipeline build is captured instead of left to wgpu's
        // uncaptured-error handler. When the driver refuses a pipeline - a
        // shader whose entry points do not match the pass's kind, for example -
        // the error becomes this pass's reason for not being drawn.
        let pipeline = capturing_validation(device, || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&format!("{}_pipeline", pass.name)),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &vertex_module,
                    entry_point: Some("vs_main"),
                    buffers: &vertex_layouts.into_iter().map(Some).collect::<Vec<_>>(),
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fragment_module,
                    entry_point: Some("fs_main"),
                    targets: &color_targets,
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                    unclipped_depth: false,
                },
                depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        })
        .map_err(|detail| RendererError::Other { detail })?;

        // Step 4: Build the pass's own bind groups.
        //
        // They come from the same function a material's groups do. All a pass
        // adds is where an input slot reads from.
        //
        // An input is a picture an earlier pass wrote, so a name no pass
        // produces means the chain cannot draw. Failing here names the pass and
        // the missing target, instead of binding the default texture and
        // quietly showing a flat frame.
        let (parameters_bind_group, textures_bind_group) = shader_bind_groups(
            device,
            queue,
            rendering_resource_storage,
            &format!("pass {}", pass.name),
            renderer_shader,
            &pass.parameters,
            textures,
            |slot_name, slot| {
                let Some(target) = pass.inputs.get(slot_name) else {
                    return Ok(None);
                };
                // Depth is not an offscreen colour target, but a pass names it
                // the same way: as something an earlier pass left behind. It is
                // read through the depth buffer's own sampler, which is a plain
                // read sampler, not a comparison one.
                if target == DEPTH_INPUT {
                    return Ok(Some((&depth_texture.texture_view, &depth_texture.sampler)));
                }
                // The target map holds every offscreen target the chain
                // declares, so it cannot tell an earlier pass's output from
                // this pass's own or a later pass's. `defined_targets` holds
                // only what the passes before this one wrote, and a target
                // outside it has not been drawn into yet.
                if !defined_targets.contains(target.as_str()) {
                    return Err(RendererError::Other {
                        detail: format!(
                            "pass {} reads `{target}`, which no earlier pass writes",
                            pass.name
                        ),
                    });
                }
                let texture = offscreen_targets.get(target).context(format!(
                    "pass {} reads `{target}`, which no earlier pass writes",
                    pass.name
                ))?;
                check_slot_shape(
                    &format!("pass {}", pass.name),
                    slot_name,
                    slot.kind,
                    texture,
                )?;
                Ok(Some((&texture.texture_view, &texture.sampler)))
            },
        )?;

        // A name the shader has no slot for is a typo, or a name left behind in
        // the pass document after its shader changed. Saying so is the
        // difference between a pass that is missing a texture and one whose
        // author believes it bound it.
        for slot_name in pass.inputs.keys() {
            if !renderer_shader.texture_slots.contains_key(slot_name) {
                warn!(
                    target: telemetry_target::RENDERING,
                    pass = pass.name.as_str(),
                    slot = %slot_name,
                    "pass names an input its shader has no slot for"
                );
            }
        }
        for (slot_name, _key) in textures {
            if !renderer_shader.texture_slots.contains_key(slot_name) {
                warn!(
                    target: telemetry_target::RENDERING,
                    pass = pass.name.as_str(),
                    slot = %slot_name,
                    "pass binds a texture its shader has no slot for"
                );
            }
        }

        Ok(Self {
            name: pass.name.clone(),
            pipeline,
            reads_engine_parameters: renderer_shader.reads_engine_parameters,
            reads_camera_parameters: renderer_shader.reads_camera_parameters,
            parameters_bind_group,
            textures_bind_group,
        })
    }
}
