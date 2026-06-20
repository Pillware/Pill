# Linux Wayland Layered Rendering

This document describes the multi-layer rendering architecture used on Linux/Wayland.
It is **Linux-only** — all relevant code is gated behind `#[cfg(target_os = "linux")]`.

---

## Layer Stack (bottom → top)

```
┌─────────────────────────────────────────────────────┐
│  5. Green HTML card  (WebKit overlay webview)        │  ← GtkOverlay child
│  4. React app        (WebKit base webview, transp.)  │  ← GtkOverlay base
│  3. wl_subsurface – wgpu small   100×50  @ (400,125) │  ← place_below(parent)
│  2. wl_subsurface – wgpu large  200×150  @ (300,225) │  ← place_below(parent)
│  1. wl_subsurface – shm orange  500×350  @ (200,225) │  ← place_below(parent)
└─────────────────────────────────────────────────────┘
              ↑ all five live inside the single GTK wl_surface (the "parent")
```

Layers 1–3 are Wayland `wl_subsurface` objects parented to the main GTK window's
`wl_surface`. They are composited by the Wayland compositor, **not** by WebKit or
GTK, so they are true hardware layers with no CSS/DOM involvement.

---

## Layer Details

### Layer 1 — shm background (orange)

| Property | Value |
|---|---|
| Source | `linux_surface_utilities::create_shm_subsurface` |
| Pixel format | `WL_SHM_FORMAT_ARGB8888` filled with `0xFFFF_A500` (orange) |
| Size | 500 × 350 px |
| Position | (200, 225) — centred in an 800 × 600 window |
| Z-order | `wl_subsurface::place_below(parent)` → always the bottommost layer |
| Buffer lifetime | `/dev/shm` file created, `fd` mapped, file unlinked immediately; the `wl_buffer` holds a reference to the mapping |
| Thread | main thread (one-shot, no loop) |

The buffer is static — it is committed once and never redrawn.  The Wayland
compositor keeps the pixels on screen as long as the `wl_surface` exists.

### Layers 2 & 3 — wgpu render surfaces

| Property | Large | Small |
|---|---|---|
| Source | `linux_surface_utilities::create_wgpu_subsurface` | same |
| Size | 200 × 150 px | 100 × 50 px |
| Position | (300, 225) | (400, 125) |
| Z-order | `place_below(parent)` | `place_below(parent)` |
| Clear colour | green (`0.0, 0.6, 0.0`) | green (`0.0, 0.6, 0.0`) |
| Triangle | red, NDC ±0.8 | red, NDC ±0.8 |
| Thread | dedicated `wgpu-orange` thread | dedicated `wgpu-orange` thread |
| Frame rate | ~60 fps (16 ms sleep) | ~60 fps (16 ms sleep) |

`create_wgpu_subsurface` returns a `WgpuSurface { surface: isize, display: isize }`
containing the raw `wl_surface*` and `wl_display*` pointers cast to `isize` for
`Send`-safe transfer to the render thread.

**Z-order insertion rule**: `place_below(parent)` inserts the subsurface *just
below the parent* in the compositor's stacking list. Each subsequent call pushes
the previously placed surface down, so the last-created subsurface ends up
directly below the parent and the first-created ends up at the bottom.  The shm
layer is created first, so it settles at the very bottom.

### Layer 4 — React webview (transparent)

The main Tauri webview. `"transparent": true` is set in `tauri.conf.json` and
the WebKit background is cleared to fully transparent via:

- `src/App.css`: `:root, html, body, #root { background: transparent !important; }`
- `index.html`: inline `style="background: transparent"` on `<html>`, `<body>`,
  and `<div id="root">` (prevents the UA-default white flash before CSS loads)

React content that does not paint a pixel lets the wgpu surfaces below show through.

### Layer 5 — Green HTML card (WebKit overlay webview)

A second WebviewWindow added via `win.add_child(WebviewBuilder::new("green-overlay", …))`.
It is a full-window transparent webview whose HTML/CSS draws a centred green card:

```html
<!-- public/green.html -->
<div style="position:fixed; width:350px; height:250px;
            top:50%; left:50%; transform:translate(-50%,-50%);
            background:rgba(0,180,0,0.8)">
  Green Overlay — HTML layer rendered by WebKit
</div>
```

#### GtkOverlay restructuring

Tauri's `add_child` appends webviews sequentially into a `GtkBox` (vertical
stack), which makes them tile top-to-bottom instead of overlapping.  The setup
code in `lib.rs` restructures the widget tree at startup to fix this:

```
Before (GtkBox children):
  [0] React webview   → occupies top half of window
  [1] green webview   → occupies bottom half of window

After (GtkOverlay):
  GtkOverlay
  ├── base:    React webview  (fills window)
  └── overlay: green webview  (fills window, drawn on top)
```

Code path (executed once in `.setup()`):

```rust
let gtk_overlay = gtk::Overlay::new();
gtk_overlay.add(&base);           // React — base layer
gtk_overlay.add_overlay(&top);    // green — drawn on top
top.set_halign(gtk::Align::Fill);
top.set_valign(gtk::Align::Fill);
root_box.pack_start(&gtk_overlay, true, true, 0);
gtk_win.show_all();
```

---

## Key Implementation Details

### Raw Wayland FFI

All Wayland calls in `linux_surface_utilities.rs` go directly through `libwayland-client.so`
via `#[link(name = "wayland-client")]` extern blocks.  No Rust `wayland-client`
crate is needed at runtime — GTK already loads the library; the `#[link]`
attribute only tells the Rust linker to link it.

Protocol globals (`wl_compositor`, `wl_subcompositor`, `wl_shm`) are obtained by
calling `wl_display_get_registry` and waiting for the `global` event with
`wl_display_roundtrip`.  Each call to `create_shm_subsurface` or
`create_wgpu_subsurface` creates its own temporary registry, binds the globals it
needs, then destroys the registry object — the bound globals themselves are
intentionally leaked so the surfaces remain valid.

### wgpu backend selection

```rust
wgpu::Instance::new(wgpu::InstanceDescriptor {
    backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
    ..Default::default()
})
```

The system uses the GL/EGL backend (Mesa).  Two important constraints apply:

- `required_limits: adapter.limits()` — the GL backend reports
  `max_compute_workgroups = 0`; the default `Limits::default()` exceeds this and
  causes `request_device` to fail.
- **No vertex buffer** — the triangle shader uses `@builtin(vertex_index)` instead
  of a vertex attribute.  Mesa's naga GLSL translation has a known path where
  `glVertexAttribPointer` state can be silently misconfigured, causing draw calls
  to produce no output even though the clear colour renders correctly.  Hardcoding
  positions via `vertex_index` / `gl_VertexID` avoids this entirely.

### Subsurface synchronisation

All `wl_subsurface` objects are **synchronous** (the default).  Position and
z-order changes take effect when the parent surface (`wl_surface::commit`) is
called, which happens once at creation time in the setup code.  The wgpu render
thread then drives its own commits through `frame.present()` independently.

The shm subsurface is static and never commits again after creation.

### Cargo dependencies (Linux-specific)

```toml
[dependencies]
wgpu    = "0.20"        # rwh 0.6 — matches Tauri 2
pollster = "0.3"        # block_on for wgpu async init on the render thread
bytemuck = { version = "1", features = ["derive"] }  # (currently unused; kept for future vertex buffers)

[target.'cfg(target_os = "linux")'.dependencies]
raw-window-handle = "0.6"   # extract wl_surface* / wl_display* from Tauri window
gtk = "0.18"                # access gtk_window(), restructure GtkBox → GtkOverlay
```

`tauri` is built with `features = ["unstable"]` to enable `window.add_child()`.

---

## Adding a New Layer

### Solid-colour shm layer

```rust
linux_surface_utilities::create_shm_subsurface(
    parent_surface,
    display_ptr,
    0xFF_RRGGBB,   // ARGB colour
    width, height, // pixels
    x, y,          // position relative to parent origin
    parent_surface, // pass parent to place below everything else
)?;
```

Pass a previously returned `wl_surface*` (cast to `*mut c_void`) as the last
argument if you need finer-grained z-order control.

### wgpu render layer

```rust
let s = linux_surface_utilities::create_wgpu_subsurface(
    parent_surface, display_ptr,
    x, y, width, height,
)?;
orange_renderer::spawn_orange_renderer(s.surface, s.display, width, height);
```

`spawn_orange_renderer` starts a dedicated thread running a `pollster::block_on`
wgpu render loop at ~60 fps.

---

## Limitations & Known Issues

- **Linux/Wayland only.** The entire layer system is compiled out on other
  platforms (`#[cfg(target_os = "linux")]`).  On X11 or macOS/Windows the app
  runs normally without the extra layers.
- **No XDG output scaling.** Subsurface positions and sizes are in logical pixels
  (surface coordinates).  On HiDPI displays (`wl_output.scale > 1`) the
  compositor scales automatically, but you may want to query the scale factor and
  multiply dimensions accordingly.
- **Static shm buffer.** The orange background is drawn once.  Resizing the
  window does not resize or repaint the shm layer.
- **wgpu render loop does not respond to resize.** The `SurfaceConfiguration` is
  fixed at creation time.  A `configure` call with updated dimensions would be
  needed to handle window resize.
- **Wayland objects are intentionally leaked.** Destroying a `wl_surface` removes
  it from the screen.  For a production app, store the handles and clean them up
  on window close.
