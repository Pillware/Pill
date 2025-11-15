# Pill Renderer vNext (wgpu) API Design

### Goals

- Modern, low-overhead rendering on wgpu (no GL-era patterns).
- Master pipeline initializes the window from a global `WindowComponent` and renders nothing by default.
- Users can define subpipelines/passes in game code, add them at runtime, and control targets/rects.
- Views: `get_view(camera)` and `get_view(entities)` produce culled renderables for passes.
- Stable API with prebuilt pipelines, bind groups, and generational handles.

### Principles distilled from TALK_0.md

- **Separate data modification from drawing**: precreate pipelines, bind groups; upload dynamic data per-pass in batches.
- **Generational handles + hot/cold SoA** for resources (textures, buffers, pipelines, materials).
- **Three bind group slots** convention (globals, material, shader-specific), dynamic offsets for temp data.
- **Minimize API calls per draw**: packed meshes, prebound resources, small draw metadata.

### High-level architecture

```
+---------------------------+        +---------------------------+
| game.rs (user code)       |        | pill_renderer (wgpu)      |
| - builds passes           |  uses  | - Renderer                |
| - defines views           +------->| - MasterPipeline          |
| - registers renderables   |        | - PassBuilder/PassGraph   |
+---------------------------+        | - ResourceManager (pools) |
                                     | - ViewBuilder + Culling   |
                                     | - FrameTempAllocator      |
                                     +---------------------------+
```

### Surface/window init via WindowComponent

- `WindowComponent` fields consumed at init: `width`, `height`, `vsync`, `fullscreen/windowed`, `title`.
- Map to winit + wgpu `SurfaceConfiguration`:
  - `vsync=true` → `PresentMode::Fifo`.
  - `vsync=false` → prefer `Mailbox` else `Immediate` if supported; fall back to `Fifo`.
  - Auto-pick `TextureFormat` via `surface.get_capabilities().formats[0]` unless overridden.

### Core types (public API)

```rust
// Handles (32-bit index + 32-bit gen)
#[repr(transparent)] pub struct Handle<T>(u64);

// Renderer creation
pub struct Renderer;
impl Renderer {
    pub fn new_from_window_component(window: &WindowComponent) -> anyhow::Result<Self>;
    pub fn master(&mut self) -> MasterPipeline;

    // Views
    pub fn get_view_from_camera(&self, cam: &CameraView) -> ViewId;
    pub fn get_view_from_entities<'a, I>(&self, cam: &CameraView, entities: I) -> ViewId
      where I: IntoIterator<Item = EntityRenderProxy<'a>>;
}

// Master pipeline is empty; users add passes at runtime
pub struct MasterPipeline;
impl MasterPipeline {
    pub fn add_pass(&mut self, pass: RenderPassDesc) -> PassId;
    pub fn remove_pass(&mut self, id: PassId);
    pub fn set_order(&mut self, new_order: &[PassId]);
}

// Pass description (immutable after build)
pub struct RenderPassDesc {
    pub name: &'static str,
    pub target: TargetDesc,        // swapchain or texture
    pub viewport: Option<Viewport>,// rect in window
    pub clear: ClearDesc,          // per-attachment clears
    pub subpipeline: Subpipeline,  // PSO/shaders, draw list recipe
}

pub enum TargetDesc { Swapchain, Texture { tex: Handle<Texture>, level: u32, layer: u32 } }

pub struct Viewport { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }

pub struct ClearDesc { pub color: Option<[f32;4]>, pub depth: Option<f32>, pub stencil: Option<u32> }

// Subpipeline: bind groups, pipeline, and a draw callback/recipe
pub struct Subpipeline {
    pub pipeline: Handle<GraphicsPipeline>,           // PSO
    pub globals: Handle<BindGroup>,                   // set 0
    pub draws: DrawRecipe,                            // how to build draw list per frame
}

pub enum DrawRecipe {
    // Renderer builds draws from a View (culled scene)
    FromView { view: ViewId, material_set: Handle<BindGroup> /* set 1 */, shader_set: Option<Handle<BindGroup>> /* set 2 */ },
    // User supplies a closure to record draws
    Inline(Box<dyn Fn(&mut DrawListBuilder) + Send + Sync>),
}

// View building and culling
pub struct CameraView { pub view: glam::Mat4, pub proj: glam::Mat4, pub frustum: Frustum }
#[repr(transparent)] pub struct ViewId(u32);

pub struct EntityRenderProxy<'a> {
    pub world_from_object: glam::Mat4,
    pub mesh: Handle<Mesh>,
    pub material: Handle<BindGroup>,
    pub bounds_ws: Aabb, // for culling
    pub user_data: &'a dyn std::any::Any, // optional
}

// Draw list
pub struct DrawListBuilder; // state-sorted writes; per-frame temp data
impl DrawListBuilder {
    pub fn set_pipeline(&mut self, pso: Handle<GraphicsPipeline>);
    pub fn set_bind_groups(&mut self, g0: Handle<BindGroup>, g1: Option<Handle<BindGroup>>, g2: Option<Handle<BindGroup>>);
    pub fn set_mesh(&mut self, mesh: Handle<Mesh>, base_v: u32, base_i: u32);
    pub fn push_per_draw<T: Pod>(&mut self, data: &T) -> u32; // returns dynamic offset
    pub fn draw_indexed(&mut self, index_count: u32, first_index: u32);
}

// Resource creation (pre-pass)
pub struct ResourceManager;
impl ResourceManager {
    pub fn create_texture(&mut self, desc: TextureDesc) -> Handle<Texture>;
    pub fn create_buffer(&mut self, desc: BufferDesc) -> Handle<Buffer>;
    pub fn create_bind_group(&mut self, desc: BindGroupDesc) -> Handle<BindGroup>;
    pub fn create_pipeline(&mut self, desc: GraphicsPipelineDesc) -> Handle<GraphicsPipeline>;
    pub fn upload_mesh(&mut self, mesh: CpuMesh) -> Handle<Mesh>; // packed mesh heap
}
```

### Data ownership and performance

- All resources live in **typed generational pools** with hot/cold split:
  - Hot: GPU handles, minimal render-time metadata (format, size, bind group ids).
  - Cold: debug name, creation descriptors, CPU-side helpers.
- No per-draw map/unmap. Dynamic per-draw data is uploaded through a per-frame **ring buffer** (COPY_DST) and bound via **dynamic offsets**.
- Bind group convention:
  - set 0: pass globals (camera, sun, shadow maps).
  - set 1: material (textures + material UBO/SSBO).
  - set 2: shader-specific (optional).

### Pass graph and execution

```
Frame N:
+-------------+     +-------------+     +-------------+
| Pass A      | --> | Pass B      | --> | Pass C      |
| target=T0   |     | target=T1   |     | target=Swap |
+-------------+     +-------------+     +-------------+
   writes T0           reads T0           reads T1
```

- Default: linear order; optional read-after-write deps declared in `RenderPassDesc` for future graphing.
- Each pass gets a viewport; swapchain always presents full frame, viewport limits draw area.

### How users create renderables from scene entities

- Provide two adapters:
  - `renderer.get_view_from_camera(cam)` → returns `ViewId` with built frustum.
  - `renderer.get_view_from_entities(cam, iter)` → users pass an iterator of `EntityRenderProxy`; renderer does frustum culling and produces a `DrawList` internally for `DrawRecipe::FromView` passes.
- Users can also bypass views and push draws manually via `DrawRecipe::Inline` for full-screen quads, gizmos, etc.

### Client usage (game.rs excerpts)

```rust
fn start(&self, engine: &mut Engine) -> Result<()> {
    // Window setup via component
    let wc = WindowComponent { width: 1280, height: 720, vsync: true, windowed: true, title: "Pill".into() };

    // Create renderer
    let mut renderer = pill_renderer::Renderer::new_from_window_component(&wc)?;

    // Create resources
    let mut rm = renderer.resources();
    let mesh = rm.upload_mesh(load_obj("models/pill.obj")?);
    let globals_layout = layouts::globals(); // provided by engine or shader gen
    let material_bg = rm.create_bind_group(material_desc());
    let pipeline = rm.create_pipeline(pipeline_desc(globals_layout, material_layout(), swapchain_format(&renderer)));

    // Build camera
    let cam = CameraView::from_transform_fov(transform, fov_y, aspect, near, far);

    // Define a view from entities
    let view = renderer.get_view_from_entities(&cam, engine.iter_renderables()?);

    // Build an opaque pass targeting the swapchain
    let pass_id = renderer.master().add_pass(RenderPassDesc {
        name: "Opaque",
        target: TargetDesc::Swapchain,
        viewport: Some(Viewport { x: 0, y: 0, w: wc.width, h: wc.height }),
        clear: ClearDesc { color: Some([0.02,0.02,0.03,1.0]), depth: Some(1.0), stencil: None },
        subpipeline: Subpipeline {
            pipeline,
            globals: build_globals_bg(&mut rm, &cam)?,
            draws: DrawRecipe::FromView { view, material_set: material_bg, shader_set: None },
        },
    });

    // Fullscreen quad post-process pass (manual draws)
    let fsq_pso = rm.create_pipeline(fsq_pipeline_desc(swapchain_format(&renderer)));
    renderer.master().add_pass(RenderPassDesc {
        name: "Post",
        target: TargetDesc::Swapchain,
        viewport: None,
        clear: ClearDesc { color: None, depth: None, stencil: None },
        subpipeline: Subpipeline {
            pipeline: fsq_pso,
            globals: default_globals(),
            draws: DrawRecipe::Inline(Box::new(|dl| {
                dl.set_pipeline(fsq_pso);
                dl.set_bind_groups(globals_bg, None, None);
                dl.set_mesh(fullscreen_triangle_mesh(), 0, 0);
                dl.draw_indexed(3, 0);
            })),
        },
    });

    Ok(())
}
```

### WGPU details and constraints

- Dynamic offsets require matching `BindGroupLayoutEntry` with `has_dynamic_offset: true`.
- Per-frame ring buffer: allocate once `MAP_WRITE | COPY_SRC` staging on CPU, `queue.write_buffer` into GPU-visible dynamic UBO/SSBO chunks.
- Swapchain rects: presentation is full-surface; use viewport/scissor to restrict drawing area.
- Pipeline creation uses `depth24plus` or `depth32float` depth formats; precreate all PSOs at startup.

### Files to add (later implementation)

- `engine/pill_renderer/DESIGN.md` (this document)
- `engine/pill_renderer/src/renderer.rs` (Renderer + MasterPipeline)
- `engine/pill_renderer/src/pass.rs` (RenderPassDesc, Subpipeline, DrawListBuilder)
- `engine/pill_renderer/src/resources/*` (pools, handles, upload)
- `engine/pill_renderer/src/view.rs` (CameraView, culling)


### Milestones (incremental, debuggable)

1) Bootstrap: Black Window
- Expected result: Resizable window presents a black frame based on `WindowComponent` (vsync, size). No passes.
- Expected client API:
  - `let mut renderer = Renderer::new_from_window_component(&wc)?;` then run engine loop; no passes added.
- Implementables:
  - winit + wgpu init; `SurfaceConfiguration` using present mode from vsync; acquire→clear→present.
  - `MasterPipeline` exists but is empty; resize handled.

2) Hello Mesh: Single Pass, Inline Draw
- Expected result: Pill mesh on screen; fallback to built-in triangle if asset missing. Fixed MVP in shader.
- Expected client API:
  - Create mesh + pipeline; `master.add_pass(RenderPassDesc { draws: Inline(|dl| { set_pipeline; set_mesh; draw_indexed; }) })`.
- Implementables:
  - Minimal `ResourceManager` for buffers, shader module, pipeline; depth buffer; unlit shader.
  - `DrawListBuilder`: `set_pipeline`, `set_mesh`, `draw_indexed`.

3) Camera + FromView Culling
- Expected result: Camera matrices applied; cull out-of-frustum entities.
- Expected client API:
  - Build `CameraView`; `renderer.get_view_from_entities(&cam, iter)`; pass uses `DrawRecipe::FromView { view, material_set, .. }`.
- Implementables:
  - Frustum planes; `EntityRenderProxy`; per-entity MVP uniform; basic sort by PSO/material.

4) Materials + Textures (set 1)
- Expected result: Textured opaque geometry via material bind group (set 1); optional per-entity tint/scalar.
- Expected client API:
  - `create_texture`, `create_bind_group(material_desc)`; assign to entities.
- Implementables:
  - Texture upload (RGBA8 sRGB), sampler; material BG layout; shader samples `albedo`.

5) Per-frame Ring Buffer + Dynamic Offsets
- Expected result: Per-draw uniforms via dynamic offsets; no per-draw map/unmap.
- Expected client API:
  - `let off = dl.push_per_draw(&PerDraw); set_bind_groups(globals, Some(material), None);` offsets applied.
- Implementables:
  - CPU staging + `queue.write_buffer`; layouts with `has_dynamic_offset: true`; per-frame ring.

6) Multi-pass + Offscreen Targets + Viewport Rects
- Expected result: Render to offscreen texture, post-process FSQ to swapchain; passes honor viewport/scissor rects.
- Expected client API:
  - Two passes: `TargetDesc::Texture` then `TargetDesc::Swapchain`; post pass via `Inline`.
- Implementables:
  - Texture render targets; FSQ pipeline; `fullscreen_triangle_mesh`.

7) Resource Pools + Hot/Cold Split
- Expected result: Opaque `Handle<T>` with generational reuse; minimal hot metadata.
- Expected client API:
  - Same API; resource creation returns `Handle<T>`.
- Implementables:
  - Typed pools, freelist + generation; hot/cold structs.

8) Draw List Optimization
- Expected result: Fewer PSO/material switches; sorted draws.
- Expected client API:
  - Optional user sort key; default internal order.
- Implementables:
  - Build sort keys; reorder before encoding; expose state-change counters.

