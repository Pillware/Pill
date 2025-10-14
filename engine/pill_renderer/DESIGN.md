# Pill Renderer vNext (wgpu) – Minimal MVP, API-Correct

### Goals

- Lean MVP aligned with wgpu/Vulkan semantics (no GL-era patterns).
- Master pipeline from `WindowComponent`, renders nothing by default.
- Mesh visible by Milestone 2 via a single pass; complexity grows incrementally.
- Separate resource mutation from drawing; small, stable public API.

### Hardware/API alignment from TALK_0.md (applied to MVP)

- Separate data modification from drawing: precreate PSOs/bind groups; upload dynamic data once per pass.
- Use bind groups (no per-draw shader resource setup); convention: set 0 (globals), set 1 (material), set 2 (optional).
- Dynamic offsets over per-draw map/unmap; per-frame ring buffer; obey min uniform offset alignment (256 bytes on wgpu).
- Avoid base-instance tricks and push-constant reliance; stick to uniform/SSBO with dynamic offsets.
- Pack meshes later; MVP uses per-mesh vertex/index buffers; introduce packing after correctness (Milestone 8 optional).

### Surface/window init via WindowComponent (wgpu-correct)

- Choose `SurfaceConfiguration` from capabilities:
  - Format: pick first sRGB-capable if present (prefer `Rgba8UnormSrgb`), else first supported.
  - Usage: `TextureUsages::RENDER_ATTACHMENT` (+ `TEXTURE_BINDING` for offscreen targets later).
  - Present mode: `Fifo` for vsync=true; for vsync=false try `Mailbox` then `Immediate`, else fall back to `Fifo`.
  - Alpha: default `CompositeAlphaMode::Auto`.
- Handle `SurfaceError` properly: `Lost|Outdated` → reconfigure; `Timeout` → skip frame; `OutOfMemory` → exit gracefully.
- Depth: create a depth texture per size change (`Depth24Plus` or `Depth32Float`), usage `RENDER_ATTACHMENT`.

### Core minimal public API (stable names; MVP may leave some unimplemented until their milestone)

```rust
pub struct Renderer;
impl Renderer {
    pub fn new_from_window_component(window: &WindowComponent) -> anyhow::Result<Self>;
    pub fn master(&mut self) -> MasterPipeline;

    // Views (Milestone 3)
    pub fn get_view_from_camera(&self, cam: &CameraView) -> ViewId;
    pub fn get_view_from_entities<'a, I>(&self, cam: &CameraView, entities: I) -> ViewId
      where I: IntoIterator<Item = EntityRenderProxy<'a>>;

    // Resource access (Milestone 2)
    pub fn resources(&mut self) -> ResourceManager;
}

pub struct MasterPipeline;
impl MasterPipeline {
    pub fn add_pass(&mut self, pass: RenderPassDesc) -> PassId;
    pub fn remove_pass(&mut self, id: PassId);
    pub fn set_order(&mut self, new_order: &[PassId]);
}

pub struct RenderPassDesc {
    pub name: &'static str,
    pub target: TargetDesc,          // swapchain or offscreen texture
    pub clear: ClearDesc,
    pub subpipeline: Subpipeline,
}

pub enum TargetDesc { Swapchain, Texture { tex: Handle<Texture>, level: u32, layer: u32 } }

pub struct Viewport { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }

pub struct ClearDesc { pub color: Option<[f32;4]>, pub depth: Option<f32>, pub stencil: Option<u32> }

pub struct Subpipeline {
    pub pipeline: Handle<GraphicsPipeline>,  // PSO (wgpu::RenderPipeline)
    pub globals: Handle<BindGroup>,          // set 0
    pub draws: DrawRecipe,                   // how to record draws
}

pub enum DrawRecipe {
    FromView { view: ViewId, material_set: Handle<BindGroup>, shader_set: Option<Handle<BindGroup>> },
    Inline(Box<dyn Fn(&mut DrawListBuilder) + Send + Sync>),
}

pub struct DrawListBuilder;
impl DrawListBuilder {
    pub fn set_pipeline(&mut self, pso: Handle<GraphicsPipeline>);
    pub fn set_bind_groups_with_offsets(
        &mut self,
        g0: Handle<BindGroup>,
        g1: Option<Handle<BindGroup>>, g2: Option<Handle<BindGroup>>,
        offs0: Option<u32>, offs1: Option<u32>, offs2: Option<u32>
    );
    pub fn set_mesh(&mut self, mesh: Handle<Mesh>, base_vertex: u32, base_index: u32);
    pub fn push_per_draw<T: Pod>(&mut self, data: &T) -> u32; // returns 256B-aligned dynamic offset
    pub fn draw_indexed(&mut self, index_count: u32, first_index: u32);
}

pub struct ResourceManager;
impl ResourceManager {
    pub fn create_texture(&mut self, desc: TextureDesc) -> Handle<Texture>;
    pub fn create_buffer(&mut self, desc: BufferDesc) -> Handle<Buffer>;
    pub fn create_bind_group(&mut self, desc: BindGroupDesc) -> Handle<BindGroup>;
    pub fn create_pipeline(&mut self, desc: GraphicsPipelineDesc) -> Handle<GraphicsPipeline>;
    pub fn upload_mesh(&mut self, mesh: CpuMesh) -> Handle<Mesh>;
}

// Milestone 3
pub struct CameraView { pub view: glam::Mat4, pub proj: glam::Mat4, pub frustum: Frustum }
#[repr(transparent)] pub struct ViewId(u32);

pub struct EntityRenderProxy<'a> {
    pub world_from_object: glam::Mat4,
    pub mesh: Handle<Mesh>,
    pub material: Handle<BindGroup>,
    pub bounds_ws: Aabb,
    pub user_data: &'a dyn std::any::Any,
}
```

### WGSL shader correctness (M2 baseline)

- Use WGSL (portable across native/web). Uniforms must respect WGSL alignment; per-draw UBO struct padded to 256 bytes.
- Example (unlit color) minimal pair:
```wgsl
// globals @group(0) @binding(0)
struct Globals { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> G: Globals;

struct VSIn { @location(0) pos: vec3<f32>; };
struct VSOut { @builtin(position) pos: vec4<f32>; };

@vertex fn vs_main(input: VSIn) -> VSOut {
  var out: VSOut;
  out.pos = G.view_proj * vec4<f32>(input.pos, 1.0);
  return out;
}

@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.7, 0.3, 1.0); }
```


### Milestones (incremental, debuggable) with API-correctness notes

1) [Done] Bootstrap: Black Window

- Expected result: Black frame; vsync honored; resizes OK.
- Client API: `Renderer::new_from_window_component(&wc)?;` run loop; no passes.
- Implementables:
  - winit + wgpu init; capabilities-driven `SurfaceConfiguration`.
  - Depth texture created but unused yet; recreate on resize.
  - Surface error handling (Lost/Outdated/Timeout/OutOfMemory).
- wgpu correctness:
  - Use `surface.get_capabilities(&adapter)` for format/present mode.
  - Set `usage: RENDER_ATTACHMENT` and correct `alpha_mode`.

2) [Done] Hello Mesh: Single Pass, Inline Draw

- Expected result: Pill mesh or fallback triangle; fixed MVP (identity or simple ortho) hardcoded; depth test on.
- Client API: Create mesh + pipeline; `Inline` pass records one draw.
- Implementables:
  - `ResourceManager`: create vertex/index buffers with `VERTEX|INDEX|COPY_DST` usages.
  - Minimal pipeline: WGSL VS/FS, color target = surface format, depth = `Depth24Plus`.
  - `DrawListBuilder` encodes: set pipeline, set vertex/index buffers, set bind groups, draw.
- wgpu correctness:
  - Index format `Uint32` or `Uint16` consistent with buffer.
  - Pipeline layout: explicit `BindGroupLayout` for globals even if empty; avoid implicit layout.
  - Clear ops via `RenderPassColorAttachment` and depth attachment.

3) [Done] Camera + FromView Culling

- Expected result: Camera matrices applied; frustum culling removes offscreen entities.
- Client API: Build `CameraView`; `get_view_from_entities`; pass uses `FromView`.
- Implementables:
  - CPU frustum planes; AABB test; build draw list from filtered entities.
  - Per-entity MVP uniform placed in per-frame uniform buffer; 256B alignment.
- wgpu correctness:
  - Uniform buffer alignment (offset multiple of 256); `BindGroupLayoutEntry` for uniform with `has_dynamic_offset: true`.
  - Call `set_bind_group(group_index, &bg, &[offset])` matching the layout.

4) [Done] Materials + Textures (set 1)

- Expected result: Textured mesh with material BG; per-entity tint supported from UBO.
- Client API: `create_texture`, `create_bind_group(material_desc)`; assign material handle.
- Implementables:
  - Texture creation with `TEXTURE_BINDING | COPY_DST`; view with correct `TextureViewDescriptor`.
  - Sampler (filtering); material BG layout: `@group(1) @binding(0) texture_2d<f32>`, `@group(1) @binding(1) sampler`.
  - FS samples sRGB texture; output to sRGB surface.
- wgpu correctness:
  - Use sRGB surface format and sRGB textures for correct gamma; or convert in shader.

5) [Done] Per-frame Ring Buffer + Dynamic Offsets

- Expected result: No per-draw map/unmap; dynamic uniform offsets.
- Client API: `push_per_draw` returns aligned offset; pass offsets via `set_bind_groups_with_offsets`.
- Implementables:
  - CPU staging buffer (MAP_WRITE) per frame; `queue.write_buffer` into GPU-visible uniform buffer.
  - Layout entries with `has_dynamic_offset=true` only for dynamic bindings; offsets are multiples of 256.
- wgpu correctness:
  - Avoid mapping GPU-only buffers; prefer `write_buffer` for simplicity.

6) [Done] Multi-pass + Offscreen Targets + Quad Composition

- Expected result: Offscreen render (T0) → FSQ/quads composite to swapchain; quads can place T0 anywhere in the framebuffer.
- Client API: Add two passes: `Texture` then `Swapchain`; post uses `Inline` to draw a quad (or fullscreen triangle) sampling T0 with UVs that map to desired target rect.
- Implementables:
  - Offscreen target texture with `RENDER_ATTACHMENT | TEXTURE_BINDING`; correct view format; sampler.
  - Composition shader samples T0; quad vertex data/uniforms carry position/size; no viewport/scissor used.
- wgpu correctness:
  - Presentable texture cannot be sampled; offscreen must be separate texture.

7) Resource Pools + Hot/Cold Split

- Expected result: Opaque `Handle<T>` pools; safe reuse; minimal hot metadata.
- Client API: Unchanged; `ResourceManager` returns handles.
- Implementables:
  - Freelist + generation; hot (wgpu handles + light metadata) vs cold (descs, debug names).
- wgpu correctness:
  - Keep GPU handles alive until all pass encoders referencing them are submitted.

8) [Basic] Draw List Optimization
  Done: sort by PSo/material/mesh
  Remaining: optional client sort keys, per-pipeline bind group layout identity safety, and a minor mesh buffer rebind micro-optimization.
- Expected result: Fewer state changes; sorted by PSO/material.
- Client API: Optional sort keys; otherwise internal.
- Implementables:
  - Build sort keys; stable sort; encode draws.
- wgpu correctness:
  - Ensure binds set when changing PSO/material; match pipeline layouts.

### Client usage snapshots

- M1: just create renderer from `WindowComponent`.
- M2: create mesh + pipeline; one Inline pass to draw.
- M3: create `CameraView`, build `ViewId`, use `FromView`.
- M4–M6: add material BG + textures; add offscreen + post; use viewport rects.

### Files to add later

- `engine/pill_renderer/DESIGN.md` (this document)
- `engine/pill_renderer/src/renderer.rs`
- `engine/pill_renderer/src/pass.rs`
- `engine/pill_renderer/src/resources/*`
- `engine/pill_renderer/src/view.rs`

### References

- wgpu: `https://docs.rs/wgpu/latest/wgpu/`
- winit: `https://docs.rs/winit/latest/winit/`
- WGSL: `https://www.w3.org/TR/WGSL/`