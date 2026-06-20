#![cfg(target_os = "linux")]
//! Raw Wayland FFI helpers for creating `wl_subsurface` layers parented to the
//! main Tauri/GTK window surface.
//!
//! Two public entry points:
//! - [`create_wgpu_subsurface`] — bare surface with no pixel buffer; wgpu renders into it.
//! - [`create_shm_subsurface`]  — solid-colour surface backed by a `/dev/shm` pixel buffer.
//!
//! Both surfaces are placed **below** the parent GTK `wl_surface` so that
//! WebKit (React) composites on top. The React layer must have a transparent
//! CSS background for the subsurfaces below to be visible.
//!
//! No Rust Wayland crate is needed — all calls go directly through
//! `libwayland-client.so`, which GTK already loads into the process.

use std::ffi::{c_int, c_void};
use std::os::raw::{c_char, c_uint};

// ---------------------------------------------------------------------------
// Opaque Wayland object aliases — these are opaque C structs managed by
// libwayland-client. We only hold pointers to them, so `c_void` is correct.
// ---------------------------------------------------------------------------
type WlDisplay       = c_void;
type WlRegistry      = c_void;
type WlCompositor    = c_void;
type WlSubcompositor = c_void;
type WlSurface       = c_void;
type WlSubsurface    = c_void;
type WlShm           = c_void;
type WlShmPool       = c_void;
type WlBuffer        = c_void;
type WlProxy         = c_void; // base type; every Wayland object is a wl_proxy*

// Minimal mirror of the C `wl_interface` struct from wayland-client.h.
// We only need its *address* when calling wl_proxy_marshal_constructor — the
// library reads the name field itself, so the rest can be zero/null.
#[repr(C)]
struct WlInterface {
    name:         *const c_char,
    version:      c_int,
    method_count: c_int,
    methods:      *const c_void,
    event_count:  c_int,
    events:       *const c_void,
}

// ---------------------------------------------------------------------------
// Protocol opcodes — index of the request within each interface's method list.
// Values come from the Wayland XML protocol; only those actually used here.
// ---------------------------------------------------------------------------
const WL_DISPLAY_GET_REGISTRY:         c_uint = 1; // wl_display::get_registry
const WL_REGISTRY_BIND:                c_uint = 0; // wl_registry::bind
const WL_COMPOSITOR_CREATE_SURFACE:    c_uint = 0; // wl_compositor::create_surface
const WL_SUBCOMPOSITOR_GET_SUBSURFACE: c_uint = 1; // wl_subcompositor::get_subsurface
const WL_SUBSURFACE_SET_POSITION:      c_uint = 1; // wl_subsurface::set_position
const WL_SUBSURFACE_PLACE_BELOW:       c_uint = 3; // wl_subsurface::place_below
const WL_SUBSURFACE_SET_DESYNC:        c_uint = 5; // wl_subsurface::set_desync
const WL_SUBSURFACE_DESTROY:           c_uint = 0; // wl_subsurface::destroy
const WL_SURFACE_ATTACH:               c_uint = 1; // wl_surface::attach
const WL_SURFACE_DAMAGE:               c_uint = 2; // wl_surface::damage
const WL_SURFACE_COMMIT:               c_uint = 6; // wl_surface::commit
const WL_SURFACE_DESTROY:              c_uint = 0; // wl_surface::destroy
const WL_SHM_CREATE_POOL:              c_uint = 0; // wl_shm::create_pool
const WL_SHM_POOL_CREATE_BUFFER:       c_uint = 0; // wl_shm_pool::create_buffer
const WL_SHM_POOL_DESTROY:             c_uint = 1; // wl_shm_pool::destroy
const WL_SHM_FORMAT_ARGB8888:          c_uint = 0; // pixel format: 32-bit ARGB little-endian

// ---------------------------------------------------------------------------
// libwayland-client FFI
//
// `wl_proxy_marshal` and friends are variadic C functions — the trailing `...`
// arguments are the request parameters, typed according to the protocol XML.
// libwayland-client.so is already loaded by GTK; `#[link]` only tells the
// Rust linker to pull in the right symbols at link time.
// ---------------------------------------------------------------------------
#[link(name = "wayland-client")]
extern "C" {
    // Send all pending requests and wait for the compositor to process them.
    fn wl_display_roundtrip(display: *mut WlDisplay) -> c_int;
    // Flush the outgoing request buffer without blocking.
    fn wl_display_flush(display: *mut WlDisplay) -> c_int;

    // Attach an event listener (vtable) to a proxy object.
    fn wl_proxy_add_listener(
        proxy: *mut WlProxy,
        implementation: *const extern "C" fn(),
        data: *mut c_void,
    ) -> c_int;

    // Send a request that creates a new object (returns it as a wl_proxy*).
    fn wl_proxy_marshal_constructor(
        proxy: *mut WlProxy,
        opcode: c_uint,
        interface: *const WlInterface,
        ...
    ) -> *mut WlProxy;

    // Same as above but also negotiates the interface version.
    fn wl_proxy_marshal_constructor_versioned(
        proxy: *mut WlProxy,
        opcode: c_uint,
        interface: *const WlInterface,
        version: c_uint,
        ...
    ) -> *mut WlProxy;

    // Send a request that does not create a new object.
    fn wl_proxy_marshal(proxy: *mut WlProxy, opcode: c_uint, ...);

    // Free a proxy object (does NOT destroy the server-side resource unless
    // the protocol requires a destructor request first).
    fn wl_proxy_destroy(proxy: *mut WlProxy);

    // Per-interface descriptor structs exported by libwayland-client.so.
    // Generated by wayland-scanner from the core Wayland XML protocol.
    static wl_registry_interface:      WlInterface;
    static wl_compositor_interface:    WlInterface;
    static wl_subcompositor_interface: WlInterface;
    static wl_surface_interface:       WlInterface;
    static wl_subsurface_interface:    WlInterface;
    static wl_shm_interface:           WlInterface;
    static wl_shm_pool_interface:      WlInterface;
    static wl_buffer_interface:        WlInterface;
}

// ---------------------------------------------------------------------------
// Registry listener
// ---------------------------------------------------------------------------

// Collects the three globals we need from the Wayland registry.
struct Globals {
    compositor:    *mut WlCompositor,
    subcompositor: *mut WlSubcompositor,
    shm:           *mut WlShm,
}

// Called by wl_display_roundtrip once per advertised global.
unsafe extern "C" fn registry_global(
    data: *mut c_void,
    registry: *mut WlProxy,
    name: u32,       // numeric name — passed back to wl_registry_bind
    interface: *const c_char, // interface name string, e.g. "wl_compositor"
    version: u32,    // highest version supported by the compositor
) {
    let g = &mut *(data as *mut Globals);
    let iface = std::ffi::CStr::from_ptr(interface).to_str().unwrap_or("");

    // Bind the global at the minimum of (compositor version, our max).
    // The variadic args are: name, interface_name, version, null_sentinel.
    macro_rules! bind {
        ($field:ident, $sym:ident, $max:expr) => {{
            let ver = version.min($max);
            g.$field = wl_proxy_marshal_constructor_versioned(
                registry, WL_REGISTRY_BIND, &$sym as *const _,
                ver, name, $sym.name, ver, std::ptr::null_mut::<c_void>(),
            ) as *mut _;
        }};
    }
    match iface {
        "wl_compositor"    => bind!(compositor,    wl_compositor_interface,    4),
        "wl_subcompositor" => bind!(subcompositor, wl_subcompositor_interface, 1),
        "wl_shm"           => bind!(shm,           wl_shm_interface,           1),
        _ => {}
    }
}

// Required by the listener vtable; we don't react to globals being removed.
unsafe extern "C" fn registry_global_remove(_: *mut c_void, _: *mut WlProxy, _: u32) {}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Open the Wayland registry, wait for the globals event burst, and return the
/// three bound globals we need. The caller is responsible for calling
/// `wl_proxy_destroy(registry)` when the globals are no longer needed.
unsafe fn bind_globals(wl_disp: *mut WlDisplay) -> Result<(Globals, *mut WlRegistry), String> {
    let registry = wl_proxy_marshal_constructor(
        wl_disp as *mut WlProxy,
        WL_DISPLAY_GET_REGISTRY,
        &wl_registry_interface as *const _,
        std::ptr::null_mut::<c_void>(),
    ) as *mut WlRegistry;
    if registry.is_null() {
        return Err("wl_display_get_registry returned null".into());
    }

    let mut globals = Globals {
        compositor:    std::ptr::null_mut(),
        subcompositor: std::ptr::null_mut(),
        shm:           std::ptr::null_mut(),
    };

    // The listener vtable must be a C-compatible function pointer array.
    // transmute is required because Rust doesn't allow casting unsafe fn
    // pointers to safe fn pointers directly.
    let listener: [extern "C" fn(); 2] = [
        std::mem::transmute(registry_global        as unsafe extern "C" fn(_, _, _, _, _)),
        std::mem::transmute(registry_global_remove as unsafe extern "C" fn(_, _, _)),
    ];
    wl_proxy_add_listener(
        registry as *mut WlProxy,
        listener.as_ptr(),
        &mut globals as *mut _ as *mut c_void,
    );

    // Block until the compositor has sent all current globals (synchronous).
    if wl_display_roundtrip(wl_disp) < 0 {
        return Err("wl_display_roundtrip failed".into());
    }
    Ok((globals, registry))
}

/// Create a `wl_surface` + `wl_subsurface` parented to the GTK window surface.
///
/// * `parent_surface` — the GTK window's `wl_surface*`
/// * `x`, `y` — position relative to the parent's top-left (0,0)
/// * `below_sibling` — sibling to stack below (pass `parent_surface` to go
///   to the very bottom, behind all other layers)
/// * `desync` — if true, commits on this surface take effect immediately
///   (required for wgpu render loops; without it frames wait for GTK commits)
///
/// The returned `wl_subsurface` is intentionally never destroyed — destroying
/// it would detach the surface from the parent and remove it from screen.
unsafe fn make_subsurface(
    compositor:     *mut WlCompositor,
    subcompositor:  *mut WlSubcompositor,
    parent_surface: *mut c_void,
    x: i32,
    y: i32,
    below_sibling: *mut c_void,
    desync: bool,
) -> Result<(*mut WlSurface, *mut WlSubsurface), String> {
    // A wl_surface is a rectangular region the compositor can display.
    let surface = wl_proxy_marshal_constructor(
        compositor as *mut WlProxy,
        WL_COMPOSITOR_CREATE_SURFACE,
        &wl_surface_interface as *const _,
        std::ptr::null_mut::<c_void>(),
    ) as *mut WlSurface;
    if surface.is_null() {
        return Err("wl_compositor_create_surface returned null".into());
    }

    // A wl_subsurface binds `surface` as a child of `parent_surface`,
    // giving us independent position and z-order control.
    let subsurface = wl_proxy_marshal_constructor(
        subcompositor as *mut WlProxy,
        WL_SUBCOMPOSITOR_GET_SUBSURFACE,
        &wl_subsurface_interface as *const _,
        std::ptr::null_mut::<c_void>(),
        surface,        // the new child surface
        parent_surface, // its parent
    ) as *mut WlSubsurface;
    if subsurface.is_null() {
        wl_proxy_destroy(surface as *mut WlProxy);
        return Err("wl_subcompositor_get_subsurface returned null".into());
    }

    // Position is relative to the parent surface origin (top-left = 0,0).
    wl_proxy_marshal(subsurface as *mut WlProxy, WL_SUBSURFACE_SET_POSITION, x, y);

    // place_below(sibling) inserts this surface just below `sibling` in the
    // compositor's stacking list. Passing the parent puts it at the very bottom.
    wl_proxy_marshal(subsurface as *mut WlProxy, WL_SUBSURFACE_PLACE_BELOW, below_sibling);

    if desync {
        // Desync mode: wl_surface_commit on this surface takes effect immediately
        // rather than waiting for the next parent commit.
        wl_proxy_marshal(subsurface as *mut WlProxy, WL_SUBSURFACE_SET_DESYNC);
    }

    Ok((surface, subsurface)) // both kept alive; caller may store subsurface for later repositioning
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Raw Wayland handles for a wgpu render target created by [`create_wgpu_subsurface`].
pub struct WgpuSurface {
    /// `wl_surface*` cast to `isize`.
    pub surface: isize,
    /// `wl_display*` cast to `isize`.
    pub display: isize,
    /// `wl_subsurface*` cast to `isize` — store this to reposition the
    /// subsurface later via [`move_wgpu_subsurface`].
    pub subsurface: isize,
}

/// Create a bare `wl_subsurface` placed below the parent GTK surface for wgpu
/// to render into.
///
/// No pixel buffer is attached here — wgpu configures the surface and attaches
/// its own buffers via `wgpu::Surface`. The surface is placed in desync mode so
/// the wgpu render loop can commit frames without waiting for GTK.
pub fn create_wgpu_subsurface(
    parent_surface: *mut c_void,
    display: *mut c_void,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<WgpuSurface, String> {
    unsafe {
        let wl_disp = display as *mut WlDisplay;
        let (globals, registry) = bind_globals(wl_disp)?;
        if globals.compositor.is_null()    { return Err("wl_compositor not found".into()); }
        if globals.subcompositor.is_null() { return Err("wl_subcompositor not found".into()); }

        let (surface, subsurface) = make_subsurface(
            globals.compositor, globals.subcompositor,
            parent_surface, x, y,
            parent_surface, // place below parent → visible through transparent React
            true,           // desync → wgpu frames commit independently of GTK
        )?;

        // Commit the parent so the new subsurface position/z-order takes effect.
        wl_proxy_marshal(parent_surface as *mut WlProxy, WL_SURFACE_COMMIT);
        wl_display_flush(wl_disp);
        wl_proxy_destroy(registry as *mut WlProxy);

        println!("[wgpu-surface] created: pos=({x},{y}), size={width}x{height}, surface={:#x}", surface as usize);
        Ok(WgpuSurface { surface: surface as isize, display: display as isize, subsurface: subsurface as isize })
    }
}

/// Handles returned by [`create_shm_subsurface`], kept alive so the surface can
/// be resized later via [`resize_shm_surface`] without re-binding the registry.
pub struct ShmSubsurface {
    /// `wl_surface*` — the backing Wayland surface (cast to `*mut c_void`).
    pub surface: *mut c_void,
    /// `wl_shm*` — kept alive so `resize_shm_surface` can create new pools
    /// without an extra `wl_display_roundtrip`.
    pub shm: *mut c_void,
}

// SAFETY: we access these pointers only from the Tauri main thread (setup +
// window-event callbacks), which is the same thread GTK's Wayland connection
// belongs to. The pointers are never freed — intentional lifetime extension.
unsafe impl Send for ShmSubsurface {}
unsafe impl Sync for ShmSubsurface {}

/// Create a solid-colour `wl_subsurface` backed by a shared-memory pixel buffer.
///
/// Pass `parent_surface` as `below_sibling` to place it below all other subsurfaces.
pub fn create_shm_subsurface(
    parent_surface: *mut c_void,
    display: *mut c_void,
    argb: u32,
    width: i32, height: i32,
    x: i32, y: i32,
    below_sibling: *mut c_void,
) -> Result<ShmSubsurface, String> {
    unsafe {
        let wl_disp = display as *mut WlDisplay;
        let (globals, registry) = bind_globals(wl_disp)?;
        if globals.compositor.is_null()    { return Err("wl_compositor not found".into()); }
        if globals.subcompositor.is_null() { return Err("wl_subcompositor not found".into()); }
        if globals.shm.is_null()           { return Err("wl_shm not found".into()); }

        let (surface, _subsurface) = make_subsurface(
            globals.compositor, globals.subcompositor,
            parent_surface, x, y, below_sibling,
            false, // sync mode is fine — buffer is static, committed only once
        )?;

        // --- Build the shared-memory pixel buffer ---
        let stride     = width * 4; // bytes per row (4 bytes per ARGB pixel)
        let total_size = (stride * height) as usize;

        // Create a file in /dev/shm (RAM-backed tmpfs) and immediately unlink
        // it so no name remains in the filesystem. The fd stays open, keeping
        // the memory alive; the compositor mmap()s it via the wl_shm_pool.
        let path = format!("/dev/shm/tauri-layer-{}-{}", std::process::id(), x);
        let file = std::fs::OpenOptions::new()
            .read(true).write(true).create(true).truncate(true)
            .open(&path)
            .map_err(|e| format!("open /dev/shm: {e}"))?;
        std::fs::remove_file(&path).ok(); // unlink name; fd keeps the data alive
        file.set_len(total_size as u64).map_err(|e| format!("set_len: {e}"))?;

        // Fill every pixel with the requested ARGB colour.
        let pixels: Vec<u32> = vec![argb; (width * height) as usize];
        let bytes = std::slice::from_raw_parts(pixels.as_ptr() as *const u8, total_size);
        use std::io::Write as _;
        (&file).write_all(bytes).map_err(|e| format!("write pixels: {e}"))?;

        use std::os::unix::io::AsRawFd as _;

        // wl_shm_pool wraps the fd; the compositor maps the memory.
        let pool = wl_proxy_marshal_constructor_versioned(
            globals.shm as *mut WlProxy, WL_SHM_CREATE_POOL,
            &wl_shm_pool_interface as *const _, 1u32,
            std::ptr::null_mut::<c_void>(), file.as_raw_fd() as c_int, total_size as c_int,
        ) as *mut WlShmPool;
        if pool.is_null() { return Err("wl_shm_create_pool returned null".into()); }

        // Create a wl_buffer view into the pool (offset=0, full width×height).
        let buffer = wl_proxy_marshal_constructor_versioned(
            pool as *mut WlProxy, WL_SHM_POOL_CREATE_BUFFER,
            &wl_buffer_interface as *const _, 1u32,
            std::ptr::null_mut::<c_void>(), 0i32, width, height, stride, WL_SHM_FORMAT_ARGB8888,
        ) as *mut WlBuffer;
        // Pool can be destroyed immediately; existing buffers keep their mapping.
        wl_proxy_marshal(pool as *mut WlProxy, WL_SHM_POOL_DESTROY);
        wl_proxy_destroy(pool as *mut WlProxy);
        if buffer.is_null() { return Err("wl_shm_pool_create_buffer returned null".into()); }

        // Attach the buffer, mark the whole surface as damaged, and commit.
        wl_proxy_marshal(surface as *mut WlProxy, WL_SURFACE_ATTACH, buffer, 0i32, 0i32);
        wl_proxy_marshal(surface as *mut WlProxy, WL_SURFACE_DAMAGE, 0i32, 0i32, width, height);
        wl_proxy_marshal(surface as *mut WlProxy, WL_SURFACE_COMMIT);

        // Commit the parent so the new subsurface state (position, z-order) is applied.
        wl_proxy_marshal(parent_surface as *mut WlProxy, WL_SURFACE_COMMIT);
        wl_display_flush(wl_disp);
        wl_proxy_destroy(registry as *mut WlProxy);

        println!("[layer] created: argb={argb:#010x}, pos=({x},{y}), size={width}x{height}");
        // `globals.shm` is intentionally kept alive (not destroyed) so the
        // caller can pass it to `resize_shm_surface` to create new pools without
        // a further `wl_display_roundtrip`.
        Ok(ShmSubsurface {
            surface: surface as *mut c_void,
            shm:     globals.shm as *mut c_void,
        })
    }
}

/// Resize an existing SHM subsurface by attaching a fresh pixel buffer.
///
/// `surface` and `shm` come from a previous [`create_shm_subsurface`] call.
/// The subsurface's position and z-order are unaffected — only the buffer
/// (and therefore the visible size) changes.
///
/// Safe to call from a window-resize callback without a `wl_display_roundtrip`
/// because `wl_shm` is already bound.
pub fn resize_shm_surface(
    surface: *mut c_void,
    display: *mut c_void,
    shm:     *mut c_void,
    argb: u32,
    width: i32, height: i32,
) -> Result<(), String> {
    unsafe {
        let wl_disp = display as *mut WlDisplay;
        let stride     = width * 4;
        let total_size = (stride * height) as usize;

        // ── Allocate a fresh /dev/shm pixel buffer at the new size.
        let path = format!("/dev/shm/tauri-bg-resize-{}", std::process::id());
        let file = std::fs::OpenOptions::new()
            .read(true).write(true).create(true).truncate(true)
            .open(&path)
            .map_err(|e| format!("open /dev/shm: {e}"))?;
        std::fs::remove_file(&path).ok(); // unlink name; fd keeps data alive
        file.set_len(total_size as u64).map_err(|e| format!("set_len: {e}"))?;

        // Fill with the requested ARGB colour.
        let pixels: Vec<u32> = vec![argb; (width * height) as usize];
        let bytes = std::slice::from_raw_parts(pixels.as_ptr() as *const u8, total_size);
        use std::io::Write as _;
        (&file).write_all(bytes).map_err(|e| format!("write pixels: {e}"))?;

        use std::os::unix::io::AsRawFd as _;

        // ── Create a wl_shm_pool + wl_buffer from the fd.
        //     The compositor mmap()s the fd to read pixel data directly.
        let pool = wl_proxy_marshal_constructor_versioned(
            shm as *mut WlProxy, WL_SHM_CREATE_POOL,
            &wl_shm_pool_interface as *const _, 1u32,
            std::ptr::null_mut::<c_void>(), file.as_raw_fd() as c_int, total_size as c_int,
        ) as *mut WlShmPool;
        if pool.is_null() { return Err("wl_shm_create_pool returned null".into()); }

        let buffer = wl_proxy_marshal_constructor_versioned(
            pool as *mut WlProxy, WL_SHM_POOL_CREATE_BUFFER,
            &wl_buffer_interface as *const _, 1u32,
            std::ptr::null_mut::<c_void>(), 0i32, width, height, stride, WL_SHM_FORMAT_ARGB8888,
        ) as *mut WlBuffer;
        // Pool can be destroyed immediately; existing buffers keep their mapping.
        wl_proxy_marshal(pool as *mut WlProxy, WL_SHM_POOL_DESTROY);
        wl_proxy_destroy(pool as *mut WlProxy);
        if buffer.is_null() { return Err("wl_shm_pool_create_buffer returned null".into()); }

        // ── Attach the new buffer, mark the whole surface as damaged, and commit.
        //     This atomically resizes the subsurface from the compositor's perspective.
        wl_proxy_marshal(surface as *mut WlProxy, WL_SURFACE_ATTACH, buffer, 0i32, 0i32);
        wl_proxy_marshal(surface as *mut WlProxy, WL_SURFACE_DAMAGE, 0i32, 0i32, width, height);
        wl_proxy_marshal(surface as *mut WlProxy, WL_SURFACE_COMMIT);
        wl_display_flush(wl_disp);

        Ok(())
    }
}

/// Move an existing wgpu subsurface to a new position within the parent
/// surface.  Must be called from the GTK/Wayland main thread.
///
/// `subsurface` is the `wl_subsurface*` returned in [`WgpuSurface::subsurface`].
pub fn move_wgpu_subsurface(
    subsurface:     *mut c_void,
    parent_surface: *mut c_void,
    display:        *mut c_void,
    x: i32,
    y: i32,
) -> Result<(), String> {
    unsafe {
        wl_proxy_marshal(subsurface as *mut WlProxy, WL_SUBSURFACE_SET_POSITION, x, y);
        wl_proxy_marshal(parent_surface as *mut WlProxy, WL_SURFACE_COMMIT);
        wl_display_flush(display as *mut WlDisplay);
        Ok(())
    }
}

/// Destroy a wgpu subsurface created by [`create_wgpu_subsurface`].
///
/// Sends `wl_subsurface::destroy` + `wl_surface::destroy`, then flushes.
/// Must be called from the GTK/Wayland main thread.
/// Call this AFTER the render thread has been signalled to shut down.
pub fn destroy_wgpu_surface(
    surface:    *mut c_void,
    subsurface: *mut c_void,
    parent_surface: *mut c_void,
    display:    *mut c_void,
) -> Result<(), String> {
    unsafe {
        // ── Destroy the subsurface role first so the parent forgets the child.
        wl_proxy_marshal(subsurface as *mut WlProxy, WL_SUBSURFACE_DESTROY);
        wl_proxy_destroy(subsurface as *mut WlProxy);

        // ── Then destroy the surface itself (frees GPU/compositor resources).
        wl_proxy_marshal(surface as *mut WlProxy, WL_SURFACE_DESTROY);
        wl_proxy_destroy(surface as *mut WlProxy);

        // ── Commit the parent so the compositor removes the subsurface
        //     immediately (no ghost panel left behind).
        wl_proxy_marshal(parent_surface as *mut WlProxy, WL_SURFACE_COMMIT);
        wl_display_flush(display as *mut WlDisplay);
        Ok(())
    }
}
