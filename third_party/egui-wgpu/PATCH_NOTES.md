# Pill Engine local `egui-wgpu` patch

This directory contains the normalized crates.io package for `egui-wgpu`
0.35.0, patched temporarily to compile against wgpu 30.

Upstream source: <https://github.com/emilk/egui/tree/0.35.0/crates/egui-wgpu>

Upstream commit: `f72eaf6be1d137b2f568f7c21d4569ab6304b2b4`

Imported: 2026-07-17

The local compatibility changes are intentionally limited to:

- upgrading the `wgpu` dependency from 29 to 30;
- supplying wgpu 30's adapter limit-bucket option and handling its new adapter
  diagnostics fields;
- wrapping the renderer's vertex-buffer layout in `Some`;
- handling mapped-buffer access as a `Result` in screenshot capture; and
- presenting surface textures through `Queue::present`.

The upstream shaders are unchanged. The patched crate passes:

```text
cargo check -p egui-wgpu --all-features
```

on Windows with Rust 1.95.0. Pill consumes only the renderer integration and
does not enable this crate's optional `winit`/`capture` integration.

Remove this directory and restore the registry dependency when an official
`egui-wgpu` release supports wgpu 30.
