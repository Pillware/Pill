//! Everything the renderer draws over what the chain rendered: the debug lines
//! on every surface, the editor's chrome (selection outlines, gizmo handles and
//! the scene grid) on the surfaces the frame marks as editor views, the pick
//! buffer for the surfaces that ask for a pick, and the debug labels, which it
//! lays out for the UI painter to draw.
//!
//! # Responsibilities
//!
//! - Build the overlay's pipelines once, beside the chain's ([`Overlay::new`]).
//! - Record a surface's overlay after its chain: the outline mask and edge,
//!   the scene grid, the depth-tested and on-top debug lines and the gizmos
//!   ([`Overlay::record`]).
//! - Lay the debug labels out as a `pill_ui_frame` list over the bitmap font
//!   ([`label_list`], [`font::texture_set`]); the UI painter draws it, since
//!   a label is a screen-space textured quad like any UI.
//! - Draw the pick buffer for a surface that has picks waiting, and read the
//!   asked-for pixels back frames later ([`Overlay::take_results`]).
//!
//! # Design
//!
//! None of this goes through the chain: a project's chain decides how its
//! scene looks, and an outline or a debug line has to look the same whatever
//! the chain is. The overlay draws after the chain into the same surface view,
//! reading the depth buffer the chain's geometry passes left, so geometry in
//! front of a depth-tested line hides it. The mask and pick passes
//! draw the frame's instances straight from the instance buffer the chain
//! already uploaded, so a queue position is an instance index in both.
//!
//! Lines, gizmo triangles and the outline are anti-aliased without
//! multisampling: each primitive is projected on the processor, widened in
//! screen space by a pixel of feather, and its fragment shader fades that
//! band back down to the edge the shape really has. Multisampling would cost
//! every surface of every frontend; this costs only the overlay's own
//! primitives.

// Standard library
use std::collections::HashSet;

// External crates
use pill_core::math::Vector2f;
use pill_engine::Entity;
use pill_renderer_api::editor::{PickHit, PickRequest, PickResult, PICK_HANDLE_BIT};
use pill_renderer_api::{DebugDepth, DebugFrame, GizmoPointShape, LabelAlign};
use pill_rendering::color_space::linear_to_srgb;
use pill_rendering_core::projection::{
    clip_polygon_to_near, expand_polygon, label_origin, line_quad, point_quad,
};
use pill_ui_frame::{Color, DrawList, Rect};

// Current crate
use crate::{
    assets::MeshVertex,
    components::RenderViewport,
    error::{capturing_validation, RendererError, Result},
    frame::RenderFrame,
    render_queue::RenderQueueItem,
    resources::{
        RendererCamera, RendererMeshHandle, RendererResourceStorage, RendererTexture, Vertex,
    },
    stream_buffer::StreamBuffer,
    Instance,
};

/// The bitmap font the debug labels are drawn in, and its painter texture.
pub(crate) mod font;
mod picking;

// =============================================================================
// Constants
// =============================================================================

/// Width of a selection outline, in pixels.
const OUTLINE_WIDTH: f32 = 2.0;
/// Format of the outline mask.
const OUTLINE_MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// Format of the pick buffer: one id per pixel.
const PICK_BUFFER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
/// Format of the pick buffer's own depth: the same one every depth-tested
/// pipeline in the module declares.
const PICK_BUFFER_DEPTH_FORMAT: wgpu::TextureFormat = RendererTexture::DEPTH_FORMAT;
/// The colour of the shadow under a label, so it reads over any background.
const LABEL_SHADOW_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.85];
/// The half width of a drawn overlay line, in pixels.
const LINE_HALF_WIDTH: f32 = 1.0;
/// How many pixels past the line's half width the anti-aliasing fade reaches.
const LINE_FEATHER: f32 = 0.5;
/// The half width of a gizmo line's quad in the pick buffer, in pixels.
///
/// Wider than the drawn line, so a click counts anywhere inside the band the
/// editor's hover test treats as the handle. That radius follows the handle's
/// length, which keeps it a roughly constant size on screen, so a handle
/// responds where it is highlighted.
const PICK_LINE_HALF_WIDTH: f32 = 7.0;
/// Which side of its line each of a quad's six corners sits on, in order.
const LINE_QUAD_SIDES: [f32; 6] = [1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
/// How far a gizmo triangle's corners are pushed out, in pixels, so its
/// anti-aliasing has a band to fade in.
const TRIANGLE_FEATHER: f32 = 1.0;
/// How far past a gizmo point's drawn size its pick quad reaches, in pixels.
const PICK_POINT_SLACK: f32 = 1.0;

// =============================================================================
// Vertex and Uniform Types
// =============================================================================

/// A line vertex after projection: the corner of the line's quad, its colour,
/// and how far in pixels it sits from the line's centre (negative on the other
/// side). The fragment shader turns that distance into coverage.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct LineVertex {
    clip_position: [f32; 4],
    color: [f32; 4],
    across_pixels: f32,
    /// Whether the fragment shader drops every other pixel of this line, which
    /// is how a dimmed drawing reads as being behind the geometry.
    hidden: f32,
}

/// A gizmo-triangle vertex after projection: the corner pushed out by the
/// feather, its colour, and the barycentric coordinate of the corner it stands
/// for. The fragment shader turns that coordinate into edge coverage.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct TriangleVertex {
    clip_position: [f32; 4],
    color: [f32; 4],
    barycentric: [f32; 3],
}

/// A gizmo-point vertex after projection: the corner of the screen-aligned
/// quad, its colour, the corner's offset from the point's centre in pixels,
/// the point's half size in pixels, and the silhouette it draws as. The
/// fragment shader tests each pixel sample against the shape.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PointVertex {
    clip_position: [f32; 4],
    color: [f32; 4],
    offset_pixels: [f32; 2],
    half_size_pixels: f32,
    shape: u32,
}

/// A gizmo line vertex in the pick buffer: the corner of the line's quad and
/// its handle value.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct HandleLineVertex {
    clip_position: [f32; 4],
    handle_value: u32,
}

/// A gizmo triangle vertex in the pick buffer: world position and handle
/// value.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct HandleVertex {
    world_position: [f32; 3],
    handle_value: u32,
}

/// The outline uniform: colour, and the width in `outline_settings[0]`.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct OutlineParameters {
    color: [f32; 4],
    outline_settings: [f32; 4],
}

/// The grid uniform: the cell size in `cell_size[0]`, and the drawing region in
/// `viewport`, in pixels. The fragment shader turns each pixel back into a
/// clip-space ray through that region.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct GridParameters {
    cell_size: [f32; 4],
    viewport: [f32; 4],
}

/// A vertex on its way to the screen, as `pill_rendering_core` names it: its
/// clip-space position and the corner of the shape it belongs to, carried
/// through clipping.
type ClipVertex = pill_rendering_core::projection::ClipVertex;

// =============================================================================
// Overlay Targets
// =============================================================================

/// The targets one surface's overlay draws into.
///
/// Owned by the surface they are sized to, beside its depth buffer and
/// offscreen targets, and rebuilt with them on a resize. An outline mask is a
/// surface-sized GPU object like any other; a cache keyed by size would make
/// two surfaces share one and evict each other's.
///
/// The pick buffer and its depth are the exception: a surface that never picks
/// never allocates them.
pub(crate) struct OverlayTargets {
    width: u32,
    height: u32,
    /// The silhouettes of the outlined meshes, drawn here and read by the edge
    /// pass.
    mask_view: wgpu::TextureView,
    outline_bind_group: wgpu::BindGroup,
    /// The pick buffer and its depth, made the first time a pick asks.
    pick_targets: Option<(wgpu::Texture, wgpu::TextureView, wgpu::TextureView)>,
}

// =============================================================================
// Overlay Frame
// =============================================================================

/// One surface's frame, as the overlay needs it.
pub(crate) struct OverlayFrame<'a> {
    /// The surface being drawn.
    pub surface: u32,
    /// The view the chain drew into.
    pub color_view: &'a wgpu::TextureView,
    /// The depth the chain's geometry left.
    pub depth_view: &'a wgpu::TextureView,
    /// The region the scene was drawn into.
    pub viewport: RenderViewport,
    /// The camera the surface was drawn through.
    pub camera: &'a RendererCamera,
    /// The frame.
    pub frame: &'a RenderFrame,
    /// The frame's queue: queue position is instance index.
    pub render_queue: &'a [RenderQueueItem],
    /// The instance buffer the chain drew from.
    pub instance_buffer: &'a wgpu::Buffer,
    /// The meshes the queue names.
    pub storage: &'a RendererResourceStorage,
}

// =============================================================================
// Overlay
// =============================================================================

/// The overlay's pipelines and the buffers they read.
///
/// One overlay serves every surface; what it draws into is made per surface
/// ([`OverlayTargets`]), because the mask and the pick buffer are sized to the
/// surface they belong to.
pub(crate) struct Overlay {
    depth_tested_lines_pipeline: wgpu::RenderPipeline,
    dimmed_lines_pipeline: wgpu::RenderPipeline,
    on_top_lines_pipeline: wgpu::RenderPipeline,
    on_top_triangles_pipeline: wgpu::RenderPipeline,
    on_top_points_pipeline: wgpu::RenderPipeline,
    outline_edge_pipeline: wgpu::RenderPipeline,
    outline_mask_pipeline: wgpu::RenderPipeline,
    mesh_pick_pipeline: wgpu::RenderPipeline,
    handle_lines_pick_pipeline: wgpu::RenderPipeline,
    handle_triangles_pick_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,
    grid_parameters_buffer: wgpu::Buffer,
    grid_bind_group: wgpu::BindGroup,
    outline_bind_group_layout: wgpu::BindGroupLayout,
    outline_parameters_buffer: wgpu::Buffer,
    tested_lines_buffer: StreamBuffer,
    dimmed_lines_buffer: StreamBuffer,
    on_top_lines_buffer: StreamBuffer,
    triangles_buffer: StreamBuffer,
    points_buffer: StreamBuffer,
    handle_lines_buffer: StreamBuffer,
    handle_triangles_buffer: StreamBuffer,
    readback: picking::PickReadback,
}

/// A vertex attribute list for consecutive fields.
macro_rules! attributes {
    ($($location:expr => $format:ident),* $(,)?) => {
        &wgpu::vertex_attr_array![$($location => $format),*]
    };
}

impl Overlay {
    /// Creates every overlay pipeline, for a surface drawn in `color_format`
    /// whose geometry writes `depth_format`. Every pipeline that reads the
    /// camera binds `camera_bind_group_layout` at group 0.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Other`] when wgpu refuses a shader or pipeline.
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Result<Self> {
        capturing_validation(device, || {
            Self::create(device, color_format, depth_format, camera_bind_group_layout)
        })
        .map_err(|detail| RendererError::Other {
            detail: format!("overlay pipelines: {detail}"),
        })
    }

    fn create(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        // Step 1: Create the three shader modules.
        let world_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay_world"),
            source: wgpu::ShaderSource::Wgsl(include_str!("world.wgsl").into()),
        });
        let outline_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay_outline"),
            source: wgpu::ShaderSource::Wgsl(include_str!("outline.wgsl").into()),
        });
        let grid_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay_grid"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grid.wgsl").into()),
        });

        // Step 2: Create the bind group layouts and the uniform buffers and
        // groups behind them.
        // The outline edge pass reads the mask texel by texel.
        let outline_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("overlay_outline"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let outline_parameters_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay_outline"),
            size: std::mem::size_of::<OutlineParameters>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The grid's own uniform: the cell size and the drawing region, so
        // one fullscreen triangle can turn any pixel into a world ray.
        let grid_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("overlay_grid"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let grid_parameters_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay_grid"),
            size: std::mem::size_of::<GridParameters>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let grid_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay_grid"),
            layout: &grid_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: grid_parameters_buffer.as_entire_binding(),
            }],
        });

        // Step 3: Build the pipeline layouts, the vertex layouts and the render
        // states every pipeline shares.
        let pipeline_layout = |label: &str, groups: &[&wgpu::BindGroupLayout]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &groups.iter().map(|group| Some(*group)).collect::<Vec<_>>(),
                immediate_size: 0,
            })
        };
        let camera_pipeline_layout = pipeline_layout("overlay_camera", &[camera_bind_group_layout]);
        let outline_pipeline_layout =
            pipeline_layout("overlay_outline", &[&outline_bind_group_layout]);
        let grid_pipeline_layout = pipeline_layout(
            "overlay_grid",
            &[camera_bind_group_layout, &grid_bind_group_layout],
        );

        let line_vertex = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: attributes![0 => Float32x4, 1 => Float32x4, 2 => Float32, 3 => Float32],
        };
        let triangle_vertex = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TriangleVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: attributes![0 => Float32x4, 1 => Float32x4, 2 => Float32x3],
        };
        let point_vertex = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PointVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: attributes![
                0 => Float32x4, 1 => Float32x4, 2 => Float32x2, 3 => Float32, 4 => Uint32
            ],
        };
        let handle_vertex = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<HandleVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: attributes![0 => Float32x3, 1 => Uint32],
        };
        let handle_line_vertex = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<HandleLineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: attributes![0 => Float32x4, 1 => Uint32],
        };
        // A mesh position, from the mesh's own vertex buffer, and the
        // instance's model rows from the chain's instance buffer.
        let mesh_position = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: attributes![0 => Float32x3],
        };
        let mesh_buffers = [mesh_position, Instance::data_layout_descriptor()];

        let blended = Some(wgpu::ColorTargetState {
            format: color_format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        });
        let value_target = Some(wgpu::ColorTargetState {
            format: PICK_BUFFER_FORMAT,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        });
        let mask_target = Some(wgpu::ColorTargetState {
            format: OUTLINE_MASK_FORMAT,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        });
        let tested = depth_state(depth_format, wgpu::CompareFunction::LessEqual, false);
        let on_top = depth_state(depth_format, wgpu::CompareFunction::Always, false);
        let pick_on_top = depth_state(
            PICK_BUFFER_DEPTH_FORMAT,
            wgpu::CompareFunction::Always,
            false,
        );
        let pick_mesh = depth_state(PICK_BUFFER_DEPTH_FORMAT, wgpu::CompareFunction::Less, true);
        // A drawing that shows through geometry is drawn where the scene is
        // nearer, so it is the pixels the scene covers that it lands on; the
        // fragment shader mutes them and drops every other one.
        let dimmed = depth_state(depth_format, wgpu::CompareFunction::Greater, false);
        // The grid is pinned to the far plane, so its own compare keeps it
        // behind the chain's geometry and lets it show everywhere else.
        let grid_depth = depth_state(depth_format, wgpu::CompareFunction::LessEqual, false);
        let grid_target = blended.clone();

        // Step 4: Create one pipeline per draw the overlay records, then the
        // buffers it streams the vertices through.
        let pipeline = |description: PipelineDescription| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(description.label),
                layout: Some(description.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: description.shader_module,
                    entry_point: Some(description.vertex_entry_point),
                    buffers: &description
                        .vertex_buffers
                        .iter()
                        .map(|buffer| Some((*buffer).clone()))
                        .collect::<Vec<_>>(),
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: description.shader_module,
                    entry_point: Some(description.fragment_entry_point),
                    targets: std::slice::from_ref(&description.color_target),
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: description.topology,
                    ..Default::default()
                },
                depth_stencil: description.depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let world = |label,
                     vertex_entry_point,
                     fragment_entry_point,
                     vertex_buffers,
                     topology,
                     color_target,
                     depth_stencil| PipelineDescription {
            label,
            pipeline_layout: &camera_pipeline_layout,
            shader_module: &world_module,
            vertex_entry_point,
            fragment_entry_point,
            vertex_buffers,
            topology,
            color_target,
            depth_stencil,
        };
        let line_buffers = std::slice::from_ref(&line_vertex);
        let triangle_buffers = std::slice::from_ref(&triangle_vertex);
        let point_buffers = std::slice::from_ref(&point_vertex);
        let handle_buffers = std::slice::from_ref(&handle_vertex);
        let handle_line_buffers = std::slice::from_ref(&handle_line_vertex);
        Self {
            depth_tested_lines_pipeline: pipeline(world(
                "overlay_lines_tested",
                "line_vertex",
                "line_fragment",
                line_buffers,
                wgpu::PrimitiveTopology::TriangleList,
                blended.clone(),
                Some(tested),
            )),
            on_top_lines_pipeline: pipeline(world(
                "overlay_lines_on_top",
                "line_vertex",
                "line_fragment",
                line_buffers,
                wgpu::PrimitiveTopology::TriangleList,
                blended.clone(),
                Some(on_top.clone()),
            )),
            dimmed_lines_pipeline: pipeline(world(
                "overlay_lines_dimmed",
                "line_vertex",
                "dimmed_line_fragment",
                line_buffers,
                wgpu::PrimitiveTopology::TriangleList,
                blended.clone(),
                Some(dimmed),
            )),
            on_top_triangles_pipeline: pipeline(world(
                "overlay_triangles_on_top",
                "triangle_vertex",
                "triangle_fragment",
                triangle_buffers,
                wgpu::PrimitiveTopology::TriangleList,
                blended.clone(),
                Some(on_top.clone()),
            )),
            on_top_points_pipeline: pipeline(world(
                "overlay_points_on_top",
                "point_vertex",
                "point_fragment",
                point_buffers,
                wgpu::PrimitiveTopology::TriangleList,
                blended.clone(),
                Some(on_top.clone()),
            )),
            outline_edge_pipeline: pipeline(PipelineDescription {
                label: "overlay_outline_edge",
                pipeline_layout: &outline_pipeline_layout,
                shader_module: &outline_module,
                vertex_entry_point: "fullscreen_vertex",
                fragment_entry_point: "outline_fragment",
                vertex_buffers: &[],
                topology: wgpu::PrimitiveTopology::TriangleList,
                color_target: blended,
                depth_stencil: Some(on_top),
            }),
            grid_pipeline: pipeline(PipelineDescription {
                label: "overlay_grid",
                pipeline_layout: &grid_pipeline_layout,
                shader_module: &grid_module,
                vertex_entry_point: "grid_vertex",
                fragment_entry_point: "grid_fragment",
                vertex_buffers: &[],
                topology: wgpu::PrimitiveTopology::TriangleList,
                color_target: grid_target,
                depth_stencil: Some(grid_depth),
            }),
            outline_mask_pipeline: pipeline(world(
                "overlay_outline_mask",
                "mesh_mask_vertex",
                "mask_fragment",
                &mesh_buffers[..],
                wgpu::PrimitiveTopology::TriangleList,
                mask_target,
                None,
            )),
            mesh_pick_pipeline: pipeline(world(
                "overlay_pick_meshes",
                "mesh_pick_vertex",
                "value_fragment",
                &mesh_buffers[..],
                wgpu::PrimitiveTopology::TriangleList,
                value_target.clone(),
                Some(pick_mesh),
            )),
            handle_lines_pick_pipeline: pipeline(world(
                "overlay_pick_handle_lines",
                "handle_clip_vertex",
                "value_fragment",
                handle_line_buffers,
                wgpu::PrimitiveTopology::TriangleList,
                value_target.clone(),
                Some(pick_on_top.clone()),
            )),
            handle_triangles_pick_pipeline: pipeline(world(
                "overlay_pick_handle_triangles",
                "handle_vertex",
                "value_fragment",
                handle_buffers,
                wgpu::PrimitiveTopology::TriangleList,
                value_target,
                Some(pick_on_top),
            )),
            grid_parameters_buffer,
            grid_bind_group,
            outline_bind_group_layout,
            outline_parameters_buffer,
            tested_lines_buffer: StreamBuffer::new(
                "overlay_lines_tested",
                wgpu::BufferUsages::VERTEX,
            ),
            dimmed_lines_buffer: StreamBuffer::new(
                "overlay_lines_dimmed",
                wgpu::BufferUsages::VERTEX,
            ),
            on_top_lines_buffer: StreamBuffer::new(
                "overlay_lines_on_top",
                wgpu::BufferUsages::VERTEX,
            ),
            triangles_buffer: StreamBuffer::new("overlay_triangles", wgpu::BufferUsages::VERTEX),
            points_buffer: StreamBuffer::new("overlay_points", wgpu::BufferUsages::VERTEX),
            handle_lines_buffer: StreamBuffer::new(
                "overlay_handle_lines",
                wgpu::BufferUsages::VERTEX,
            ),
            handle_triangles_buffer: StreamBuffer::new(
                "overlay_handle_triangles",
                wgpu::BufferUsages::VERTEX,
            ),
            readback: picking::PickReadback::default(),
        }
    }

    /// Creates the targets a surface of `width` by `height` draws its overlay
    /// into.
    ///
    /// They are made when the surface is attached or resized, so the overlay's
    /// targets follow the surface they are sized to rather than a cache keyed
    /// by size.
    pub(crate) fn new_targets(
        &self,
        device: &wgpu::Device,
        (width, height): (u32, u32),
    ) -> OverlayTargets {
        let width = width.max(1);
        let height = height.max(1);
        let mask = create_target_texture(
            device,
            "overlay_outline_mask",
            width,
            height,
            OUTLINE_MASK_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let mask_view = mask.create_view(&wgpu::TextureViewDescriptor::default());
        let outline_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay_outline"),
            layout: &self.outline_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.outline_parameters_buffer.as_entire_binding(),
                },
            ],
        });
        OverlayTargets {
            width,
            height,
            mask_view,
            outline_bind_group,
            pick_targets: None,
        }
    }

    /// Records one surface's overlay after its chain: the outlines, the scene
    /// grid, the debug lines and the gizmos into the surface, and the pick
    /// buffer when this surface has picks waiting.
    pub fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        targets: &mut OverlayTargets,
        overlay_frame: OverlayFrame,
    ) {
        // Step 1: Work out what this surface draws.
        let frame = overlay_frame.frame;
        let picks: Vec<PickRequest> = frame
            .pick_requests
            .iter()
            .filter(|request| request.surface.0 == overlay_frame.surface)
            .copied()
            .collect();
        // The editor's chrome (outlines, gizmo handles and their picks) belongs
        // only on the surfaces the frame marks as editor views; the game's own
        // window draws the game and nothing else. The project's own debug lines
        // are not chrome, so they draw on every surface.
        let editor_view = frame
            .editor_views
            .contains(&pill_renderer_api::SurfaceId(overlay_frame.surface));
        let outlined_positions = if editor_view {
            outlined_queue_positions(frame, overlay_frame.render_queue)
        } else {
            Vec::new()
        };
        // The scene grid, when the frame asked for one on this surface. The
        // editor draws it on the viewports bound to a view; a game's surface
        // never draws one.
        let grid = if editor_view {
            frame.grid_cell_size
        } else {
            None
        };
        if frame.debug.is_empty()
            && outlined_positions.is_empty()
            && picks.is_empty()
            && grid.is_none()
        {
            return;
        }

        // Step 2: Prepare the pass inputs: the outline mask, the outline and
        // grid uniforms, and the vertex buffers.
        if !outlined_positions.is_empty() {
            self.record_outline_mask(encoder, targets, &overlay_frame, &outlined_positions);
            let parameters = OutlineParameters {
                color: frame.outlines.color,
                outline_settings: [OUTLINE_WIDTH, 0.0, 0.0, 0.0],
            };
            queue.write_buffer(
                &self.outline_parameters_buffer,
                0,
                bytemuck::bytes_of(&parameters),
            );
        }

        // Everything drawn into the surface, uploaded before the pass opens.
        let view_projection = &overlay_frame.camera.parameters_data.view_projection_matrix;
        let viewport_size = (
            overlay_frame.viewport.width.max(1) as f32,
            overlay_frame.viewport.height.max(1) as f32,
        );
        let (tested, dimmed, on_top) =
            line_vertices(&frame.debug, view_projection, viewport_size, editor_view);
        let triangles =
            triangle_vertices(&frame.debug, view_projection, viewport_size, editor_view);
        let points = point_vertices(&frame.debug, view_projection, viewport_size, editor_view);
        let tested_count = tested.len() as u32;
        let dimmed_count = dimmed.len() as u32;
        let on_top_count = on_top.len() as u32;
        let triangle_count = triangles.len() as u32;
        let point_count = points.len() as u32;
        let tested_buffer = self
            .tested_lines_buffer
            .write(device, queue, bytemuck::cast_slice(&tested))
            .cloned();
        let dimmed_buffer = self
            .dimmed_lines_buffer
            .write(device, queue, bytemuck::cast_slice(&dimmed))
            .cloned();
        let on_top_buffer = self
            .on_top_lines_buffer
            .write(device, queue, bytemuck::cast_slice(&on_top))
            .cloned();
        let triangles_buffer = self
            .triangles_buffer
            .write(device, queue, bytemuck::cast_slice(&triangles))
            .cloned();
        let points_buffer = self
            .points_buffer
            .write(device, queue, bytemuck::cast_slice(&points))
            .cloned();

        let draws_into_surface = !outlined_positions.is_empty()
            || tested_buffer.is_some()
            || dimmed_buffer.is_some()
            || on_top_buffer.is_some()
            || triangles_buffer.is_some()
            || points_buffer.is_some()
            || grid.is_some();
        if let Some(grid_cell_size) = grid {
            let parameters = GridParameters {
                cell_size: [grid_cell_size, 0.0, 0.0, 0.0],
                viewport: [
                    overlay_frame.viewport.x as f32,
                    overlay_frame.viewport.y as f32,
                    overlay_frame.viewport.width.max(1) as f32,
                    overlay_frame.viewport.height.max(1) as f32,
                ],
            };
            queue.write_buffer(
                &self.grid_parameters_buffer,
                0,
                bytemuck::bytes_of(&parameters),
            );
        }
        // Step 3: Draw the overlay into the surface.
        if draws_into_surface {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: overlay_frame.color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: overlay_frame.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            set_viewport(&mut pass, overlay_frame.viewport);
            // The grid first: it is the scene's floor, and every other overlay
            // primitive is drawn over it.
            if grid.is_some() {
                pass.set_pipeline(&self.grid_pipeline);
                pass.set_bind_group(0, &overlay_frame.camera.bind_group, &[]);
                pass.set_bind_group(1, &self.grid_bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            // Outlines first, so a debug line drawn over a selected entity
            // stays visible.
            if !outlined_positions.is_empty() {
                pass.set_pipeline(&self.outline_edge_pipeline);
                pass.set_bind_group(0, &targets.outline_bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            pass.set_bind_group(0, &overlay_frame.camera.bind_group, &[]);
            let world_draws = [
                (
                    &self.depth_tested_lines_pipeline,
                    &tested_buffer,
                    tested_count,
                ),
                (&self.dimmed_lines_pipeline, &dimmed_buffer, dimmed_count),
                (&self.on_top_lines_pipeline, &on_top_buffer, on_top_count),
                (
                    &self.on_top_triangles_pipeline,
                    &triangles_buffer,
                    triangle_count,
                ),
                (&self.on_top_points_pipeline, &points_buffer, point_count),
            ];
            for (pipeline, buffer, count) in world_draws {
                if let Some(buffer) = buffer {
                    pass.set_pipeline(pipeline);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..count, 0..1);
                }
            }
        }

        // Step 4: Draw the pick buffer.
        if !picks.is_empty() {
            self.record_picks(
                device,
                queue,
                encoder,
                targets,
                &overlay_frame,
                picks,
                editor_view,
            );
        }
    }

    /// Starts reading back the picks recorded by the frame that was just
    /// submitted.
    pub fn after_submit(&mut self) {
        self.readback.start_mapping();
    }

    /// Answers with nothing every request whose surface was not drawn this
    /// frame. Its pixel was never drawn, so the caller must not keep waiting
    /// for it.
    pub fn answer_undrawn(&mut self, requests: &[PickRequest], drawn_surfaces: &[u32]) {
        for request in requests {
            if !drawn_surfaces.contains(&request.surface.0) {
                self.readback.answer(*request, PickHit::Nothing);
            }
        }
    }

    /// Returns every pick answer that has arrived since the last call.
    pub fn take_results(&mut self, device: &wgpu::Device) -> Vec<PickResult> {
        self.readback.collect(device)
    }

    /// Draw the outlined instances' silhouettes into the mask.
    fn record_outline_mask(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        targets: &OverlayTargets,
        overlay_frame: &OverlayFrame,
        outlined_queue_positions: &[u32],
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay_outline_mask"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.mask_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        set_viewport(&mut pass, overlay_frame.viewport);
        pass.set_pipeline(&self.outline_mask_pipeline);
        pass.set_bind_group(0, &overlay_frame.camera.bind_group, &[]);
        pass.set_vertex_buffer(1, overlay_frame.instance_buffer.slice(..));
        draw_queue_positions(
            &mut pass,
            overlay_frame,
            outlined_queue_positions.iter().copied(),
        );
    }

    /// Draws the pick buffer and records the copies of the requested pixels.
    ///
    /// A surface that is not an editor view draws no handles into its pick
    /// buffer, because the gizmo is not there and nothing can pick a handle on
    /// it.
    #[allow(clippy::too_many_arguments)]
    fn record_picks(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        targets: &mut OverlayTargets,
        overlay_frame: &OverlayFrame,
        picks: Vec<PickRequest>,
        editor_view: bool,
    ) {
        let (width, height) = (targets.width, targets.height);
        // Step 1: Create the pick buffer and its depth the first time a pick
        // asks for them.
        let pick_targets = targets.pick_targets.get_or_insert_with(|| {
            let texture = create_target_texture(
                device,
                "overlay_pick",
                width,
                height,
                PICK_BUFFER_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let depth = create_target_texture(
                device,
                "overlay_pick_depth",
                width,
                height,
                PICK_BUFFER_DEPTH_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
            (texture, view, depth_view)
        });
        // Step 2: Upload the handle quads and triangles this surface picks
        // through.
        let view_projection = &overlay_frame.camera.parameters_data.view_projection_matrix;
        let viewport_size = (
            overlay_frame.viewport.width.max(1) as f32,
            overlay_frame.viewport.height.max(1) as f32,
        );
        let handle_lines = if editor_view {
            handle_line_vertices(&overlay_frame.frame.debug, view_projection, viewport_size)
        } else {
            Vec::new()
        };
        let handle_triangles = if editor_view {
            handle_triangle_vertices(&overlay_frame.frame.debug)
        } else {
            Vec::new()
        };
        let handle_line_count = handle_lines.len() as u32;
        let handle_triangle_count = handle_triangles.len() as u32;
        let handle_lines_buffer = self
            .handle_lines_buffer
            .write(device, queue, bytemuck::cast_slice(&handle_lines))
            .cloned();
        let handle_triangles_buffer = self
            .handle_triangles_buffer
            .write(device, queue, bytemuck::cast_slice(&handle_triangles))
            .cloned();
        // Step 3: Draw the meshes into the pick buffer, then the handles over
        // them.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay_pick"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &pick_targets.1,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &pick_targets.2,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            set_viewport(&mut pass, overlay_frame.viewport);
            pass.set_bind_group(0, &overlay_frame.camera.bind_group, &[]);
            pass.set_pipeline(&self.mesh_pick_pipeline);
            pass.set_vertex_buffer(1, overlay_frame.instance_buffer.slice(..));
            draw_queue_positions(
                &mut pass,
                overlay_frame,
                0..overlay_frame.render_queue.len() as u32,
            );
            // The handles are drawn over the meshes, so a handle that sits in
            // front of an entity is what a click there picks.
            let handle_draws = [
                (
                    &self.handle_lines_pick_pipeline,
                    &handle_lines_buffer,
                    handle_line_count,
                ),
                (
                    &self.handle_triangles_pick_pipeline,
                    &handle_triangles_buffer,
                    handle_triangle_count,
                ),
            ];
            for (pipeline, buffer, count) in handle_draws {
                if let Some(buffer) = buffer {
                    pass.set_pipeline(pipeline);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..count, 0..1);
                }
            }
        }
        // Step 4: Record the copies of the requested pixels.
        let entities: Vec<Entity> = overlay_frame
            .render_queue
            .iter()
            .map(|item| overlay_frame.frame.instances[item.frame_instance_index as usize].entity)
            .collect();
        self.readback
            .copy_requested_pixels(device, encoder, &pick_targets.0, picks, entities);
    }
}

// =============================================================================
// Pipeline Description
// =============================================================================

/// The settings one overlay pipeline differs in.
struct PipelineDescription<'a> {
    label: &'a str,
    pipeline_layout: &'a wgpu::PipelineLayout,
    shader_module: &'a wgpu::ShaderModule,
    vertex_entry_point: &'a str,
    fragment_entry_point: &'a str,
    vertex_buffers: &'a [wgpu::VertexBufferLayout<'a>],
    topology: wgpu::PrimitiveTopology,
    color_target: Option<wgpu::ColorTargetState>,
    depth_stencil: Option<wgpu::DepthStencilState>,
}

// =============================================================================
// Free Functions
// =============================================================================

/// A depth state that reads `format` with `compare`, writing when `write`.
fn depth_state(
    format: wgpu::TextureFormat,
    compare: wgpu::CompareFunction,
    write: bool,
) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

/// A single-sample 2D texture sized `width` by `height`.
fn create_target_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// Clip a pass to the region the scene was drawn into.
fn set_viewport(pass: &mut wgpu::RenderPass, viewport: RenderViewport) {
    pass.set_viewport(
        viewport.x as f32,
        viewport.y as f32,
        viewport.width as f32,
        viewport.height as f32,
        0.0,
        1.0,
    );
    pass.set_scissor_rect(viewport.x, viewport.y, viewport.width, viewport.height);
}

/// The queue positions of the instances whose entity is outlined.
fn outlined_queue_positions(frame: &RenderFrame, render_queue: &[RenderQueueItem]) -> Vec<u32> {
    if frame.outlines.entities.is_empty() {
        return Vec::new();
    }
    let outlined_entities: HashSet<Entity> = frame.outlines.entities.iter().copied().collect();
    render_queue
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            frame
                .instances
                .get(item.frame_instance_index as usize)
                .is_some_and(|instance| outlined_entities.contains(&instance.entity))
        })
        .map(|(position, _)| position as u32)
        .collect()
}

/// Draw each queue position's mesh as the instance at that position; runs of
/// consecutive positions sharing a mesh become one draw.
fn draw_queue_positions(
    pass: &mut wgpu::RenderPass,
    overlay_frame: &OverlayFrame,
    positions: impl Iterator<Item = u32>,
) {
    let mut bound_mesh: Option<(RendererMeshHandle, u32)> = None;
    let mut queue_position_run: Option<std::ops::Range<u32>> = None;
    for position in positions {
        let Some(item) = overlay_frame.render_queue.get(position as usize) else {
            continue;
        };
        let mesh_handle = item.mesh;
        let continues = bound_mesh.is_some_and(|(bound, _)| bound == mesh_handle)
            && queue_position_run
                .as_ref()
                .is_some_and(|run| run.end == position);
        if continues {
            if let Some(run) = &mut queue_position_run {
                run.end += 1;
            }
            continue;
        }
        if let (Some((_, index_count)), Some(run)) = (bound_mesh, queue_position_run.take()) {
            pass.draw_indexed(0..index_count, 0, run);
        }
        if bound_mesh.is_none_or(|(bound, _)| bound != mesh_handle) {
            let Some(mesh) = overlay_frame.storage.meshes.get(mesh_handle) else {
                bound_mesh = None;
                continue;
            };
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            bound_mesh = Some((mesh_handle, mesh.index_count));
        }
        queue_position_run = Some(position..position + 1);
    }
    if let (Some((_, index_count)), Some(run)) = (bound_mesh, queue_position_run) {
        pass.draw_indexed(0..index_count, 0, run);
    }
}

/// The frame's debug lines and gizmo lines as screen-space quads, split into
/// the lines the scene's depth hides, the lines that show through it in the
/// muted look, and the lines drawn on top.
///
/// Each quad is the segment widened by the line's half width plus its feather
/// on each side. Every corner carries its distance from the line's centre, so
/// the fragment shader can fade the edge. The widening happens in screen space
/// after projection, so a line keeps the same width at any depth. When
/// `editor_view` is false, the project's own debug lines still draw but the
/// editor's own line drawings - its drag guides and its gizmo handles - do not.
fn line_vertices(
    debug_frame: &DebugFrame,
    view_projection: &pill_core::math::Matrix4f,
    viewport_size: (f32, f32),
    editor_view: bool,
) -> (Vec<LineVertex>, Vec<LineVertex>, Vec<LineVertex>) {
    let offset = LINE_HALF_WIDTH + LINE_FEATHER;
    let push_line = |output: &mut Vec<LineVertex>,
                     start: [f32; 3],
                     end: [f32; 3],
                     color: [f32; 4],
                     hidden: f32| {
        let Some(quad) = line_quad(start, end, view_projection, viewport_size, offset) else {
            return;
        };
        for (clip_position, side) in quad.into_iter().zip(LINE_QUAD_SIDES) {
            output.push(LineVertex {
                clip_position,
                color,
                across_pixels: side * offset,
                hidden,
            });
        }
    };
    let dim = |color: [f32; 4], brightness: f32| {
        [
            color[0] * brightness,
            color[1] * brightness,
            color[2] * brightness,
            color[3],
        ]
    };
    let mut tested = Vec::new();
    let mut dimmed = Vec::new();
    let mut on_top = Vec::new();
    for line in debug_frame.visible_lines(editor_view) {
        match line.depth {
            DebugDepth::OnTop => push_line(&mut on_top, line.start, line.end, line.color, 0.0),
            DebugDepth::Hidden => {
                push_line(&mut tested, line.start, line.end, line.color, 0.0);
            }
            DebugDepth::Dimmed { hidden } => {
                // The part in front of the geometry keeps the line's colour, and
                // the part the geometry covers is drawn too, muted - the same
                // vertices, under the pipeline that keeps what the scene is in
                // front of.
                push_line(&mut tested, line.start, line.end, line.color, 0.0);
                let checker = if hidden.checker { 1.0 } else { 0.0 };
                push_line(
                    &mut dimmed,
                    line.start,
                    line.end,
                    dim(line.color, hidden.brightness.clamp(0.0, 1.0)),
                    checker,
                );
            }
        }
    }
    for line in &debug_frame.gizmo_lines {
        if editor_view {
            push_line(&mut on_top, line.start, line.end, line.color, 0.0);
        }
    }
    (tested, dimmed, on_top)
}

/// The frame's gizmo triangles as screen-space triangles whose corners are
/// pushed `TRIANGLE_FEATHER` pixels out, and which the fragment shader feathers
/// back down to the silhouette the triangles really have. Returns an empty list
/// when `editor_view` is false; see [`line_vertices`].
fn triangle_vertices(
    debug_frame: &DebugFrame,
    view_projection: &pill_core::math::Matrix4f,
    viewport_size: (f32, f32),
    editor_view: bool,
) -> Vec<TriangleVertex> {
    if !editor_view {
        return Vec::new();
    }
    let mut vertices = Vec::new();
    for triangle in &debug_frame.gizmo_triangles {
        let corners = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let polygon: Vec<ClipVertex> = triangle
            .corners
            .iter()
            .zip(corners)
            .map(|(corner, barycentric)| {
                let clip_position = (*view_projection
                    * glam::Vec4::new(corner[0], corner[1], corner[2], 1.0))
                .to_array();
                (clip_position, barycentric)
            })
            .collect();
        let polygon = clip_polygon_to_near(&polygon);
        if polygon.len() < 3 {
            continue;
        }
        let expanded = expand_polygon(&polygon, viewport_size, TRIANGLE_FEATHER);
        for index in 1..expanded.len() - 1 {
            for (clip_position, barycentric) in [expanded[0], expanded[index], expanded[index + 1]]
            {
                vertices.push(TriangleVertex {
                    clip_position,
                    color: triangle.color,
                    barycentric,
                });
            }
        }
    }
    vertices
}

/// The frame's gizmo points as screen-aligned quads of the requested pixel
/// size, each corner carrying its offset from the point's centre so the
/// fragment shader can test the silhouette per sample. Returns an empty list
/// when `editor_view` is false; see [`line_vertices`].
fn point_vertices(
    debug_frame: &DebugFrame,
    view_projection: &pill_core::math::Matrix4f,
    viewport_size: (f32, f32),
    editor_view: bool,
) -> Vec<PointVertex> {
    if !editor_view {
        return Vec::new();
    }
    let mut vertices = Vec::new();
    for point in &debug_frame.gizmo_points {
        let Some(quad) = point_quad(
            point.position,
            point.size_pixels,
            view_projection,
            viewport_size,
            0.0,
        ) else {
            continue;
        };
        let half_size_pixels = point.size_pixels.max(1.0) * 0.5;
        // The quad's corners in order, as offsets from the centre: two
        // triangles sharing the diagonal.
        let corners = [
            [-half_size_pixels, -half_size_pixels],
            [half_size_pixels, -half_size_pixels],
            [half_size_pixels, half_size_pixels],
            [-half_size_pixels, -half_size_pixels],
            [half_size_pixels, half_size_pixels],
            [-half_size_pixels, half_size_pixels],
        ];
        let shape = match point.shape {
            GizmoPointShape::Circle => 0,
            GizmoPointShape::Square => 1,
            GizmoPointShape::Diamond => 2,
        };
        for (clip_position, offset_pixels) in quad.into_iter().zip(corners) {
            vertices.push(PointVertex {
                clip_position,
                color: point.color,
                offset_pixels,
                half_size_pixels,
                shape,
            });
        }
    }
    vertices
}

/// The gizmo lines and point quads as the pick buffer draws them. Each is the
/// same geometry as the drawn handle, widened to the band the editor's hover
/// test accepts, so a pick covers the handle the pointer highlights.
fn handle_line_vertices(
    debug_frame: &DebugFrame,
    view_projection: &pill_core::math::Matrix4f,
    viewport_size: (f32, f32),
) -> Vec<HandleLineVertex> {
    let mut vertices = Vec::new();
    for line in &debug_frame.gizmo_lines {
        let value = PICK_HANDLE_BIT | line.handle;
        let Some(quad) = line_quad(
            line.start,
            line.end,
            view_projection,
            viewport_size,
            PICK_LINE_HALF_WIDTH,
        ) else {
            continue;
        };
        vertices.extend(quad.map(|clip_position| HandleLineVertex {
            clip_position,
            handle_value: value,
        }));
    }
    for point in &debug_frame.gizmo_points {
        // A mark that only shows a position is not a handle: it must not
        // shadow whatever the pointer is really over.
        if !point.pickable {
            continue;
        }
        let value = PICK_HANDLE_BIT | point.handle;
        let Some(quad) = point_quad(
            point.position,
            point.size_pixels,
            view_projection,
            viewport_size,
            PICK_POINT_SLACK,
        ) else {
            continue;
        };
        vertices.extend(quad.map(|clip_position| HandleLineVertex {
            clip_position,
            handle_value: value,
        }));
    }
    vertices
}

/// The frame's gizmo triangles as the pick buffer draws them, three
/// world-space vertices each.
fn handle_triangle_vertices(debug_frame: &DebugFrame) -> Vec<HandleVertex> {
    debug_frame
        .gizmo_triangles
        .iter()
        .flat_map(|triangle| {
            let value = PICK_HANDLE_BIT | triangle.handle;
            triangle.corners.map(|position| HandleVertex {
                world_position: position,
                handle_value: value,
            })
        })
        .collect()
}

/// The frame's labels as a `pill_ui_frame` list in surface pixels: glyph quads
/// over the bitmap font texture, each drawn over a shadow one glyph pixel down
/// and right. The UI painter draws it after the overlay, so text stays readable
/// over everything the overlay drew.
///
/// A label draws at the size, alignment and background it asks for. When
/// `editor_view` is false the editor's own labels are left out, exactly as its
/// line drawings are.
///
/// Debug colours are linear and the painter decodes sRGB vertex colours, so
/// each colour is converted on the way in and reaches the screen unchanged.
pub(crate) fn label_list(
    debug_frame: &DebugFrame,
    view_projection: &pill_core::math::Matrix4f,
    viewport: RenderViewport,
    editor_view: bool,
) -> DrawList {
    let mut draw_list = DrawList::new();
    let (atlas_width, atlas_height) = font::atlas_size();
    let (atlas_width, atlas_height) = (atlas_width as f32, atlas_height as f32);
    let to_painter_color = |color: [f32; 4]| {
        Color::rgba(
            linear_to_srgb(color[0]),
            linear_to_srgb(color[1]),
            linear_to_srgb(color[2]),
            color[3],
        )
    };
    for label in debug_frame.visible_labels(editor_view) {
        let Some((x, y)) = label_origin(label.anchor, view_projection, viewport) else {
            continue;
        };
        let scale = label.scale.max(0.1);
        let width = font::text_width(&label.text, scale);
        let height = font::line_height(scale);
        let x = match label.align {
            LabelAlign::Left => x,
            LabelAlign::Center => x - width * 0.5,
            LabelAlign::Right => x - width,
        };
        if let Some(background) = label.background {
            draw_list.push_quad(
                font::FONT_TEXTURE,
                Rect::from_min_size(Vector2f::new(x, y), Vector2f::new(width, height)),
                Vector2f::new(0.0, 0.0),
                Vector2f::new(0.0, 0.0),
                to_painter_color(background),
            );
        }
        for (offset, color) in [
            (font::GLYPH_SCALE * scale, LABEL_SHADOW_COLOR),
            (0.0, label.color),
        ] {
            let color = to_painter_color(color);
            for quad in font::layout_text(&label.text, x + offset, y + offset, scale) {
                let rect = Rect::from_min_size(
                    Vector2f::new(quad.origin[0], quad.origin[1]),
                    Vector2f::new(quad.size[0], quad.size[1]),
                );
                let uv_min = Vector2f::new(
                    quad.atlas_origin[0] as f32 / atlas_width,
                    quad.atlas_origin[1] as f32 / atlas_height,
                );
                let uv_max = Vector2f::new(
                    (quad.atlas_origin[0] + font::GLYPH_WIDTH) as f32 / atlas_width,
                    (quad.atlas_origin[1] + font::GLYPH_HEIGHT) as f32 / atlas_height,
                );
                draw_list.push_quad(font::FONT_TEXTURE, rect, uv_min, uv_max, color);
            }
        }
    }
    draw_list
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::UiRenderer;
    use pill_rendering_core::projection::to_pixels;

    /// Gizmo lines draw on top whatever their depth asks for. Every line
    /// becomes a six-vertex quad, and the pick quad carries the handle.
    #[test]
    fn lines_split_by_their_depth() {
        let mut draw = pill_renderer_api::DebugDraw::new(true);
        let style = pill_renderer_api::DebugStyle::default();
        draw.line([-1.0, 0.0, -5.0], [1.0, 0.0, -5.0], style);
        draw.line(
            [-1.0, 0.0, -5.0],
            [1.0, 0.0, -5.0],
            pill_renderer_api::DebugStyle {
                depth: pill_renderer_api::DebugDepth::OnTop,
                ..style
            },
        );
        draw.gizmo_line([-1.0, 0.0, -5.0], [1.0, 0.0, -5.0], [1.0, 4.0, 1.0, 1.0], 5);
        let frame = draw.collect_frame(0.016);
        let projection = glam::camera::rh::proj::directx::perspective(1.0, 1.0, 0.1, 100.0);
        let (tested, dimmed, on_top) = line_vertices(&frame, &projection, (100.0, 100.0), true);
        assert_eq!(tested.len(), 6, "the hidden line is depth-tested");
        assert!(dimmed.is_empty(), "nothing asked to show through geometry");
        assert_eq!(on_top.len(), 12, "the on-top line and the gizmo line");
        let handle_lines = handle_line_vertices(&frame, &projection, (100.0, 100.0));
        assert_eq!(handle_lines.len(), 6);
        assert_eq!(handle_lines[0].handle_value, PICK_HANDLE_BIT | 5);
    }

    /// A line that shows through geometry becomes three vertex sets: the part in
    /// front, the muted part behind it, and the check flag that marks the
    /// latter.
    #[test]
    fn a_dimmed_line_draws_its_hidden_part_in_the_muted_look() {
        let mut draw = pill_renderer_api::DebugDraw::new(true);
        draw.line(
            [-1.0, 0.0, -5.0],
            [1.0, 0.0, -5.0],
            pill_renderer_api::DebugStyle {
                depth: pill_renderer_api::DebugDepth::Dimmed {
                    hidden: Default::default(),
                },
                ..pill_renderer_api::DebugStyle::default()
            },
        );
        let frame = draw.collect_frame(0.016);
        let projection = glam::camera::rh::proj::directx::perspective(1.0, 1.0, 0.1, 100.0);
        let (tested, dimmed, on_top) = line_vertices(&frame, &projection, (100.0, 100.0), true);
        assert_eq!(tested.len(), 6, "the visible part keeps the line's colour");
        assert_eq!(dimmed.len(), 6, "the covered part is drawn too");
        assert!(on_top.is_empty());
        assert_eq!(dimmed[0].hidden, 1.0, "the covered part is checkered");
        assert!(
            dimmed[0].color[0] < tested[0].color[0],
            "the covered part is muted"
        );
    }

    /// A surface that is not an editor view draws the project's own debug
    /// lines and no editor chrome. The editor's chrome lines (the drag guides
    /// it draws as plain debug lines but marks as its own), its gizmo lines and
    /// its gizmo triangles are all left out.
    #[test]
    fn a_game_surface_draws_its_debug_lines_without_the_editor_chrome() {
        let mut draw = pill_renderer_api::DebugDraw::new(true);
        draw.line(
            [-1.0, 0.0, -5.0],
            [1.0, 0.0, -5.0],
            pill_renderer_api::DebugStyle::default(),
        );
        draw.line(
            [-1.0, 1.0, -5.0],
            [1.0, 1.0, -5.0],
            pill_renderer_api::DebugStyle {
                editor_only: true,
                depth: pill_renderer_api::DebugDepth::OnTop,
                ..pill_renderer_api::DebugStyle::default()
            },
        );
        draw.gizmo_line([-1.0, 0.0, -5.0], [1.0, 0.0, -5.0], [1.0, 4.0, 1.0, 1.0], 5);
        draw.gizmo_triangle(
            [[-1.0, -1.0, -5.0], [1.0, -1.0, -5.0], [0.0, 1.0, -5.0]],
            [1.0, 1.0, 1.0, 1.0],
            7,
        );
        let frame = draw.collect_frame(0.016);
        let projection = glam::camera::rh::proj::directx::perspective(1.0, 1.0, 0.1, 100.0);
        let (tested, dimmed, on_top) = line_vertices(&frame, &projection, (100.0, 100.0), false);
        assert_eq!(
            tested.len() + dimmed.len() + on_top.len(),
            6,
            "the project's own line stays, and the editor's guide does not"
        );
        assert!(triangle_vertices(&frame, &projection, (100.0, 100.0), false).is_empty());
        // The same frame on an editor view draws the chrome as well.
        let (editor_tested, editor_dimmed, editor_on_top) =
            line_vertices(&frame, &projection, (100.0, 100.0), true);
        assert_eq!(
            editor_tested.len() + editor_dimmed.len() + editor_on_top.len(),
            18,
            "the guide joins the gizmo line on an editor view"
        );
        assert_eq!(editor_on_top.len(), 12, "the guide is drawn on top");
        assert_eq!(
            triangle_vertices(&frame, &projection, (100.0, 100.0), true).len(),
            3
        );
    }

    /// A line entirely behind the camera draws nothing; one that crosses the
    /// near side draws the part that is in view.
    #[test]
    fn lines_behind_the_camera_are_clipped_away() {
        let projection = glam::camera::rh::proj::directx::perspective(1.0, 1.0, 0.1, 100.0);
        let mut draw = pill_renderer_api::DebugDraw::new(true);
        let style = pill_renderer_api::DebugStyle::default();
        draw.line([-1.0, 0.0, 1.0], [1.0, 0.0, 2.0], style);
        let behind = draw.collect_frame(0.016);
        let (tested, dimmed, on_top) = line_vertices(&behind, &projection, (100.0, 100.0), true);
        assert!(tested.is_empty() && dimmed.is_empty() && on_top.is_empty());

        draw.line([-0.2, 2.0, 5.0], [1.0, -1.0, -5.0], style);
        let in_view = draw.collect_frame(0.016);
        let (tested, dimmed, on_top) = line_vertices(&in_view, &projection, (100.0, 100.0), true);
        assert_eq!(tested.len() + dimmed.len() + on_top.len(), 6);
    }

    /// A gizmo triangle's corners move out past their projection, so the
    /// feather has a band to fade in. They keep their barycentric corners, so
    /// the coverage still measures the true edge.
    #[test]
    fn gizmo_triangles_expand_for_antialiasing() {
        let projection = glam::camera::rh::proj::directx::perspective(1.0, 1.0, 0.1, 100.0);
        let mut draw = pill_renderer_api::DebugDraw::new(true);
        let corners = [[-1.0, -1.0, -5.0], [1.0, -1.0, -5.0], [0.0, 1.0, -5.0]];
        draw.gizmo_triangle(corners, [1.0, 1.0, 1.0, 1.0], 7);
        let frame = draw.collect_frame(0.016);
        let vertices = triangle_vertices(&frame, &projection, (100.0, 100.0), true);
        assert_eq!(vertices.len(), 3);
        let mut barycentrics: Vec<[f32; 3]> =
            vertices.iter().map(|vertex| vertex.barycentric).collect();
        barycentrics.sort_by(|left, right| left.partial_cmp(right).unwrap());
        assert_eq!(
            barycentrics,
            vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]
        );
        let clip = (projection * glam::Vec4::new(corners[0][0], corners[0][1], corners[0][2], 1.0))
            .to_array();
        let (raw_x, raw_y) = to_pixels(clip, (100.0, 100.0));
        let (moved_x, moved_y) = to_pixels(vertices[0].clip_position, (100.0, 100.0));
        assert!(
            moved_x < raw_x && moved_y < raw_y,
            "{moved_x} {moved_y} vs {raw_x} {raw_y}"
        );

        let mut behind = pill_renderer_api::DebugDraw::new(true);
        behind.gizmo_triangle(
            [[-1.0, -1.0, 5.0], [1.0, -1.0, 5.0], [0.0, 1.0, 5.0]],
            [1.0, 1.0, 1.0, 1.0],
            7,
        );
        let behind = behind.collect_frame(0.016);
        assert!(triangle_vertices(&behind, &projection, (100.0, 100.0), true).is_empty());
    }

    /// A point keeps its pixel size in every view: the drawn quad covers the
    /// same number of pixels at two viewport sizes, and a game view draws
    /// none of it.
    #[test]
    fn points_keep_their_pixel_size_in_every_view() {
        let projection = glam::camera::rh::proj::directx::perspective(1.0, 1.0, 0.1, 100.0);
        let mut draw = pill_renderer_api::DebugDraw::new(true);
        draw.gizmo_point([0.0, 0.0, -5.0], 12.0, GizmoPointShape::Square, [1.0; 4], 3);
        let frame = draw.collect_frame(0.016);
        for viewport_size in [(200.0, 100.0), (800.0, 600.0)] {
            let vertices = point_vertices(&frame, &projection, viewport_size, true);
            assert_eq!(vertices.len(), 6);
            let pixels: Vec<(f32, f32)> = vertices
                .iter()
                .map(|vertex| to_pixels(vertex.clip_position, viewport_size))
                .collect();
            let min_x = pixels.iter().map(|at| at.0).fold(f32::INFINITY, f32::min);
            let max_x = pixels
                .iter()
                .map(|at| at.0)
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(
                (max_x - min_x - 12.0).abs() < 1e-3,
                "{viewport_size:?}: {} pixels wide",
                max_x - min_x
            );
        }
        assert!(point_vertices(&frame, &projection, (200.0, 100.0), false).is_empty());
    }

    /// A pick on a point answers with the handle it carries, and a point the
    /// camera is behind is not pickable.
    #[test]
    fn a_pick_on_a_point_carries_its_handle() {
        let projection = glam::camera::rh::proj::directx::perspective(1.0, 1.0, 0.1, 100.0);
        let mut draw = pill_renderer_api::DebugDraw::new(true);
        draw.gizmo_point(
            [0.0, 0.0, -5.0],
            10.0,
            GizmoPointShape::Circle,
            [1.0; 4],
            21,
        );
        let frame = draw.collect_frame(0.016);
        let vertices = handle_line_vertices(&frame, &projection, (100.0, 100.0));
        assert_eq!(vertices.len(), 6);
        assert!(vertices
            .iter()
            .all(|vertex| vertex.handle_value == PICK_HANDLE_BIT | 21));

        let mut behind = pill_renderer_api::DebugDraw::new(true);
        behind.gizmo_point([0.0, 0.0, 5.0], 10.0, GizmoPointShape::Circle, [1.0; 4], 21);
        let behind = behind.collect_frame(0.016);
        assert!(handle_line_vertices(&behind, &projection, (100.0, 100.0)).is_empty());
    }

    /// A mark is drawn but never picked, so a click on it reaches the scene
    /// behind it.
    #[test]
    fn a_mark_is_drawn_but_not_picked() {
        let projection = glam::camera::rh::proj::directx::perspective(1.0, 1.0, 0.1, 100.0);
        let mut draw = pill_renderer_api::DebugDraw::new(true);
        draw.gizmo_mark([0.0, 0.0, -5.0], 9.0, GizmoPointShape::Square, [1.0; 4]);
        let frame = draw.collect_frame(0.016);
        assert_eq!(
            point_vertices(&frame, &projection, (200.0, 100.0), true).len(),
            6,
            "the mark is still drawn in an editor view"
        );
        assert!(
            handle_line_vertices(&frame, &projection, (200.0, 100.0)).is_empty(),
            "a mark must not enter the pick buffer"
        );
    }

    /// A device and queue without a surface, forcing the platform's fallback
    /// adapter (WARP on Windows) so the test runs without a GPU. Returns `None`
    /// when the machine offers no adapter at all.
    async fn windowless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            flags: wgpu::InstanceFlags::from_build_config(),
            backend_options: wgpu::BackendOptions::default(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: None,
                force_fallback_adapter: true,
                apply_limit_buckets: false,
            })
            .await
            .ok()?;
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("overlay label test device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .ok()
    }

    /// Draw `draw_list` with `painter` into a transparent sRGB target of `size`
    /// and read its pixels back as tightly packed RGBA8 rows.
    fn render_pixels(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        painter: &mut UiRenderer,
        size: (u32, u32),
        draw_list: &DrawList,
    ) -> Vec<u8> {
        let target_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlay label test target"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());
        // A texture-to-buffer copy pads every row to the copy alignment.
        let row_bytes = size.0 as usize * 4;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded_row_bytes = row_bytes.div_ceil(alignment) * alignment;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay label test readback"),
            size: (padded_row_bytes * size.1 as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("overlay label test encoder"),
        });
        painter.render(
            device,
            queue,
            &mut encoder,
            &view,
            size,
            &[(draw_list, 1.0)],
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes as u32),
                    rows_per_image: Some(size.1),
                },
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("the readback finishes");
        receiver
            .recv()
            .expect("the map callback ran")
            .expect("the readback maps");
        let mapped = slice.get_mapped_range().expect("the mapped range");
        let mut pixels = Vec::with_capacity(row_bytes * size.1 as usize);
        for row in 0..size.1 as usize {
            let start = row * padded_row_bytes;
            pixels.extend_from_slice(&mapped[start..start + row_bytes]);
        }
        pixels
    }

    /// A debug label drawn through the UI painter shows its linear debug
    /// colour, encoded into the sRGB target, only where its glyph is lit, and
    /// over its shadow one glyph pixel down and right. The test is skipped on a
    /// machine with no adapter at all.
    #[test]
    fn labels_draw_through_the_painter_in_their_linear_colour() {
        let Some((device, queue)) = pill_core::platform::futures::block_on(windowless_device())
        else {
            eprintln!("no wgpu adapter on this machine; the label render test is skipped");
            return;
        };
        let mut painter = UiRenderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
        painter.apply_texture_set(&device, &queue, &font::texture_set());

        // `|` lights its glyph's middle column, so the text covers x 4..6 and
        // y 0..14, and the shadow x 6..8 and y 2..16.
        let mut draw = pill_renderer_api::DebugDraw::new(true);
        let style = pill_renderer_api::DebugStyle::colored([0.5, 0.25, 1.0, 1.0]);
        draw.label_screen(0.0, 0.0, "|", style);
        let frame = draw.collect_frame(0.016);
        let label_draw_list = label_list(
            &frame,
            &pill_core::math::Matrix4f::IDENTITY,
            RenderViewport::new(0, 0, 32, 32),
            true,
        );
        let size = (32u32, 32u32);
        let pixels = render_pixels(&device, &queue, &mut painter, size, &label_draw_list);
        let pixel = |x: u32, y: u32| {
            let start = ((y * size.0 + x) * 4) as usize;
            [
                pixels[start],
                pixels[start + 1],
                pixels[start + 2],
                pixels[start + 3],
            ]
        };

        // Linear 0.5 and 0.25 encode to sRGB 188 and 137.
        let text = pixel(5, 1);
        let expected = [188, 137, 255, 255];
        for (channel, (got, want)) in text.iter().zip(expected).enumerate() {
            assert!(
                got.abs_diff(want) <= 1,
                "text channel {channel}: {text:?} vs {expected:?}"
            );
        }
        let shadow = pixel(7, 8);
        assert_eq!(&shadow[..3], &[0, 0, 0], "the shadow is black: {shadow:?}");
        assert!(
            shadow[3].abs_diff(217) <= 1,
            "the shadow is 85% opaque: {shadow:?}"
        );
        assert_eq!(pixel(20, 20), [0, 0, 0, 0], "nothing outside the label");
    }
}
