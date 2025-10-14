# Align Renderer With TALK: 2-Pass Occlusion + CPU Visibility

### Scope

- Add a depth-only prepass and build a HiZ depth pyramid each frame.
- Use last frame’s HiZ to perform conservative CPU occlusion culling after frustum culling.
- Keep overlay/composition unchanged; integrate via feature flag and metrics.

### Files To Touch

- `Pill-Engine/engine/pill_renderer/src/resources/renderer_pipeline.rs` (add depth-only pipeline + layout)
- `Pill-Engine/engine/pill_renderer/src/renderer.rs` (insert prepass; build HiZ; visibility stage; drive main pass survivors)
- `Pill-Engine/engine/pill_renderer/src/visibility/` (new: `hiz.rs`, `occlusion.rs` helpers)

### Design

- Depth prepass: render opaque geometry with a depth-only pipeline (no color, depth write enabled).
- HiZ pyramid: allocate mip-chain texture for depth; generate with compute (preferred) or render downsampling. Store previous frame’s HiZ for CPU queries.
- CPU occlusion: project entity world AABB to screen-space bounds; query HiZ at appropriate mip level using conservative test; mark occluded entities as culled.
- Frame cadence: use frame N-1 HiZ to cull frame N (avoids GPU→CPU sync); update HiZ after main pass for frame N+1.

### Integration Points

- Before main pass in `renderer.rs`:
1) run depth prepass
2) dispatch HiZ generation
3) frustum cull → occlusion cull → build `visible` list
4) draw main pass using `visible`
- Add `Renderer` toggles: `enable_depth_prepass`, `enable_hiz_culling` (config/env).
- Add counters: total, frustum_kept, occluded, drawn; expose in logs.

### Minimal API/Resource Changes

- Add depth-only `RenderPipeline` to `RendererPipeline` (e.g., `depth_prepass_pipeline`).
- Add persistent `hiz_texture` with mips; `hiz_bind_group` + compute pipeline for downsampling.
- Store per-mesh AABB (or compute at load); per-entity world AABB is derived via transform.

### Risks/Notes

- One-frame-late occlusion may pop on camera cuts; mitigate via camera-move tolerance or disable on large camera deltas.
- Keep culling conservative (AABB expanded slightly) to avoid over-culling.