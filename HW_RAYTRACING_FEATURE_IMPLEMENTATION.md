# Hardware Ray Tracing Feature Implementation Plan

Status: design proposal; implementation has not started.

Immediate executable baseline: Pill Engine native renderer, `wgpu` 27.0.1, hardware inline ray queries.

Evaluated upgrade target: `wgpu` 30.0.x, adopted at Phase 0 only after the renderer/debug-UI dependency gate in Section 2.4 passes.

Last reviewed: 2026-07-17.

## 1. Executive decision

Implement hardware ray tracing as an optional, renderer-owned **inline ray-query** capability with an unconditional raster fallback. The first production feature should be ray-traced hard shadows in the existing forward-lit path. It should not begin as a full path tracer.

This wording matters: both `wgpu` 27 and 30 expose BLAS/TLAS acceleration structures and ray queries callable from ordinary compute or fragment shaders. Neither exposes a usable safe high-level ray-generation/miss/closest-hit pipeline, shader binding table, or ray-tracing pass. Pill should name the Cargo feature `hardware_ray_tracing`, but its public capability should say `hardware_ray_query` so the API does not promise functionality that the selected `wgpu` version cannot provide.

The implementation must satisfy these decisions:

- Preserve the existing raster renderer and all native, web, and headless builds.
- Compile the experimental code only for native `pill_renderer`; never enable it in `pill_web`.
- Default runtime policy to `Off` while the upstream API is experimental.
- Support `Off`, `Prefer`, and `Require` device policies.
- Treat native Vulkan as the supported v1 backend. Always check the advertised feature and limits as the technical authority; do not infer support from the backend name alone.
- Keep all `wgpu::Blas`, `wgpu::Tlas`, bind groups, buffers, and unsafe experimental opt-in code inside `pill_renderer`.
- Expose only backend-neutral capabilities and frame data through `pill_engine::PillRenderer`.
- Own one static BLAS per unique renderer mesh and one dynamic TLAS for the active rendered scene.
- Use a 24-bit ray instance ID to index renderer metadata. Do not pack engine entity, mesh, or material generational handles into `TlasInstance::custom_data`.
- Batch pending BLAS builds before the TLAS build in the existing per-frame command encoder.
- Start with opaque indexed triangles. Defer alpha-tested traversal, procedural geometry, skinned/deforming meshes, and BLAS refit.
- Isolate all version-sensitive WGSL and experimental API usage so a later `wgpu` upgrade has a small blast radius.
- Re-evaluate the objective `wgpu` 30 adoption gate immediately before Phase 0. If it passes, migrate directly from 25 to 30 rather than implementing RT on 27 and immediately migrating again.
- Never request `EXPERIMENTAL_RAY_TRACING_PIPELINES` for this feature. Its presence in wgpu 30 is not a safe public pipeline API.

## 2. What the supplied sources establish

### 2.1 `wgpu` 27 release

The [`wgpu` 27 release notes](https://github.com/gfx-rs/wgpu/releases/tag/v27.0.0) make two integration changes relevant to Pill:

1. The old acceleration-structure feature was merged into `Features::EXPERIMENTAL_RAY_QUERY`. Request that one feature; do not look for `EXPERIMENTAL_RAY_TRACING_ACCELERATION_STRUCTURE`.
2. Experimental features require an explicit unsafe acknowledgement through `DeviceDescriptor::experimental_features`.

The unsafe call does not make the rest of Pill unsafe Rust. It acknowledges that the upstream feature may have validation gaps, major bugs, and breaking changes. Put it in one reviewed device-creation helper and document why its preconditions are considered satisfied.

`wgpu` 27's acceleration-structure limits default to zero. The RT-enabled device request must start from:

```rust
wgpu::Limits::default()
    .using_minimum_supported_acceleration_structure_values()
```

Then validate the selected adapter's actual limits and Pill's configured scene capacity before creating the device or any acceleration structure.

### 2.2 Ray-tracing API specification

The [tagged v27 specification](https://github.com/gfx-rs/wgpu/blob/v27.0.0/docs/api-specs/ray_tracing.md) is the implementation contract for the immediate lane. If the v30 gate passes, replace it with the [version-pinned v30.0.0 specification](https://github.com/gfx-rs/wgpu/blob/v30.0.0/docs/api-specs/ray_tracing.md) as the sole contract for that implementation. The [trunk specification](https://github.com/gfx-rs/wgpu/blob/trunk/docs/api-specs/ray_tracing.md) is useful for lifecycle and future direction, but is a living document and is not a release source of truth.

The reusable rules are:

- A BLAS must be built in the same acceleration-structure build call as a TLAS that first references it, or in an earlier build call.
- If a referenced BLAS is rebuilt or replaced, every dependent TLAS becomes dirty and must be rebuilt.
- `TlasInstance::custom_data` has only 24 usable bits.
- The instance visibility mask has 8 bits.
- TLAS transforms are row-major affine 3x4 arrays.
- Acceleration-structure creation capacity constrains later builds; TLAS growth requires recreation.
- Static BLAS compaction is asynchronous and replacement-based. A compacted BLAS cannot itself be rebuilt or compacted again.
- Correct ray-query state transitions matter. Accessing candidate or committed data in the wrong traversal state can be undefined behavior.
- Ray-tracing pipelines are described as future work; Naga's usable v27 path is inline ray queries.

Version-sensitive shader syntax must be pinned. The official [v27 sample shader](https://github.com/gfx-rs/wgpu/blob/v27.0.0/examples/features/src/ray_traced_triangle/shader.wgsl) does **not** use `enable wgpu_ray_query;`. That directive became mandatory in wgpu/Naga 28 and is present in the trunk documentation. A shader compiled against v27 must omit it. Add a shader canary test so a future dependency update fails clearly instead of producing a runtime surprise.

Even the tagged prose can lag Naga's exact v27 call syntax. Candidate mutation calls in v27 take the ray-query pointer, for example `rayQueryConfirmIntersection(&rq)`, `rayQueryGenerateIntersection(&rq, t)`, and `rayQueryTerminate(&rq)`. Treat the tagged official sample plus a compiled canary as stronger evidence than copying pseudocode from trunk.

### 2.3 Practical Zenn guide

The [Zenn hardware ray-tracing guide](https://zenn.dev/kokutoupan/articles/eefc517ac4210d?locale=en) and its [v0.1 source](https://github.com/kokutoupan/fast-raytracing-wgpu/tree/v0.1) are useful implementation references, but target `wgpu` 28, between the two evaluated Pill baselines. Re-derive every Rust descriptor from the selected version's tagged API.

Adopt these ideas:

- BLAS per unique mesh and TLAS per scene.
- `BLAS_INPUT` on geometry buffers.
- Batched scene metadata for hit reconstruction.
- A renderer-owned global vertex/index table for advanced hit shading.
- Transpose a glam matrix before extracting the row-major 3x4 TLAS transform.
- Use primitive index and barycentrics to reconstruct hit attributes when full hit shading is added.

Adapt these ideas:

- Use a single 24-bit metadata-table index in `custom_data`. The guide's `(mesh_id << 16) | material_id` convention leaves only eight bits for the mesh ID because bits 24-31 must be zero.
- Treat `[f32; 4]` vertex fields as a storage-layout technique, not a BLAS requirement. A baseline BLAS can consume `Float32x3` positions with the declared stride.
- Batch all pending BLAS entries and the TLAS entry in one `build_acceleration_structures` call when possible, allowing scratch storage to be reused.
- Keep the first Pill consumer hybrid and one-bounce. The guide's path-traced renderer needs material, light, texture-indexing, accumulation, invalidation, and denoising systems that Pill does not yet have.

Do not copy these assumptions:

- Fixed scene capacity with no add/remove/growth policy.
- A single `rayQueryProceed` pattern generalized to non-opaque geometry.
- Path-tracing accumulation reset only on camera movement.
- Wgpu 28 Rust descriptor shapes copied into either selected baseline. Its ray-query enable directive is valid only for the v30 lane, not v27.
- Vulkan availability inferred without feature and limit checks.

### 2.4 `wgpu` 30 investigation and baseline decision

[`wgpu` 30.0.0](https://github.com/gfx-rs/wgpu/releases/tag/v30.0.0) was released on 2026-07-01. It is a worthwhile direct target when Pill's full renderer dependency stack can use it, but it does **not** justify expanding the V1 feature into a path tracer or adding more supported backends.

The version-pinned [`wgpu` 30.0.0 ray-tracing specification](https://github.com/gfx-rs/wgpu/blob/v30.0.0/docs/api-specs/ray_tracing.md) and [30.0.0 feature documentation](https://docs.rs/wgpu/30.0.0/wgpu/struct.Features.html) establish the following:

| Area | `wgpu` 30 finding | Consequence for Pill |
| --- | --- | --- |
| Public query support | `EXPERIMENTAL_RAY_QUERY` is still experimental, native-only, and officially supported on Vulkan | Keep Vulkan as the V1 certified backend and preserve raster fallback on DX12, Metal, GLES, web, and headless |
| Full RT pipelines | `EXPERIMENTAL_RAY_TRACING_PIPELINES` exists and Naga gained SPIR-V output work, but safe `wgpu` exposes no ray-pipeline descriptor/creation method, SBT, trace pass, or dispatch API | Do not request the feature or use `as_hal`; keep inline queries in fragment/compute shaders |
| WGSL | `enable wgpu_ray_query;` is mandatory from wgpu 28 onward | A v30 shader must contain the directive; a v27 shader must omit it; compile a canary for the selected baseline |
| Geometry | V30 adds procedural AABB BLAS creation/build descriptors, making the already-defined AABB candidate/generated-intersection query path usable | Keep opaque triangles in V1; add AABBs only with a separately designed intersection contract and tests |
| Hit vertex return | The optional feature and WGSL extension remain Vulkan-only | Do not require them for hard shadows; gate any later use independently |
| Updates | Standard safe backends still perform full builds; `Tlas::lowest_unmodified` is for custom backends | Keep the existing dirty graph and full TLAS rebuild model; do not promise refit performance |
| Limits and validation | V30 adds `max_buffers_and_acceleration_structures_per_shader_stage` and an acceleration-structure build error for offsets limited to 4 GiB | Validate combined binding pressure and keep geometry build offsets below 4 GiB or split future arenas |
| Instance policy | `STRICT_WEBGPU_COMPLIANCE` excludes native experimental extensions | Never enable that instance flag on a native RT candidate; web remains a separate raster-only path |
| Backend work | Naga/Metal gained ray-query correctness work | Treat this as implementation progress, not public Metal support; advertised features plus Pill's certified matrix remain authoritative |

The blocking ecosystem fact is [`egui-wgpu` 0.35.0](https://docs.rs/crate/egui-wgpu/0.35.0), the newest released integration at this review, depends on `wgpu` 29 rather than 30. Pill's `pill_runtime` always enables `pill_renderer/debug_ui`, and `EguiDrawer` passes the renderer's `wgpu::Device`, `Queue`, texture views, and render pass directly to `egui-wgpu`. Cargo can resolve two wgpu majors, but their GPU types are nominally incompatible, so that is not a working integration.

The baseline decision is therefore conditional but objective:

1. **Keep 27.0.1 as the immediately executable baseline** with the [compatible egui 0.33.3 stack](https://raw.githubusercontent.com/emilk/egui/0.33.3/Cargo.toml) while the v30 UI gate is closed.
2. **Prefer a direct 25-to-30 migration before RT-specific implementation** if, at Phase 0 start, an official `egui-wgpu` release uses the same wgpu 30 major and the complete raster matrix passes.
3. Do not close the gap by silently disabling debug UI, running two devices, or accepting two incompatible wgpu type universes. A temporary egui fork requires an explicit maintenance decision outside this feature plan.
4. Do not select wgpu 29 merely as an intermediate stepping stone. It is a valid ecosystem-aligned alternative, but it adds another experimental-API migration without resolving the requested v30 target.

The v30 gate passes only when all of these are true:

- An official released `egui-wgpu` uses wgpu 30, and `egui`, `egui-wgpu`, and `egui-winit` can be upgraded as one compatible set.
- Cargo resolves one wgpu API generation for `pill_renderer` and its debug UI.
- The chosen v30 patch release passes Pill's native, web, debug-UI, hot-reload, headless, and example raster matrix in an RT-free migration change.
- A minimal query/BLAS/TLAS probe passes validation on every certified Vulkan vendor/driver target.
- The effective MSRV of the complete dependency set is declared and tested. [`wgpu` 30 itself records MSRV 1.87](https://docs.rs/crate/wgpu/30.0.0), while the [egui 0.35 workspace manifest](https://raw.githubusercontent.com/emilk/egui/0.35.0/Cargo.toml) declares Rust 1.92, so wgpu's MSRV alone is not sufficient evidence.

Once selected, pin all specifications, examples, shader syntax, and canaries to that one baseline. Do not combine v27 Rust calls with trunk/v30 shader snippets.

## 3. Current Pill Engine constraints

The plan is intentionally shaped around the repository as it exists:

- `engine/pill_renderer/Cargo.toml` uses `wgpu = "25.0.0"` for native and web and `egui-wgpu = "0.32.1"`.
- `State::new` in `engine/pill_renderer/src/renderer.rs` requests one surface-compatible adapter, silently intersects optional profiling/depth features, and requests `Limits::default()`.
- `PillRenderer` in `engine/pill_engine/src/graphics/renderer.rs` is renderer-neutral, but `render` receives a raster sort queue plus full camera/transform component storages rather than an explicit frame description.
- `RenderQueueItem` contains only a packed sort key and entity index. `MeshDrawer` reconstructs renderer handles from bitfields in that key.
- `RendererMesh` creates vertex and index buffers with only `VERTEX` and `INDEX` usage. It stores no BLAS state or geometry capacity metadata.
- `MeshData` is immutable after resource creation, which is a good fit for static `PREFER_FAST_TRACE` BLASes.
- The renderer already uses one encoder and one queue submission per frame, providing a clean place to record BLAS/TLAS builds before consuming passes.
- The default lit shader uses a hard-coded point light; there is no canonical light frame data or PBR material table.
- Material texture updates are incomplete, and material/shader destruction does not fully repair dependent resources. The shadow MVP can avoid mirroring material data, but these dependencies must be fixed before persistent RT material tables are introduced.
- Engine shaders are currently vertex/fragment pairs compiled through the HLSL-to-WGSL asset rule. The asset rule has no compute stage and the shader resource always creates a render pipeline.
- `.gitignore` ignores `*.wgsl`, so any authored renderer-internal WGSL needs a targeted exception.
- `TransformComponent` has cached matrices and a dirty flag, but the rendering system does not call `update_transform_matrices`; that function also does not currently clear the dirty flag. There are multiple rotation helpers with differing multiplication orders. This must be corrected before TLAS transforms can be trusted.
- A scene has a 16-bit component mask. Adding a required ray-tracing component would consume scarce component capacity.
- Engine/runtime hot reload recreates the renderer and world; game-only reload preserves renderer resources. RT ownership must follow those same rules.
- `DummyRenderer` and WASM must remain valid raster/no-op implementations.

These are not reasons to push ray-tracing types into the engine. They are reasons to improve the frame boundary and keep the experimental implementation behind it.

## 4. Scope

### 4.1 V1 deliverable

The v1 feature is complete when Pill can:

- start on supported native hardware with ray queries enabled;
- build one BLAS per unique opaque triangle mesh;
- share that BLAS across multiple transformed instances;
- build and update an active-scene TLAS;
- cast a hardware shadow ray from the default lit fragment path;
- add, move, remove, and reuse mesh instances without stale acceleration-structure references;
- grow TLAS capacity within device limits;
- report the selected adapter, backend, requested policy, enabled state, limits, and fallback reason;
- run unchanged on unsupported native hardware, web, and headless configurations when policy is `Prefer` or `Off`;
- fail early with a precise diagnostic when policy is `Require` and the feature cannot be enabled.

### 4.2 Explicit non-goals for V1

- Ray-generation, miss, closest-hit, any-hit pipelines, or shader binding tables.
- A production path tracer, multiple bounces, denoising, or temporal upscaling.
- Procedural AABB geometry.
- Skinned, morph-target, particle, or otherwise deforming BLAS geometry.
- BLAS update/refit; the standard safe path performs full builds in both evaluated baselines even though update-related names exist.
- Alpha-tested or transparent candidate-intersection traversal.
- Hardware RT on WebGPU/WASM, Metal, or GLES.
- Shipping support for a backend not documented as supported by the pinned `wgpu` release.
- Bindless texture/material parity with arbitrary custom raster shaders.
- Physics, visibility, or gameplay queries through the render TLAS.
- Async-compute scheduling or raw `wgpu-hal` integration.
- BLAS compaction in the first correctness milestone.

## 5. Proposed architecture

```text
Game/ECS
  rendering_system
       |
       | builds backend-neutral RenderFrame
       v
pill_engine::PillRenderer
  capabilities()       render(&RenderFrame)
       |                         |
       v                         v
DummyRenderer             pill_renderer::Renderer
  RT unavailable            Raster state (always)
                            RayTracingState
                              Disabled(reason)
                              Enabled(RayTracingScene)
                                      |
                +---------------------+---------------------+
                |                     |                     |
          Mesh BLAS cache       Active-scene TLAS     RT shader state
          pending builds        instance metadata      shadow/debug pass
                |                     |                     |
                +---------- existing frame encoder --------+
                                      |
                               one queue submission
```

### 5.1 Compile-time boundary

Add a native renderer feature:

```toml
[features]
hardware_ray_tracing = []
```

Enable it on the `pill_renderer` dependency in `pill_runtime`, where the native renderer is assembled. Do not enable it from `pill_web`. Guard the implementation with both `feature = "hardware_ray_tracing"` and `not(target_arch = "wasm32")`.

The compile-time feature means "the binary contains the implementation." The runtime policy still decides whether the device requests the experimental feature. This distinction keeps one native binary usable on both RT and non-RT machines.

### 5.2 Runtime state

Avoid scattered `Option` checks. Model the renderer state explicitly:

```rust
enum RayTracingState {
    Disabled(RayTracingDisabledReason),
    Enabled(RayTracingScene),
}
```

`RayTracingScene` owns all acceleration-structure and query-pipeline state. Raster resources remain unconditional.

Suggested renderer module layout:

```text
engine/pill_renderer/src/ray_tracing/
  mod.rs
  capability.rs
  scene.rs
  blas.rs
  tlas.rs
  instance_table.rs
  pipeline.rs
  transform.rs
  shaders/
    ray_query_canary.wgsl
    shadow_query.wgsl
```

The exact split may be reduced during implementation, but device negotiation, AS lifecycle, metadata allocation, and shaders should not be folded into the already-large `renderer.rs`.

## 6. Configuration and public capability API

### 6.1 Device policy

Add a string/enum getter to `EngineConfig`; do not encode modes as integers. Parse this key case-insensitively:

```ini
RAY_TRACING_MODE=off   # off | prefer | require
```

Semantics:

| Mode | Device request | Unsupported result |
| --- | --- | --- |
| `Off` | Do not request experimental RT features or AS limits | Continue with raster |
| `Prefer` | Select a supported surface-compatible adapter and request RT | Continue with raster and one structured warning |
| `Require` | Select and request RT or fail before renderer initialization completes | Return an actionable error |

Default to `Off`. A future stable upstream API and a mature hardware CI matrix may justify changing the default to `Prefer`; that is not part of this plan.

Add a string setter as well as a getter so tests, launch code, and embedded users do not need to round-trip through INI text. The resolved device policy is immutable after device creation; changing `Off` to `Prefer` or `Require` at runtime requires full renderer/device recreation.

Optional capacity settings can be added only when measured use cases require them:

```ini
MAX_RT_INSTANCES=16384
```

Do not expose BLAS scratch sizes or driver-specific knobs in the game config.

### 6.2 Renderer-neutral capabilities

Add types under `pill_engine::graphics`, without importing `wgpu`:

```rust
pub struct RendererCapabilities {
    pub backend: RendererBackend,
    pub adapter_name: String,
    pub hardware_ray_query: Option<HardwareRayQueryCapabilities>,
}

pub struct HardwareRayQueryCapabilities {
    pub max_blas_primitive_count: u32,
    pub max_blas_geometry_count: u32,
    pub max_tlas_instance_count: u32,
    pub max_acceleration_structures_per_shader_stage: u32,
    // Present on the v30 lane; None on v27.
    pub max_buffers_and_acceleration_structures_per_shader_stage: Option<u32>,
}
```

Add `fn capabilities(&self) -> &RendererCapabilities` to `PillRenderer` and a read-only `Engine::renderer_capabilities()` accessor. `DummyRenderer` reports a headless backend and no ray-query capability.

Capabilities report what was actually enabled on the created device, not merely what the adapter advertised. Keep unrequested adapter features, such as vertex return in V1, in private startup diagnostics rather than presenting them as enabled capability. If vertex return is enabled in a later phase, expose an explicitly named `vertex_return_enabled` field then. Games can use capabilities to choose presentation or quality settings, but should not need them to remain correct.

### 6.3 Per-object policy

Extend `MeshRenderingComponent` with a small backend-neutral visibility description rather than creating a new ECS component:

```rust
pub struct RayVisibility {
    pub ray_visible: bool,
    pub casts_shadow: bool,
    pub mask: u8,
    pub opacity: RayOpacityMode,
}

pub enum RayOpacityMode {
    Auto,
    ForceOpaque,
    Exclude,
}
```

Defaults: ray-visible, casts shadows, mask `0xff`, opacity `Auto`. `ray_visible` is independent of raster visibility and means that the instance may participate in renderer ray queries. V1 inserts an instance into its shadow TLAS only when it is ray-visible, casts shadows, has a nonzero mask, resolves to opaque, and has an eligible BLAS. `casts_shadow = false` omits it from the V1 TLAS.

Opacity resolution must be deterministic:

- `Auto` is opaque only for the exact built-in shader/resource classes registered as opaque when default resources are created; do not classify by a mutable display name.
- `ForceOpaque` is an explicit game-author guarantee that alpha/discard behavior may be ignored and the geometry can be marked `OPAQUE`.
- `Exclude` never enters the V1 TLAS.

The instance mask is copied unchanged. Each `RenderLight` supplies a `shadow_cull_mask` (default `0xff`), and the shader passes that value to the ray descriptor; an instance participates only when the two masks overlap. Test overlapping and non-overlapping masks. Future reflection/visibility queries must define their own receiver/query mask source or separate TLASes.

Until materials have a canonical alpha mode, custom/transparent materials resolve to excluded under `Auto` and require the explicit `ForceOpaque` contract; never guess and silently trace them as opaque.

## 7. Dependency migration gate

Upgrade and stabilize the raster renderer before adding RT code. At Phase 0 start, record the result of the Section 2.4 v30 gate and select exactly one lane:

| Lane | Coherent dependency set | Selection rule |
| --- | --- | --- |
| Immediate | `wgpu` 27.0.1 plus `egui`, `egui-wgpu`, and `egui-winit` 0.33.3 | Use while no official egui integration targets wgpu 30 |
| Direct v30 | One chosen `wgpu` 30.0.x patch plus an official egui integration built against that same major | Prefer if the v30 gate passes before RT-specific work starts |

Then:

1. Change native and WASM `wgpu` dependencies from 25 to the selected version in the same migration change.
2. Upgrade `egui`, `egui-wgpu`, and `egui-winit` together; do not use a different wgpu major transitively.
3. Verify `cargo tree` resolves one compatible `wgpu` API generation across the renderer and debug UI.
4. Declare the effective MSRV for the complete selected dependency graph and add an MSRV check. The v27 lane requires Rust 1.88; wgpu 30 itself requires 1.87, but the eventual compatible UI stack may require more.
5. Pin the dependency set reproducibly. Because the repository currently ignores `*.lock`, either add a deliberate exception for the engine/launcher lockfile or use exact compatible crate versions and a documented update procedure.
6. Fix all descriptor, pipeline, surface, and shader-language changes with no RT feature requested.
7. Build native debug/release/hot-reload, WASM, headless, and every existing example before proceeding.

This should be a standalone change/PR. It separates migration regressions from ray-tracing validation failures and gives a clean rollback point.

### 7.1 Additional direct 25-to-30 migration checklist

If the v30 lane is selected, audit the cumulative v28-v30 changes rather than treating the release as an RT-only bump. At minimum:

- Adapt instance creation to the new descriptor constructors/display-handle flow, and await adapter enumeration.
- Handle `Surface::get_current_texture()` as `CurrentSurfaceTexture`, including `Success`, `Suboptimal`, `Timeout`, `Occluded`, `Outdated`, `Lost`, and `Validation` according to the tagged contract instead of converting every result into a fatal error.
- Add `SurfaceConfiguration::color_space`, initially `SurfaceColorSpace::Auto` to preserve current output.
- Replace `surface_texture.present()` with `queue.present(surface_texture)`.
- Add `multiview_mask` to render-pass descriptors and preserve `None` for the current renderer.
- Add the required `depth_slice` field to render-pass color attachments and preserve `None` for ordinary 2D targets.
- Update pipeline-layout optional bind-group slots and `immediate_size`; do not accidentally renumber the fixed bind-group contract.
- Wrap vertex layouts in `Some` for `VertexState::buffers`.
- Migrate depth/stencil optional fields and prove the existing depth-clip feature contract remains valid.
- Use the separate mipmap filter mode in texture sampler descriptors.
- Add explicit `@interpolate(flat)` to every integer inter-stage shader value.
- Rename any compute dispatch calls to `dispatch_workgroups*` if a compute canary or later pass introduces them.
- Handle mapped-range APIs as `Result` values in profiler/readback paths instead of relying on a panic.
- Compile every cooked/default shader through the selected Naga version, not just the new ray-query shader.

Keep `RequestAdapterOptions::apply_limit_buckets` false for the RT selection path so diagnostics and capacity checks see the actual acceleration-structure limits. Do not add `InstanceFlags::STRICT_WEBGPU_COMPLIANCE` to the native `Prefer`/`Require` instance because it deliberately excludes non-WebGPU experimental features.

## 8. Adapter and device negotiation

Refactor `State::new` into testable selection and request steps.

### 8.1 Candidate evaluation

For every candidate adapter allowed by `WGPU_BACKENDS`:

1. Require presentation support for the current surface.
2. Record adapter name, device type, driver, backend, features, and limits.
3. For RT candidacy, require `EXPERIMENTAL_RAY_QUERY` and all requested AS limits.
4. Apply the product support allowlist. For v1 that is Vulkan, even if an undocumented backend happens to advertise the bit.
5. Rank valid RT candidates using the requested power preference and device type.
6. If none qualify, execute the `Prefer`/`Require` policy instead of silently intersecting the feature away.

The public feature documentation for both evaluated baselines lists ray queries as native-only and Vulkan-supported. Their implementations contain evolving backend work, so feature probing must remain authoritative at the API level, while Pill's tested support matrix remains deliberately narrower. Add other backends only after an integration test and CI/hardware run prove the pinned version.

Respect explicit backend restrictions. For example, `RAY_TRACING_MODE=require` plus `WGPU_BACKENDS=METAL` should fail with "policy requires hardware ray queries, but the requested backend set cannot provide them," not silently override the user's backend choice.

### 8.2 Device request

Build three separate values:

- baseline required features;
- optional profiling/depth features that may be intersected;
- mandatory RT features when the selected policy resolved to enabled.

Never place mandatory RT features in the current optional-intersection path.

Also repair the existing depth-feature contract during this refactor: raster pipelines must not set `unclipped_depth: true` unless `DEPTH_CLIP_CONTROL` was actually enabled, or the feature must become mandatory. The current optional intersection and unconditional pipeline state are inconsistent.

For the RT device request:

```rust
let required_features = baseline | wgpu::Features::EXPERIMENTAL_RAY_QUERY;
let required_limits = wgpu::Limits::default()
    .using_minimum_supported_acceleration_structure_values();
let experimental_features = unsafe {
    // SAFETY: isolated opt-in to the pinned experimental ray-query API.
    // All AS descriptors, lifetimes, build ordering, and shader state are
    // validated by RayTracingScene and covered by GPU validation tests.
    wgpu::ExperimentalFeatures::enabled()
};
```

Do not request `EXPERIMENTAL_RAY_HIT_VERTEX_RETURN`, `EXTENDED_ACCELERATION_STRUCTURE_VERTEX_FORMATS`, or `ACCELERATION_STRUCTURE_BINDING_ARRAY` for v1. Each broadens the validation and shader surface without helping one global TLAS and opaque shadow rays.

Validate and log these AS limits explicitly:

- `max_blas_primitive_count`;
- `max_blas_geometry_count`;
- `max_tlas_instance_count`;
- `max_acceleration_structures_per_shader_stage`.

On the v30 lane, also validate `max_buffers_and_acceleration_structures_per_shader_stage` against the complete shader layout, not just the TLAS count. Requesting the RT feature must not push storage, uniform, vertex-buffer, and acceleration-structure bindings beyond their shared backend table. Leave adapter limit bucketing disabled so these checks use the reported hardware limits.

In `Prefer`, a device-request failure attributable to unsupported experimental capability may retry once with the raster descriptor. In `Require`, return the original structured error. Validation bugs, OOM, and device loss after initialization are not normal "unsupported" fallbacks and must not be silently swallowed.

## 9. Frame boundary refactor

Introduce a renderer-neutral frame description instead of extending the already raster-specific `render` argument list repeatedly:

```rust
pub struct RenderFrame<'a> {
    pub camera: RenderCamera,
    pub instances: &'a [RenderInstance],
    pub lights: &'a [RenderLight],
    pub delta_time: f32,
    // Existing UI/timing hooks remain feature-gated.
}

pub struct RenderCamera {
    pub entity: EntityHandle,
    pub renderer_handle: RendererCameraHandle,
    pub world_position: Vector3f,
    pub view: Matrix4f,
    pub projection: Matrix4f,
    pub view_projection: Matrix4f,
    pub inverse_view: Matrix4f,
    pub inverse_projection: Matrix4f,
    pub clear_color: Vector3f,
    pub fog_density: f32,
    pub fog_color: Vector3f,
}

pub struct RenderInstance {
    pub entity: EntityHandle,
    pub mesh: RendererMeshHandle,
    pub material: RendererMaterialHandle,
    pub shader: RendererShaderHandle,
    pub raster_sort_key: u64,
    pub model: Matrix4f,
    pub normal: Matrix3fA,
    pub ray_visibility: RayVisibility,
}
```

Keep `raster_sort_key` only for ordering raster draws. Stop reconstructing handles from its packed fields in `MeshDrawer`; use the explicit handles. This avoids coupling TLAS metadata to the sort-key bit allocation and its small handle fields.

The frame packet must be self-sufficient. The renderer should not need the camera or transform component storages to turn `camera.entity` back into data. Extract camera pose, matrices, clear/fog values, and renderer handle in `rendering_system`; the renderer may upload or cache those values, but may not reach back into ECS storage. Compute the inverse matrices once with explicit singular/error handling because later camera-ray generation needs them.

Build the RT scene from all ray-visible active-scene instances, not from a future camera-frustum-culled list. Off-screen objects can still cast shadows or appear in reflections.

`RenderLight` can initially hold the point light currently hard-coded in `default_lit_fragment.hlsl`. Moving that light into frame data is a prerequisite for a meaningful shadow ray and creates the correct seam for future lighting work.

## 10. Transform correctness gate

Before creating a TLAS, make one transform implementation canonical for raster and ray tracing.

Required cleanup:

1. Define the engine's model convention in one helper: translation, Euler rotation order, scale, handedness, and degree/radian conversion.
2. Make `rendering_system` update dirty cached matrices once per frame and clear `matrix_update_required`.
3. Derive the normal matrix as the inverse-transpose of the model matrix's upper-left 3x3, then normalize the transformed normal. The current rotation-only calculation is incorrect under non-uniform scale.
4. Make raster instances consume that canonical model/normal result instead of re-encoding position, rotation, and scale through an independent path where possible.
5. Convert the canonical column-major glam matrix to the row-major affine `[f32; 12]` expected by `TlasInstance` in one renderer helper.
6. Reject or quarantine non-finite and singular transforms from the TLAS. A zero-scale/near-zero-determinant instance may remain a degenerate raster draw under `Prefer`, but cannot participate in ray queries; `Require` reports it. Negative scale is allowed only after front-face and normal orientation behavior is tested.

The conversion should follow the official sample's intent:

```rust
let row_major = model.transpose().to_cols_array();
let tlas_transform: [f32; 12] = row_major[..12].try_into().unwrap();
```

Unit tests must compare raster-transformed reference vertices against the same vertices transformed by the reconstructed TLAS 3x4 matrix for identity, translation, each axis rotation, combined rotation, non-uniform scale, negative scale, and large/small coordinates. They must also compare inverse-transpose normal results for non-uniform and negative scale and assert the defined rejection path for singular matrices.

## 11. BLAS ownership and lifecycle

Extend `RendererMesh`, not the engine `Mesh`, with optional RT state:

```rust
struct RayTracingMesh {
    blas: wgpu::Blas,
    size: wgpu::BlasTriangleGeometrySizeDescriptor,
    build_state: BlasBuildState,
    primitive_count: u32,
    vertex_count: u32,
    index_count: u32,
}

enum RendererMeshRayTracingState {
    RasterOnly(RayTracingMeshError),
    Eligible(RayTracingMesh),
}

enum BlasBuildState {
    Pending,
    Encoded { frame_epoch: u64 },
    Submitted { submission: SubmissionSerial },
    Failed(RayTracingMeshError),
    Retiring { after: SubmissionSerial },
    // Compaction states are added in a later phase.
}
```

`Encoded` is not synonymous with built: an encoder can be abandoned or fail to finish. A BLAS encoded in the same `build_acceleration_structures` call may be consumed by that call's TLAS, but persistent state advances to `Submitted` only after the command buffer is submitted and receives a serial. Queue ordering makes it usable by later submissions; completion is still required before compaction or retirement. If encoding/finish/submission fails, restore `Pending` when retry is safe or record `Failed` with the original validation/device error.

`SubmissionSerial` is a renderer-owned monotonic wrapper associated with the `wgpu::SubmissionIndex`/completion callback. It gives the BLAS, metadata allocator, and retirement queue one testable ordering vocabulary without exposing `wgpu` outside `pill_renderer`.

### 11.1 Geometry buffers

When RT is enabled, create mesh buffers with:

```text
vertex: VERTEX | BLAS_INPUT
index:  INDEX  | BLAS_INPUT
```

Pill's `MeshVertex` starts with a three-float position and has a known stride, so it can describe a baseline `Float32x3` position stream without enabling extended vertex formats. Confirm the selected version's restriction that the BLAS position starts at the provided buffer offset; retain a separate RT position buffer if future vertex layouts place position elsewhere.

Do not add `STORAGE` solely for the shadow MVP. Shadow occlusion needs no hit attributes. For later reflections/path tracing, create a renderer-owned, explicitly aligned global geometry arena rather than binding an arbitrary per-mesh buffer after a hit.

### 11.2 Descriptor validation

Validate before creating the BLAS:

- vertices and indices are non-empty when required;
- the index count is divisible by three;
- every index is in range;
- positions are finite;
- offset, stride, index format, and counts match the creation size descriptor;
- primitive and geometry counts fit the enabled device limits;
- V1 geometry is indexed triangles and marked `OPAQUE`.

Construct the size descriptor through the selected version's tagged API. On v30 this is explicitly `BlasGeometrySizeDescriptors::Triangles { descriptors }`; reject `AABBs` in V1 even though the API can create them. On that lane, validate the chosen position format against `device.features().allowed_vertex_formats_for_blas()` as well as enforcing the baseline `Float32x3` policy. Also validate every acceleration-structure input offset against the backend's 4 GiB build-offset constraint and diagnose the v30 `BuildAccelerationStructureError::OffsetLimitedTo4GB` condition with the mesh/arena range. Today's per-mesh buffers naturally begin near offset zero; a future global geometry arena must split/chunk allocations before this becomes a runtime build failure.

Use `PREFER_FAST_TRACE` for immutable Pill meshes. Do not add `ALLOW_UPDATE` in V1; the standard safe path in both evaluated versions performs full builds even though update-related names exist.

Represent ineligible geometry explicitly as `RasterOnly(reason)` instead of leaving a half-created BLAS. Under `Prefer`, the mesh may continue to rasterize and a ray-visible instance is omitted with a deduplicated diagnostic. Under `Require`, creation or first ray-visible use returns a precise error. This degradation must be observable because an omitted occluder produces incomplete shadows.

### 11.3 Deferred build

`create_mesh` should allocate buffers and a BLAS descriptor, then queue the mesh as `Pending`. Do not create a private command encoder and submit per mesh.

On the v30 lane, keep creation and build geometry variants paired: a BLAS created from `BlasGeometrySizeDescriptors::Triangles` must be built with `BlasGeometries::TriangleGeometries`. The build call consumes iterators of references to `BlasBuildEntry` values and TLAS values; prepare stable per-frame entry storage rather than references to short-lived temporaries. Treat a size/build variant mismatch as a renderer bug before encoding.

At frame start:

1. Gather all pending BLAS build entries needed by this frame.
2. Gather TLAS instances, including those pending BLAS references.
3. Call `build_acceleration_structures(pending_blas, tlas)` once so BLAS work is ordered before the TLAS.
4. Record the consuming render/compute pass after the build call in the same encoder.
5. Submit once.

This integrates with the existing renderer's command flow and avoids unnecessary synchronization.

### 11.4 Destruction and replacement

Resource deletion must follow dependency order:

1. Remove every TLAS instance that references the mesh BLAS.
2. Mark/rebuild the TLAS without that reference before it is used again.
3. Retire the old BLAS and its geometry buffers only after submitted GPU work that references them has completed.

Use a renderer retirement queue keyed to submission completion rather than relying on immediate Rust drops as a lifecycle policy. Game-only hot reload keeps renderer resources, so generation-safe lookup and deletion behavior must remain correct across a game dylib swap.

The current engine order must be changed explicitly: `Mesh::destroy` presently calls `renderer.destroy_mesh` before detaching all `MeshRenderingComponent` users. Make engine resource teardown detach/invalidate those components first, then call the renderer. As a second line of defense, `Renderer::destroy_mesh` should tombstone the renderer handle, remove its reverse BLAS-to-instance references from the next TLAS snapshot, and defer the actual BLAS/buffer drop through the retirement queue. Add `engine/pill_engine/src/resources/mesh.rs` to the Phase 2 change set and test destruction between frame extraction and submission.

## 12. TLAS and instance metadata

### 12.1 Ownership and capacity

V1 can own one TLAS for the active scene because the current engine renders one active scene. Rebuild it when:

- instance membership changes;
- a model transform changes;
- mask or opaque/visibility policy changes;
- a referenced BLAS is built, rebuilt, compacted, or replaced;
- the active scene changes.

Do not rebuild it for camera-only changes.

Create the V1 TLAS with `AccelerationStructureUpdateMode::Build` and initially prefer fast build because moving rigid instances can make TLAS rebuilds frequent; benchmark `PREFER_FAST_BUILD` against `PREFER_FAST_TRACE` on representative static and dynamic scenes before freezing the flag policy.

Allocate capacity using a bounded growth policy such as the next power of two, capped by both `MAX_RT_INSTANCES` and `max_tlas_instance_count`. A TLAS has fixed maximum capacity; growth creates a new TLAS and recreates bind groups that reference it. Handle zero instances without creating invalid descriptors.

The shared RT-enabled group-0 bind group requires a valid, already-built TLAS even when the raster shader does not query it. Bootstrap a minimum-capacity TLAS and encode an empty build before the first `set_bind_group`. When the active scene becomes empty, rebuild the TLAS with every instance slot set to `None` so stale geometry is removed while the binding remains valid. On growth, build the replacement before binding it and retire the old TLAS plus bind group only after their last submission completes.

Maintain stable ordering where practical, but correctness must not depend on `instance_index` remaining stable across frames.

### 12.2 24-bit ID indirection

Allocate a bounded, generation-tracked `RtInstanceId` table slot in `0..(1 << 24)` and put the slot index in `custom_data`. Store a parallel GPU metadata record:

```rust
#[repr(C)]
struct GpuRtInstance {
    mesh_metadata_index: u32,
    material_metadata_index: u32,
    entity_debug_id: u32,
    flags: u32,
}
```

For the shadow MVP, most of this table is optional, but establishing the ID convention now prevents a breaking change later. Validate generations on the CPU before writing a record. Never expose stale slot-map indices to the shader.

Do not immediately reuse a freed ID or overwrite its metadata slot. An in-flight submission may still resolve the old `custom_data` value. Retire IDs and metadata ranges against the last submission that referenced them, then return them to the allocator only from the submission-completion path. Capacity/exhaustion tests must cover rapid remove/recreate cycles with multiple frames in flight.

Standardize the 8-bit instance mask as ray visibility layers. Reserve `0xff` for "all layers" and document any future layer allocation centrally.

### 12.3 Dirty graph

Track cause-specific revisions rather than hashing all GPU resources blindly:

```text
mesh geometry create/change -> BLAS dirty -> all referencing TLASes dirty
mesh removal/replacement     -> TLAS membership dirty + resource retirement
transform/mask change        -> TLAS dirty only
camera change                -> shadow pass data dirty, TLAS unchanged
light change                 -> light data dirty, TLAS unchanged
surface resize               -> output/history resources dirty, AS unchanged
```

Mesh geometry is currently immutable, so most frames should perform no BLAS builds. A scene with moving rigid instances performs a TLAS build only.

## 13. Shader and render-pass integration

### 13.1 Binding convention prerequisite

The current renderer assumes conceptual bind groups 0-3 but only pushes layouts that exist, which can shift later groups. Fix this before adding a ray-scene binding. Keep the contract within the four baseline groups:

```text
group 0: engine/frame, extended with TLAS and later RT metadata on an RT device
group 1: camera
group 2: material
group 3: textures
```

Use fixed compatible layouts/placeholders or a declarative pipeline-layout builder. RT and raster variants must agree on every shared group.

On the v30 lane, express the fixed slots through `PipelineLayoutDescriptor`'s `&[Option<&BindGroupLayout>]` form and set `immediate_size` explicitly. Optional slots are not permission to compact the vector and shift later groups.

For V1, the RT-enabled group-0 layout adds a TLAS entry using `BindingType::AccelerationStructure { vertex_return: false }`, visible only to stages that execute the query. This avoids turning `max_bind_groups >= 5` into another RT hardware requirement. Recreate the group-0 bind group when TLAS capacity growth replaces the bound TLAS; recreate pipelines only if the layout itself changes.

### 13.2 Shader source policy

Create the first query canary as renderer-internal, hand-authored WGSL pinned to the baseline chosen in Phase 0. This avoids pretending the current vertex/fragment-only HLSL cooking rule supports an experimental WGSL extension.

Add a targeted `.gitignore` exception for the renderer's authored shader directory. Compile the canary during a test or renderer initialization under an error scope.

Before the production shadow variant, run a small spike through the existing Slang HLSL pipeline. If Slang can reliably emit ray-query WGSL for the selected baseline, extend the cooker with an explicit ray-query profile and golden output test. Otherwise keep RT variants as authored WGSL until the shader resource system supports renderer-internal modules cleanly.

Pin the extension line explicitly:

| Selected baseline | Required ray-query WGSL prologue |
| --- | --- |
| wgpu 27.0.1 | Omit `enable wgpu_ray_query;` |
| wgpu 30.0.x | Include `enable wgpu_ray_query;` |

Do not attempt one source string that happens to parse under both. The canary must fail if the dependency and shader dialect diverge.

### 13.3 First consumer: hard shadows

Add a ray-enabled variant of the default lit pipeline. Its fragment stage:

1. Receives world position and an inverse-transpose-correct geometric world normal from the raster vertex stage; keep any normal-mapped shading normal separate.
2. Reads the point-light position from frame/light data.
3. Uses the preliminary surface-to-light vector only to reject a near-zero light distance and skip direct lighting on the non-lit side under the chosen one-sided material convention.
4. Offsets the ray origin along the normalized geometric normal by a scene-scale-aware bias. Do not use the perturbed normal-map normal for the offset.
5. From that final offset origin, recomputes `to_light = light_position - origin`, `distance = length(to_light)`, and `direction = to_light / distance`, with a second near-zero guard.
6. Sets `t_min` above zero and `t_max = distance - endpoint_bias`. Do not mix a world-distance `t_max` with an unnormalized direction.
7. Uses `RenderLight::shadow_cull_mask` as the ray cull mask.
8. Uses opaque/terminate-on-first-hit flags.
9. Runs the complete query state sequence for the selected version and reads the committed result only after traversal is complete.
10. Multiplies direct illumination by a binary visibility term.

Before `rayQueryInitialize`, the shared helper must reject a non-finite origin or direction, a zero-length direction, a negative/non-finite `t_min`, and a non-finite `t_max` or `t_max < t_min`. Construct ray flags in that helper so mutually exclusive skip, face-cull, and opacity modes cannot be combined accidentally. Invalid light/frame inputs take a deterministic no-shadow/fallback path and increment a diagnostic counter instead of invoking the spec's implementation-dependent invalid-ray behavior.

Keep the original raster pipeline variant available. Select the RT variant only when the device enabled ray queries, the TLAS is valid, and the material/shader declares compatibility. Custom game shaders remain raster unless they opt into a future RT shader contract.

Centralize query traversal in one reviewed WGSL helper. Even though V1 marks everything opaque, write the control flow so adding candidate confirmation later cannot accidentally read committed/candidate data in an invalid state.

### 13.4 Debug path

Before lighting integration, implement a minimal hit/miss or hit-distance output, preferably as a small native example and validation test. It should prove:

- feature and limits are enabled;
- BLAS/TLAS build ordering is correct;
- the TLAS binding is usable;
- camera ray and transform conventions agree;
- moving an instance changes the result after a TLAS-only rebuild.

This is a diagnostic milestone, not a second renderer architecture.

## 14. Later hit shading and path-tracing preparation

Full reflections or path tracing need data that a ray intersection does not automatically provide. Add it only after the shadow vertical slice is stable:

- a global RT position/normal/UV/tangent arena;
- a `GpuRtMesh` table with base vertex/index offsets and geometry ranges;
- canonical material records independent of arbitrary raster shader parameters;
- a texture atlas or a validated binding-array strategy;
- explicit light records;
- accumulation/history textures and sample counters;
- invalidation revisions for camera, transforms, geometry, materials, lights, resize, shader reload, and settings;
- tone mapping and optional denoising.

Prefer a duplicate, tightly packed RT geometry arena first. Pill's current 56-byte interleaved Rust vertex does not map naturally to a normal WGSL storage struct because `vec3` fields have stricter alignment. A raw-word decoder is possible, but an explicit aligned GPU format is easier to validate and evolve.

Do not require `EXPERIMENTAL_RAY_HIT_VERTEX_RETURN` for the baseline. It has narrower hardware support and ties BLAS flags and shader bindings to another experimental feature. Reconstruct attributes from the geometry table first; benchmark vertex return as a separate optional path later.

## 15. Compaction and dynamic geometry

### 15.1 Static BLAS compaction

Add compaction only after correctness and memory telemetry exist. The state machine must include:

```text
PendingBuild -> Built -> CompactionRequested -> Ready
             -> Compacting -> Compacted
```

Flow:

1. Recreate eligible Phase 2 BLASes with `PREFER_FAST_TRACE | ALLOW_COMPACTION`; the flag cannot be added to an existing BLAS in place. Build replacements incrementally, switch all dependent TLAS instances, rebuild the TLAS, and retire the original BLASes. Do not make Phase 2 pay an unmeasured compaction-capable allocation cost.
2. Build and submit.
3. Call `prepare_compaction_async` and continue rendering.
4. Poll readiness without blocking the frame.
5. Call `Queue::compact_blas`.
6. Replace the BLAS in every `TlasInstance`.
7. Rebuild affected TLASes.
8. Retire the original after GPU completion.

A rebuild cancels pending compaction. A compacted BLAS is independent and cannot be rebuilt or compacted again. Measure memory saved and frame-time cost before enabling this by default.

### 15.2 Dynamic/deforming geometry

Pill meshes are currently immutable, so V1 has no deforming path. When one is introduced:

- mark it explicitly, never infer from update frequency;
- use `PREFER_FAST_BUILD`;
- rebuild the BLAS, because refit/update is not implemented in the evaluated standard safe paths;
- dirty all referencing TLASes;
- apply per-frame build and memory budgets;
- fall back to raster when the budget or feature policy disallows it.

## 16. Failure handling and diagnostics

Define structured disable/failure reasons, for example:

- compile-time feature absent;
- target unsupported;
- backend outside support matrix;
- no surface-compatible adapter;
- feature bit absent;
- required AS limit too small;
- device request rejected;
- mesh geometry invalid;
- configured TLAS capacity exceeds limit;
- shader validation failed;
- AS build validation failed.

Log one startup record with:

```text
policy, compiled support, target, adapter, backend,
wgpu version, strict-compliance flag, limit-bucketing state,
advertised feature, enabled feature, AS and combined limits, final mode, fallback reason
```

Use the selected version's scoped-error API around experimental shader/pipeline and AS initialization in validation builds and tests, plus an uncaptured-error handler. In v30, use `let scope = device.push_error_scope(...); ...; scope.pop().await` rather than calling the removed `Device::pop_error_scope`. Include resource labels, mesh handles/generations, primitive counts, and active-scene revision in errors.

Policy rules:

- `Off`: no warning for being disabled.
- `Prefer`: one warning when falling back; no per-frame spam.
- `Require`: fail before the game starts, with remediation such as selecting Vulkan, updating the driver, or changing the config.
- A validation error after RT initialization is a bug, not an availability result. Disable the affected experimental path only if doing so is proven safe; otherwise recreate/fail the renderer visibly.
- OOM and device loss require renderer recovery or termination. They are never converted into an unexplained raster fallback.

## 17. Resize, scene switch, and hot reload

- Surface resize does not invalidate BLAS/TLAS. It does invalidate any RT output, accumulation, or history textures and dependent bind groups.
- Active-scene switch dirties/replaces TLAS membership but can reuse mesh BLASes stored in renderer resource storage.
- Game-only hot reload preserves the renderer, so RT mesh resources and scene revision reconciliation must remain valid.
- Engine/runtime hot reload tears down and recreates the renderer. Drop TLAS before its BLAS references, drain/retire submitted work safely, then recreate RT state from engine resources as the normal renderer initialization path does.
- Shader reload must recreate RT pipeline/bind groups as needed. A path-traced accumulator must reset; the shadow MVP has no temporal history.
- Device loss requires full renderer/AS recreation. Surface loss alone follows the existing surface recovery path and should not rebuild acceleration structures.
- On the v30 lane, classify every `CurrentSurfaceTexture` outcome deliberately: render/present `Success`; render or discard `Suboptimal` and schedule reconfiguration; skip `Timeout`/`Occluded`; reconfigure on `Outdated`; recreate then configure the surface on `Lost`; and surface `Validation` as an actionable renderer error. Present acquired frames through `Queue::present`. None of these surface outcomes dirties BLAS/TLAS unless device loss is reported separately.
- Current `State::new` discards the `wgpu::Instance` after creating the surface and does not store the window target. Phase 0 must either retain the instance plus an owned/`Arc` window target so `Lost` can recreate the surface in place, or return a structured signal that makes the host recreate the full renderer. Merely configuring the lost surface is not a valid recovery path.

## 18. File-level change map

| Area | Planned changes |
| --- | --- |
| `engine/pill_renderer/Cargo.toml` | Upgrade wgpu; upgrade matching egui stack; add `hardware_ray_tracing` feature |
| `engine/pill_runtime/Cargo.toml` | Enable native renderer RT implementation |
| `engine/pill_engine/src/app_config.rs` | Add string/enum parsing for `RAY_TRACING_MODE` |
| `engine/pill_engine/src/graphics/renderer.rs` | Add capability query and `RenderFrame`/explicit instance boundary |
| `engine/pill_engine/src/graphics/dummy_renderer.rs` | Report unavailable capability and accept new frame API safely |
| `engine/pill_engine/src/graphics/render_queue.rs` | Keep sort key, add/use explicit renderer handles or replace with `RenderInstance` |
| `engine/pill_engine/src/ecs/systems/rendering_system.rs` | Canonical transform update; build complete backend-neutral frame instances/lights |
| `engine/pill_engine/src/ecs/components/mesh_rendering_component.rs` | Add defaulted ray visibility/mask/casts-shadow/opacity policy |
| `engine/pill_engine/src/ecs/components/transform_component.rs` | Unify matrix convention, clear dirty state, add tests |
| `engine/pill_engine/src/resources/mesh.rs` | Detach component users before renderer mesh destruction |
| `engine/pill_renderer/src/renderer.rs` | Refactor instance/adapter/device negotiation and v30 surface acquire/present handling; integrate RT scene build before consuming pass |
| `engine/pill_renderer/src/profiler.rs` | Handle v30 mapped-range results and preserve profiling/readback error reporting |
| `engine/pill_renderer/src/resources/renderer_mesh.rs` | Add conditional `BLAS_INPUT`, counts, descriptor, BLAS/build state |
| `engine/pill_renderer/src/resources/renderer_texture.rs` | Apply the selected sampler/mipmap-filter descriptor API |
| `engine/pill_renderer/src/drawers/mesh_drawer.rs` | Consume explicit frame handles/transforms; add RT pipeline variant selection |
| `engine/pill_renderer/src/drawers/*` | Migrate every render-pass attachment/descriptor field, including depth slice and multiview mask on v30 |
| `engine/pill_renderer/src/resources/renderer_shader.rs` | Stabilize bind-group layout indices and RT-compatible variant layout |
| `engine/pill_renderer/src/ray_tracing/*` | New capability, BLAS/TLAS, instance table, shader, and lifecycle implementation |
| `engine/pill_assets/src/rules/hlsl_to_wgsl.rs` | Optional later RT shader profile only if Slang output for the selected baseline is validated |
| `.gitignore` | Targeted exception for authored renderer RT WGSL |
| `examples/hardware_raytracing/*` | Native diagnostic example with static/shared/moving instances and fallback display |
| `.github/workflows/ci.yml` | Add pure tests and raster fallback builds; optional RT hardware job |

Do not combine every row into one change. The phases below are intended to remain reviewable and revertible.

## 19. Implementation phases and exit criteria

### Phase 0 - baseline selection, wgpu migration, and prerequisites

Work:

- Record the Section 2.4 gate result, select one baseline, and upgrade wgpu plus the compatible egui stack in one RT-free change.
- Restore native, WASM, debug UI, hot reload, and headless builds.
- Fix fixed bind-group layout indexing.
- Fix and test canonical transforms.
- Add `EngineConfig` string/enum access.

Exit criteria:

- No experimental RT feature is requested.
- Existing examples render as before.
- Native release/hot-reload, WASM, and headless builds pass.
- `cargo fmt` and clippy are clean through the launcher workflow.
- Matrix tests cover all agreed transform cases.
- The selected dependency graph contains one wgpu API generation, has a declared/tested effective MSRV, and records exact version pins.

### Phase 1 - capability and policy layer

Work:

- Add Cargo/runtime gates.
- Add adapter enumeration/ranking, feature/limit validation, and unsafe experimental token.
- Add `Off`/`Prefer`/`Require` behavior.
- Expose renderer capabilities and structured diagnostics.
- Add WGSL canary compilation for the selected baseline, including the correct ray-query extension prologue.

Exit criteria:

- `Off` never requests ray-query features.
- `Prefer` selects a valid tested adapter or falls back once with a reason.
- `Require` fails deterministically on unsupported configurations.
- Web/headless capabilities report unavailable and remain buildable.
- No `wgpu` type crosses into `pill_engine`.

### Phase 2 - frame boundary and acceleration-structure scene

Work:

- Introduce explicit `RenderFrame`/`RenderInstance` data.
- Stop decoding handles from the raster sort key.
- Add conditional BLAS buffers/state to `RendererMesh`.
- Add batched pending BLAS builds, TLAS instance table, growth, rebuild, and retirement.
- Add native hit/miss diagnostic output.

Exit criteria:

- One mesh BLAS is shared by multiple instances.
- Static frames perform no redundant BLAS builds.
- Transform-only motion rebuilds TLAS, not BLAS.
- Mesh add/remove/reuse and active-scene changes produce no stale hits or validation errors.
- BLAS state changes to submitted/usable only after queue submission; abandoned encoders remain retryable.
- Instance IDs and metadata slots are not reused while an older submission is in flight.
- Capacity growth recreates TLAS and its bind group safely.
- A GPU readback or image test proves hit, miss, and moved-instance behavior.

### Phase 3 - production hybrid hard shadows

Work:

- Move default point-light data into the frame/global lighting buffer.
- Add the stable RT-enabled group-0 frame binding and default-lit RT pipeline variant.
- Implement ray bias, masks, opaque traversal, and shadow visibility.
- Add runtime/debug UI status and comparison controls.

Exit criteria:

- Default-lit opaque objects cast correct hard shadows on supported hardware.
- Raster output remains the fallback for unsupported/custom shaders.
- Self-intersection, back-face/negative-scale, light-inside-geometry, and large-scene tests pass.
- Resize, game reload, runtime reload, and scene switch are validation-clean.

### Phase 4 - performance and lifecycle hardening

Work:

- Add CPU/GPU telemetry and build counters.
- Optimize dirty extraction and TLAS rebuild decisions.
- Implement deferred BLAS compaction behind a separate setting.
- Add memory reporting and resource retirement stress tests.

Exit criteria:

- Static scenes show zero AS build work after warm-up.
- Metrics identify BLAS build, TLAS build, shadow query, extraction, and AS memory independently.
- Compaction never leaves stale TLAS references and has measured benefit before default enablement.
- Large create/destroy cycles remain validation-clean and memory-stable.

### Phase 5 - advanced hit shading, optional

Work:

- Add geometry, mesh, canonical material, texture, and light tables.
- Add reflection/debug path-tracing modes.
- Add accumulation invalidation, tone mapping, and optional denoising.
- Evaluate vertex-return support separately.

Exit criteria:

- Attribute reconstruction is correct across multiple meshes/materials/geometries.
- All temporal invalidation causes are covered.
- Unsupported/custom materials have a documented fallback.
- The advanced path remains optional and does not weaken the Phase 3 fallback.

## 20. Verification strategy

### 20.1 Pure unit tests

- Config parsing, defaults, and invalid values.
- Adapter candidate ranking and policy resolution using synthetic capabilities.
- Required-limit comparison and capacity growth.
- Transform-to-row-major-3x4 conversion, raster equivalence, inverse-transpose normals, and singular rejection.
- Primitive-count/index validation and overflow checks.
- 24-bit ID allocation, exhaustion, deferred reuse, and generation validation across simulated in-flight submissions.
- Instance-mask semantics.
- `RayOpacityMode` resolution by stable built-in class and explicit override.
- BLAS/TLAS dirty propagation.
- Scene revision and temporal invalidation rules.
- Retirement queue ordering.
- BLAS state rollback for abandoned/failed encoding and transition on successful submission.

### 20.2 GPU validation tests

On a pinned RT-capable adapter:

- Build a single triangle BLAS and one-instance TLAS; verify hit and miss via readback.
- Build BLAS and TLAS in the same call.
- Share one BLAS across multiple transforms.
- Move an instance and verify TLAS-only update.
- Add/remove instances, destroy/recreate a mesh slot, and switch active scene.
- Destroy a mesh after frame extraction but before submission; verify tombstoning and deferred retirement prevent stale use.
- Exercise empty TLAS, TLAS growth, limit rejection, and bind-group replacement.
- Bind the bootstrapped empty TLAS, remove the final instance, and verify that the rebound empty scene has no stale hit.
- Verify overlapping and non-overlapping instance/light shadow masks.
- Exercise zero/non-finite direction inputs, invalid intervals, and every fixed flag combination through the pre-query guard; no invalid query may reach traversal.
- Validate all authored WGSL and RT/raster pipeline variants.
- Validate the selected shader dialect explicitly: v30's enabled module succeeds and a missing `enable wgpu_ray_query;` module fails; v27's directive-free module succeeds and an accidental later-dialect prologue fails.
- Run with error scopes and uncaptured-error capture; zero validation errors is required.
- Later, compact a BLAS, replace references, and verify identical hits.

Tests must skip with a clear capability reason when hardware is absent; a skipped RT test is not a passed hardware matrix job.

### 20.3 Raster/fallback regression matrix

Mandatory CI/build coverage:

| Target/backend | RT policy | Expected |
| --- | --- | --- |
| Native tested Vulkan RT GPU | `Require` | RT enabled; GPU tests run |
| Native unsupported adapter/software Vulkan | `Prefer` | Raster fallback with one reason |
| Native unsupported adapter | `Require` | Early actionable failure |
| Windows non-validated backend | `Prefer` | Raster unless separately certified |
| macOS/Metal | `Prefer` | Raster |
| WASM/WebGPU | `Prefer` or compile-time absent | Raster build/run |
| Headless/DummyRenderer | any non-required normal server config | No-op/raster-neutral capability |

Retain every existing example build. Add Windows and macOS compile jobs if available, plus an optional/self-hosted Vulkan RT execution job on at least one NVIDIA and one AMD driver stack over time.

### 20.4 Visual cases

The native example should include:

- a floor, a static occluder, and a rotating shared-mesh instance;
- translation, combined rotation, non-uniform scale, and negative scale;
- add/remove/recreate controls;
- visible adapter/capability/build-counter diagnostics;
- raster/RT shadow comparison where supported;
- deterministic camera/light presets for screenshot comparison.

Image tests need tolerances and vendor baselines; binary hit/miss readback remains the primary correctness oracle.

### 20.5 Repository verification commands

Build the launcher first, then use it whenever a real game/example must be linked into the workspace:

```text
cargo build --release --manifest-path engine/pill_launcher/Cargo.toml

<launcher-exe> -a cargo -p examples/floating_pills -- fmt
<launcher-exe> -a cargo -p examples/floating_pills -- clippy -- -D warnings
<launcher-exe> -a build -p examples/hardware_raytracing
<launcher-exe> -a build -t web -c release -p examples/cube

cargo test --manifest-path engine/Cargo.toml -p pill_engine
cargo test --manifest-path engine/Cargo.toml -p pill_renderer
cargo test --manifest-path engine/Cargo.toml -p pill_renderer --features hardware_ray_tracing
cargo tree --manifest-path engine/Cargo.toml -p pill_renderer -i wgpu
```

Run the native RT example on certified hardware with both `Prefer` and `Require`, and run the unsupported/fallback tests with the backend constrained deliberately. Review the dependency tree for duplicate wgpu majors and verify the debug-UI path specifically; a successful non-UI renderer build is not enough. Launcher runs rewrite `engine/Cargo.toml` and the linked game's `Cargo.toml`; diffs limited to those expected workspace-path rewrites are not feature changes and must be excluded exactly as CI already does.

## 21. Performance and memory gates

Instrument before optimizing. Record at least:

- CPU frame extraction and dirty detection time;
- BLAS count, primitive count, build count, and GPU build time;
- TLAS instance count, rebuild count, and GPU build time;
- ray-query shadow pass GPU time;
- uncompacted/compacted AS memory where the API/telemetry permits;
- geometry metadata/arena memory;
- retired-but-not-yet-reclaimed resource count and bytes.

Benchmark these scenarios:

- static scene after warm-up;
- many moving instances sharing a small set of BLASes;
- burst mesh creation/destruction;
- TLAS capacity growth boundary;
- large triangle-count static scene;
- RT enabled versus raster fallback on the same camera.

Do not invent a universal millisecond budget before measurements. Define per-target budgets from representative scenes, report p50/p95, and gate regressions relative to the approved baseline. The absolute correctness constraint is that a static scene schedules no BLAS or TLAS rebuild after warm-up unless membership, transform, mask, or referenced BLAS state changes.

## 22. Risks and mitigations

| Risk | Consequence | Mitigation/rollback |
| --- | --- | --- |
| Experimental wgpu API changes or validation gaps | Breakage, driver crash, shader UB | Pin exact versions or deliberately track a lockfile; isolate module/unsafe token; shader canary; default `Off`; raster rollback |
| Official backend support is narrow/evolving | Works on one developer GPU only | Product allowlist plus feature/limit probe; vendor matrix; `Prefer` fallback |
| Selected wgpu/egui migration regresses raster behavior | Existing renderer breaks before RT work | Separate Phase 0 change and full native/web/debug-UI regression gate |
| No released `egui-wgpu` for wgpu 30 | Incompatible `Device`/`Queue`/render-pass types or pressure to disable UI | Keep v27 as implement-now lane; adopt v30 only through the objective same-major dependency gate; no implicit local fork |
| Fresh v30 release line has RT regressions | Validation errors or vendor-driver failures | Pin one exact release, run the vendor probe/matrix before selection, retain v27/raster rollback |
| Raster/TLAS transform mismatch | Visually displaced hits and self-shadowing | One canonical matrix path and golden conversion tests |
| Stale BLAS/TLAS references after deletion/reuse | Wrong hits or validation/device failure | Generation validation, explicit dirty graph, dependency-ordered rebuild, deferred retirement |
| Excessive per-frame AS builds | Frame spikes | Static BLAS cache, TLAS-only rigid motion, dirty counters, performance gates |
| AS memory growth | OOM or poor residency | Capacity bounds, telemetry, later measured compaction, graceful raster fallback before device failure |
| Invalid ray-query state flow | Undefined shader behavior | Opaque-only MVP, one reviewed WGSL helper, shader validation and hit/miss tests |
| Arbitrary materials lack RT representation | Incorrect reflections/alpha | Shadows first; explicit alpha mode; canonical later material table; custom shader fallback |
| Bind-group layouts shift | Pipeline/bind mismatch | Fixed group contract before RT integration |
| Hot reload/device loss leaks or stales AS state | Crash after reload | Renderer ownership, teardown ordering, retirement, full recreation tests |
| Lack of hardware CI | Regressions discovered late | Pure policy/transform tests everywhere; optional then required self-hosted vendor RT jobs |

## 23. Definition of done for the hardware-ray-query feature

The feature is ready to advertise when all of the following are true:

- The Phase 0-selected wgpu/egui migration is independently green, uses one wgpu API generation, and records why v27 or v30 was selected.
- `Off`, `Prefer`, and `Require` semantics are documented and tested.
- The renderer reports actual enabled capability and precise fallback reasons.
- BLAS/TLAS creation, build ordering, transforms, add/remove, growth, destruction, and hot reload are validation-clean.
- The default lit shader has correct ray-traced hard shadows on the certified Vulkan hardware matrix.
- Raster output remains available without source changes on unsupported native, web, headless, and custom-shader paths.
- No experimental `wgpu` type or unsafe opt-in leaks through `pill_engine`'s renderer boundary.
- Static frames schedule no redundant acceleration-structure builds.
- CI includes shader/pure tests, existing raster builds, explicit unsupported fallback, and at least one executing RT hardware job before making `Prefer` a recommended user setting.
- Known limitations are published: inline queries only, native tested backend only, opaque static triangles, no deforming meshes, no full path tracer.

## 24. Recommended change series

Use this review order:

1. **Baseline decision plus coherent wgpu/egui migration** with raster parity only; choose v27 now or direct v30 after its gate, never both.
2. **Transform and frame-boundary cleanup** with no RT dependency in engine types.
3. **Capability policy and diagnostic probe** with no visible RT effect.
4. **BLAS/TLAS lifecycle plus readback/debug canary**.
5. **Default-lit hybrid hard shadows and native example**.
6. **Stress tests, profiling, retirement, and optional compaction**.
7. **Advanced material/geometry tables and reflection/path modes**, only if separately approved.

Every change must keep the raster path buildable and revertible. Avoid a long-lived branch that combines the dependency migration, renderer refactor, AS lifecycle, and a path tracer into one unreviewable unit.

## 25. Primary references

- [`wgpu` v27.0.0 release notes](https://github.com/gfx-rs/wgpu/releases/tag/v27.0.0)
- [`wgpu` v27 ray-tracing specification](https://github.com/gfx-rs/wgpu/blob/v27.0.0/docs/api-specs/ray_tracing.md)
- [`wgpu` v30.0.0 release notes](https://github.com/gfx-rs/wgpu/releases/tag/v30.0.0)
- [`wgpu` v30.0.0 ray-tracing specification](https://github.com/gfx-rs/wgpu/blob/v30.0.0/docs/api-specs/ray_tracing.md)
- [`wgpu` trunk ray-tracing specification](https://github.com/gfx-rs/wgpu/blob/trunk/docs/api-specs/ray_tracing.md)
- [`wgpu` 27.0.1 `Features` documentation](https://docs.rs/wgpu/27.0.1/wgpu/struct.Features.html)
- [`wgpu` 27.0.1 `Limits` documentation](https://docs.rs/wgpu/27.0.1/wgpu/struct.Limits.html)
- [`wgpu` 30.0.0 `Features` documentation](https://docs.rs/wgpu/30.0.0/wgpu/struct.Features.html)
- [`wgpu` 30.0.0 `Limits` documentation](https://docs.rs/wgpu/30.0.0/wgpu/struct.Limits.html)
- [`wgpu` 30.0.0 `Device` documentation](https://docs.rs/wgpu/30.0.0/wgpu/struct.Device.html)
- [Official v27 ray-traced triangle Rust example](https://github.com/gfx-rs/wgpu/blob/v27.0.0/examples/features/src/ray_traced_triangle/mod.rs)
- [Official v27 ray-traced triangle WGSL](https://github.com/gfx-rs/wgpu/blob/v27.0.0/examples/features/src/ray_traced_triangle/shader.wgsl)
- [Official wgpu v30.0.0 examples](https://github.com/gfx-rs/wgpu/tree/v30.0.0/examples)
- [Zenn: hardware ray tracing with Rust and wgpu](https://zenn.dev/kokutoupan/articles/eefc517ac4210d?locale=en)
- [Zenn guide source, tag v0.1](https://github.com/kokutoupan/fast-raytracing-wgpu/tree/v0.1)
- [`wgpu` v28 release note for the later WGSL `enable` directive](https://github.com/gfx-rs/wgpu/releases/tag/v28.0.0)
- [`wgpu` v29.0.0 cumulative migration notes](https://github.com/gfx-rs/wgpu/releases/tag/v29.0.0)
- [`egui-wgpu` 0.35.0 crate metadata](https://docs.rs/crate/egui-wgpu/0.35.0)
- [`egui` 0.33.3 workspace manifest (`wgpu` 27.0.1)](https://raw.githubusercontent.com/emilk/egui/0.33.3/Cargo.toml)
- [`egui` 0.35.0 workspace manifest and MSRV](https://raw.githubusercontent.com/emilk/egui/0.35.0/Cargo.toml)
