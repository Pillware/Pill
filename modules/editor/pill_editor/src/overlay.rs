//! The editor's composition overlay: the UI layer drawn above the scene's
//! own child window.
//!
//! # Responsibilities
//!
//! - [`OverlaySurface`]: the DirectComposition target and visual over the
//!   editor window, the `wgpu` surface drawn through it, the size that keeps
//!   the swapchain matching the window, and the gate that skips empty frames.
//!
//! # Design
//!
//! A flip-model swapchain on the window's `HWND` cannot be transparent over
//! the scene's child window. A DirectComposition visual on a `topmost`
//! target can: the desktop compositor draws it above every child surface, so
//! each overlay pixel blends with the scene through its own alpha. `wgpu`
//! draws into the overlay through `SurfaceTargetUnsafe::CompositionVisual`.
//! Its DX12 backend implements that by setting the swapchain as the visual's
//! content, so the tree is committed after every configure and the visual
//! outlives the surface.
//!
//! The content is the shell's `EguiShellFrame`. `egui-wgpu`'s renderer draws
//! it on the editor's own device with premultiplied alpha (each colour
//! already multiplied by its alpha), which is the form the compositor
//! expects.
//!
//! Only the editor binary creates the DirectComposition device, through the
//! `windows` crate. A shared dylib must not: every hot patch of it would
//! otherwise import `dcomp.dll`.

// Standard library
use std::sync::Arc;

// External crates
// Only the composition path logs through the editor target; the window-surface
// posture has nothing to report, so the import is platform-gated too.
#[cfg(windows)]
use pill_core::telemetry::telemetry_target;
use pill_editor_core::egui::epaint::ClippedPrimitive;
use pill_editor_core::egui::TexturesDelta;
use winit::window::Window;

// =============================================================================
// Constants
// =============================================================================

/// The colour the window posture clears to: a dark neutral, so an uncovered
/// region of the window is not black.
const WINDOW_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.09,
    g: 0.10,
    b: 0.12,
    a: 1.0,
};

// =============================================================================
// Overlay types
// =============================================================================

/// Why an overlay could not be created. The executable logs it, falls back to
/// the window surface and retries the composition overlay every so often.
pub type OverlayError = String;

/// Where the UI's pixels go.
///
/// `Composition` is only ever constructed by the Windows posture and
/// `Floating` only by the Unix one, so each is unconstructed on the other
/// platform while the runner keeps matching on both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayMode {
    /// The composition visual above the scene: transparent where the shell
    /// drew nothing, so the scene shows through the UI's gaps.
    #[cfg_attr(not(windows), allow(dead_code))]
    Composition,
    /// A window of the editor's own, drawn by the desktop above the scene's
    /// child windows and transparent where the shell drew nothing.
    ///
    /// This is the Unix counterpart of [`Self::Composition`], which needs
    /// DirectComposition: a Unix child window is always drawn *above* its
    /// parent, so UI drawn on the editor's own surface can never cover a
    /// scene viewport, while a window of its own can.
    #[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
    Floating,
    /// The window's own surface, with no composition: `PILL_EDITOR_NO_OVERLAY`
    /// asked for it, or a failed overlay fell back to it. The UI is opaque,
    /// and a scene child window sits above its parent, so it hides whatever
    /// the UI draws under it.
    Window,
}

impl OverlayMode {
    /// Whether the surface lets the scene show through where the UI drew
    /// nothing.
    pub fn clears_transparent(self) -> bool {
        matches!(self, OverlayMode::Composition | OverlayMode::Floating)
    }
}

// =============================================================================
// OverlaySurface
// =============================================================================

/// The composition overlay over the editor window.
pub struct OverlaySurface {
    /// The posture this surface was created in.
    mode: OverlayMode,
    /// The DirectComposition tree the surface is set on. It is kept alive
    /// and committed for the surface's whole life; only the composition
    /// posture has one.
    #[cfg(windows)]
    composition: Option<composition::Tree>,
    /// The overlay's swapchain, drawn through the composition visual.
    surface: wgpu::Surface<'static>,
    /// The window the floating posture draws into. Only that posture has one:
    /// the other two draw into a window the editor already owns, whose
    /// placement is not the overlay's to set.
    floating_window: Option<Arc<Window>>,
    /// The swapchain's configuration: the window's client area in physical
    /// pixels, the editor's format, the posture's composition mode.
    surface_configuration: wgpu::SurfaceConfiguration,
    /// Whether the last frame drew content, so the first blank frame after
    /// content clears the swapchain and later blanks cost no GPU work.
    has_drawn_content: bool,
    /// Overlay frames presented since startup.
    frame_count: u64,
}

impl OverlaySurface {
    /// Create the composition overlay for `window`.
    ///
    /// DirectComposition and the `CompositionVisual` surface target wgpu takes
    /// from it are Windows-only, so this posture exists there alone; every
    /// other platform starts in [`Self::new_window`] instead.
    ///
    /// # Errors
    ///
    /// Returns the reason, which the executable logs, when the adapter is not
    /// DX12, when the composition tree cannot be built or committed, when the
    /// surface cannot be created from the visual, or when the swapchain cannot
    /// carry the given format and premultiplied alpha.
    #[cfg(windows)]
    pub fn new(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        window: Arc<Window>,
        format: wgpu::TextureFormat,
        window_size: (u32, u32),
    ) -> Result<OverlaySurface, OverlayError> {
        // Step 1: the composition visual comes first - the surface is created
        // from it, and wgpu keeps the visual alive as long as the surface.
        if adapter.get_info().backend != wgpu::Backend::Dx12 {
            return Err(format!(
                "the overlay needs the DX12 backend (CompositionVisual is DX12-only), the adapter is {:?}",
                adapter.get_info().backend
            ));
        }
        let composition = composition::Tree::new(&window)?;
        // Step 2: the surface, from the visual's COM pointer.
        // SAFETY: the visual is a live `IDCompositionVisual` owned by
        // `composition`, which outlives the surface, and the pointer is its
        // interface pointer, which is what the DX12 backend borrows and clones.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(
                composition.visual_pointer(),
            ))
        }
        .map_err(|error| format!("the composition surface was refused: {error}"))?;
        // Step 3: the swapchain must carry the editor's format and
        // premultiplied composition (the visual is composited over the scene).
        let capabilities = surface.get_capabilities(adapter);
        if !capabilities.formats.contains(&format) {
            return Err(format!(
                "the overlay surface does not offer the editor's format {format:?}"
            ));
        }
        if !capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            return Err("the overlay surface does not offer premultiplied composition".to_string());
        }
        let surface_configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: window_size.0.max(1),
            height: window_size.1.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(device, &surface_configuration);
        // Step 4: `configure` set the swapchain as the visual's content, so
        // the tree only shows it once committed.
        composition.commit()?;
        Ok(OverlaySurface {
            mode: OverlayMode::Composition,
            #[cfg(windows)]
            composition: Some(composition),
            surface,
            floating_window: None,
            surface_configuration,
            has_drawn_content: false,
            frame_count: 0,
        })
    }

    /// Create the overlay in the window posture: the UI's frames go to the
    /// window's own surface, with no composition and no transparency.
    ///
    /// This is the posture `PILL_EDITOR_NO_OVERLAY` asks for, and the one a
    /// failed composition overlay falls back to. It needs no composition and
    /// no transparency, so every backend can provide it.
    ///
    /// # Errors
    ///
    /// Returns the reason, which the executable logs, when the window surface
    /// cannot be created or does not offer the editor's format.
    pub fn new_window(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        window: Arc<Window>,
        format: wgpu::TextureFormat,
        window_size: (u32, u32),
    ) -> Result<OverlaySurface, OverlayError> {
        // Step 1: a plain surface on the window.
        let surface = instance
            .create_surface(window)
            .map_err(|error| format!("the window surface was refused: {error}"))?;
        let capabilities = surface.get_capabilities(adapter);
        if !capabilities.formats.contains(&format) {
            return Err(format!(
                "the window surface does not offer the editor's format {format:?}"
            ));
        }
        // Step 2: pick the first alpha mode that is not premultiplied; the
        // window posture draws an opaque UI. `Auto` is the fallback when every
        // mode is premultiplied.
        let alpha_mode = capabilities
            .alpha_modes
            .iter()
            .copied()
            .find(|mode| *mode != wgpu::CompositeAlphaMode::PreMultiplied)
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        let surface_configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: window_size.0.max(1),
            height: window_size.1.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(device, &surface_configuration);
        Ok(OverlaySurface {
            mode: OverlayMode::Window,
            #[cfg(windows)]
            composition: None,
            surface,
            floating_window: None,
            surface_configuration,
            has_drawn_content: false,
            frame_count: 0,
        })
    }

    /// Create the overlay in the floating posture: a window of the editor's
    /// own, transparent where the shell drew nothing.
    ///
    /// A Unix loader has no composition visual, and a child window of the
    /// editor window is always drawn above it, so UI drawn on the editor's own
    /// surface can never cover a scene viewport. A window of its own, which
    /// the desktop compositor paints above the editor, can - and it takes no
    /// input, so every pointer event still reaches the window underneath.
    ///
    /// # Errors
    ///
    /// Returns the reason, which the executable logs, when the window surface
    /// cannot be created, does not offer the editor's format, or does not offer
    /// premultiplied composition - without which the window would paint a
    /// black rectangle over the scene.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn new_floating(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        window: Arc<Window>,
        format: wgpu::TextureFormat,
        window_size: (u32, u32),
    ) -> Result<OverlaySurface, OverlayError> {
        // Step 1: the surface, and the format and alpha mode the posture
        // needs.
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|error| format!("the floating surface was refused: {error}"))?;
        let capabilities = surface.get_capabilities(adapter);
        if !capabilities.formats.contains(&format) {
            return Err(format!(
                "the floating surface does not offer the editor's format {format:?}"
            ));
        }
        if !capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            return Err(
                "the floating surface does not offer premultiplied composition".to_string(),
            );
        }
        let surface_configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: window_size.0.max(1),
            height: window_size.1.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(device, &surface_configuration);
        Ok(OverlaySurface {
            mode: OverlayMode::Floating,
            #[cfg(windows)]
            composition: None,
            surface,
            floating_window: Some(window),
            surface_configuration,
            has_drawn_content: false,
            frame_count: 0,
        })
    }

    /// Returns the posture this surface draws in.
    pub fn mode(&self) -> OverlayMode {
        self.mode
    }

    /// The window the floating posture draws into, if this is that posture.
    pub fn floating_window(&self) -> Option<&Arc<Window>> {
        self.floating_window.as_ref()
    }

    /// Resize the swapchain to `window_size` physical pixels (the window's
    /// client area), and commit the composition tree when there is one.
    pub fn resize(&mut self, device: &wgpu::Device, window_size: (u32, u32)) {
        let width = window_size.0.max(1);
        let height = window_size.1.max(1);
        if self.surface_configuration.width == width && self.surface_configuration.height == height
        {
            return;
        }
        self.surface_configuration.width = width;
        self.surface_configuration.height = height;
        self.surface.configure(device, &self.surface_configuration);
        #[cfg(windows)]
        if let Some(composition) = &self.composition {
            if let Err(error) = composition.commit() {
                pill_core::warn!(
                    target: telemetry_target::EDITOR,
                    "[editor] overlay commit failed after resize: {error}"
                );
            }
        }
    }

    /// Returns the number of overlay frames presented since startup.
    pub fn frames(&self) -> u64 {
        self.frame_count
    }

    /// Draw the shell frame's meshes and present them.
    ///
    /// An empty mesh list clears the overlay and presents that once, so the
    /// previous frame's content disappears. The composition posture clears to
    /// fully transparent; the window posture clears to `WINDOW_CLEAR`. A
    /// still-empty list after that is not drawn at all. Returns whether GPU
    /// work happened.
    ///
    /// # Errors
    ///
    /// Returns the reason when the swapchain gives no frame this time; the
    /// caller logs it and tries again on the next frame.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut egui_wgpu::Renderer,
        meshes: &[ClippedPrimitive],
        mut textures: TexturesDelta,
        pixels_per_point: f32,
    ) -> Result<bool, OverlayError> {
        // The overlay's paint and present, in one zone: the texture uploads,
        // the recorded pass, its submission and the swapchain present.
        let _zone = pill_core::profile_scope!("overlay paint and present");
        let has_content = !meshes.is_empty();
        if !has_content && !self.has_drawn_content {
            // Nothing is drawn, so nothing the shell uploaded is used: the
            // texture delta is cleared rather than left to trip epaint's debug
            // assertion when it drops.
            textures.clear();
            return Ok(false);
        }
        // Step 1: apply this frame's texture updates before drawing with
        // them. One texture may carry several deltas in one frame. The
        // updates belong to the renderer even when no swapchain frame comes
        // back, so they are applied before that check; dropping a delta
        // unapplied trips epaint's debug assertion.
        for (texture_id, deltas) in &textures.set {
            for image_delta in deltas {
                renderer.update_texture(device, queue, *texture_id, image_delta);
            }
        }
        for texture_id in &textures.free {
            renderer.free_texture(texture_id);
        }
        textures.clear();
        // Step 2: a suboptimal frame is still drawable; an occluded window
        // draws nothing, and any other status goes back to the caller as an
        // error, which logs it and skips the UI for this frame.
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Occluded => return Ok(false),
            other => return Err(format!("the overlay swapchain is unavailable: {other:?}")),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [
                self.surface_configuration.width,
                self.surface_configuration.height,
            ],
            pixels_per_point: pixels_per_point.max(0.01),
        };
        // Step 3: record the draw commands. `update_buffers` may produce its
        // own command buffers, which are submitted alongside the pass.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pill_editor overlay"),
        });
        let user_buffers =
            renderer.update_buffers(device, queue, &mut encoder, meshes, &screen_descriptor);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pill_editor overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // The composition and floating postures clear to
                        // fully transparent: something else is behind the UI
                        // there, and whatever the meshes leave out must show
                        // it through. The window posture owns every pixel of
                        // its surface, so it clears to `WINDOW_CLEAR` instead.
                        load: wgpu::LoadOp::Clear(if self.mode.clears_transparent() {
                            wgpu::Color::TRANSPARENT
                        } else {
                            WINDOW_CLEAR
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // `forget_lifetime` widens the pass's borrows to `'static`. The
            // pass stays inside this block, so nothing here outlives `view` or
            // the encoder.
            let mut pass = pass.forget_lifetime();
            renderer.render(&mut pass, meshes, &screen_descriptor);
        }
        // Step 4: submit and present.
        queue.submit(user_buffers.into_iter().chain(Some(encoder.finish())));
        queue.present(frame);
        self.has_drawn_content = has_content;
        self.frame_count += 1;
        Ok(true)
    }
}

// =============================================================================
// Window handle
// =============================================================================

/// The window's `HWND`, as the composition target needs it. Returns `None`
/// when the window's raw handle is not a Win32 one.
#[cfg(windows)]
pub fn hwnd_of(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(windows::Win32::Foundation::HWND(
            win32.hwnd.get() as *mut std::ffi::c_void
        )),
        _ => None,
    }
}

// =============================================================================
// DirectComposition
// =============================================================================

/// The DirectComposition half of the overlay, in the Windows API's terms.
#[cfg(windows)]
mod composition {
    // Standard library
    use std::ffi::c_void;

    // External crates
    use windows::core::IUnknown;
    use windows::Win32::Graphics::DirectComposition::{
        DCompositionCreateDevice3, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
    };
    use winit::window::Window;

    // Current crate
    use crate::overlay::hwnd_of;

    /// One window's composition tree: the composition device, a `topmost`
    /// target on the window, and the visual whose content becomes the
    /// overlay's swapchain.
    pub struct Tree {
        /// The composition device; every tree change is committed through it.
        device: IDCompositionDevice,
        /// The target on the window: the root the visual is set on. Held for
        /// as long as the tree, so the root stays alive.
        _target: IDCompositionTarget,
        /// The visual whose content becomes the overlay's swapchain.
        visual: IDCompositionVisual,
    }

    impl Tree {
        /// Create the tree over `window`, with the visual on top of the
        /// window's child surfaces.
        ///
        /// # Errors
        ///
        /// Returns a message when the window has no Win32 handle, when the
        /// composition device, the target or the visual cannot be created,
        /// when the visual cannot be set as the target's root, or when the
        /// first commit fails.
        pub fn new(window: &Window) -> Result<Tree, String> {
            let hwnd = hwnd_of(window).ok_or("the window has no Win32 handle")?;
            // SAFETY: `DCompositionCreateDevice3` is given no rendering
            // device (the composition device makes its own), and returns a
            // fresh interface whose reference the returned value owns.
            let device: IDCompositionDevice =
                unsafe { DCompositionCreateDevice3(None::<&IUnknown>) }
                    .map_err(|error| format!("DCompositionCreateDevice3 failed: {error}"))?;
            // SAFETY: the handle is live and the device is alive; `true` asks
            // for the topmost target, which is what puts the visual above the
            // window's child windows.
            let target = unsafe { device.CreateTargetForHwnd(hwnd, true) }
                .map_err(|error| format!("CreateTargetForHwnd failed: {error}"))?;
            // SAFETY: the device is alive; the visual is a fresh interface the
            // returned value owns.
            let visual = unsafe { device.CreateVisual() }
                .map_err(|error| format!("CreateVisual failed: {error}"))?;
            // SAFETY: the target and the visual are alive and belong to this
            // device, which is the precondition `SetRoot` requires.
            unsafe { target.SetRoot(&visual) }
                .map_err(|error| format!("SetRoot failed: {error}"))?;
            let tree = Tree {
                device,
                _target: target,
                visual,
            };
            tree.commit()?;
            Ok(tree)
        }

        /// The visual's COM interface pointer, which `wgpu`'s DX12 backend
        /// borrows to build the overlay surface.
        pub fn visual_pointer(&self) -> *mut c_void {
            windows::core::Interface::as_raw(&self.visual)
        }

        /// Publish the tree: the composition device only shows a change once
        /// it is committed.
        ///
        /// # Errors
        ///
        /// Returns a message when the composition device refuses the commit.
        pub fn commit(&self) -> Result<(), String> {
            // SAFETY: the device is alive for as long as the tree is.
            unsafe { self.device.Commit() }.map_err(|error| format!("Commit failed: {error}"))
        }
    }
}
