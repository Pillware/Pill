# PassTonemap — gamma

No `pow(color, 1/2.2)` in the shader. The swapchain view is always an `*Srgb` format;
the GPU applies the sRGB OETF on fragment write automatically.

## Why a separate `swapchain_view_format`

Desktop: surface format is already `Bgra8UnormSrgb` / `Rgba8UnormSrgb` — view = surface format.

WebGPU/wasm32: canvas API only exposes `bgra8unorm` / `rgba8unorm` (no sRGB surface variant).
Fix: register the sRGB variant in `SurfaceConfiguration::view_formats`, build the pipeline
against it, and create the swapchain `TextureView` with it. The GPU still applies the OETF.

Same pattern used by Bevy (`bevy_render/src/view/window/mod.rs`):
```rust
let texture_view_format = if !format.is_srgb() { Some(format.add_srgb_suffix()) } else { None };
```

## References

- wgpu `TextureFormat` docs — `*Srgb` variants: *"Apply sRGB transfer function for color-space conversion"* on write. https://docs.rs/wgpu/latest/wgpu/enum.TextureFormat.html
- wgpu wiki — *"When image format names contain the suffix `*_SRGB`, the sRGB gamma transform is applied transparently on every texel write."* https://github.com/gfx-rs/wgpu/wiki/Texture-Color-Formats-and-Srgb-conversions
