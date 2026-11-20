//! The window surface and the swapchain lifecycle it carries.
//!
//! # Responsibilities
//!
//! - Build the GPU bootstrap in the one order that works: instance, surface,
//!   adapter, device, queue, configuration ([`Surface::create`]).
//! - Keep the surface configured - at startup, on a resize, and on the
//!   recovery path a lost or outdated swapchain needs - in vsync unless the
//!   caller or `PILL_PRESENT_MODE` asked for another present mode.
//! - Hand out frames ([`Surface::acquire`]), and own the colour format every
//!   pipeline that renders to it declares.
//!
//! # Design
//!
//! [`Surface::create`] builds the surface, the device and the queue together,
//! because the adapter and the device are chosen for the surface they have to
//! present to. The type owns the state that belongs to the surface: the
//! swapchain, its configuration, and the format chosen from the surface's
//! capabilities. It takes the device as an argument wherever it needs one, so
//! a type named for the surface does not quietly become the device's owner.
//!
//! Nothing here names a windowing crate. The surface is built from a
//! [`RawWindowData`], which carries the window's platform handles as plain
//! data, and the sizes that reach it are plain pixels. That is what lets any
//! winit frontend (the standalone host, the editor) attach this renderer.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_core::{info, warn};
use pill_renderer_api::RawWindowData;

// Current crate
use crate::error::{captured, captured_now, CapturedErrors, RendererError, Result};

// =============================================================================
// Constants
// =============================================================================

/// Environment variable that asks for a present mode other than vsync.
const PRESENT_MODE_VARIABLE: &str = "PILL_PRESENT_MODE";

// =============================================================================
// Surface
// =============================================================================

/// A window's swapchain, its configuration, and the colour format its
/// pipelines draw in.
///
/// Built by [`Surface::create`] or [`Surface::create_additional`], and
/// reconfigured in place for the rest of its life. The device it was configured
/// against is passed in rather than held: the same device creates every other
/// GPU object in the renderer, so holding it here would make this type look
/// like the device's owner as well as the surface's.
pub struct Surface {
    surface: wgpu::Surface<'static>,
    surface_configuration: wgpu::SurfaceConfiguration,
    /// The render format: what the pipelines render to and what each frame is
    /// drawn through. It is the configured format with an sRGB suffix where
    /// the surface has an sRGB variant; see [`Surface::create`].
    render_format: wgpu::TextureFormat,
    /// The graphics API the device runs on, which the GPU profiler names its
    /// timeline with.
    backend: wgpu::Backend,
}

impl Surface {
    /// Builds the GPU bootstrap: instance, surface, adapter, device and queue.
    ///
    /// Returns the surface beside the device and queue it was configured with,
    /// and the instance and adapter it was built from:
    /// [`Surface::create_additional`] needs both to build further surfaces.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when a creation step fails: surface or
    /// adapter acquisition, device creation, a surface advertising no texture
    /// format or alpha mode, or a configuration every candidate was refused
    /// for.
    ///
    /// # Safety
    ///
    /// `window` must name a live window, and that window must outlive the
    /// returned surface: nothing here keeps it alive.
    pub async unsafe fn create(
        window: RawWindowData,
        width: u32,
        height: u32,
    ) -> Result<(
        Self,
        wgpu::Device,
        wgpu::Queue,
        wgpu::Instance,
        wgpu::Adapter,
    )> {
        // Step 1: Clamp the requested size and build the instance and the
        // window surface. A zero-sized window still needs a surface it can
        // configure, and the first resize replaces the clamped size.
        let width = width.max(1);
        let height = height.max(1);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            // Vulkan, DX12, Metal or WebGPU, never OpenGL unless `WGPU_BACKEND`
            // asks for it. On Windows, wgpu's GL backend leaves a hidden helper
            // window behind, whose message handler is code in this DLL. Once
            // the host unloads the renderer (on a reload or at shutdown), the
            // next message to that window calls freed code, and the process
            // dies with 0xC000041D or 0xC0000005. Every machine this renders on
            // has one of the primary backends, so GL only ever added that
            // hazard.
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            // Debug labels in a debug build, the backend's validation layer off
            // unless `WGPU_VALIDATION=1` asks for it: that layer ships with the
            // Vulkan SDK, and without it every start warns that it is missing.
            // wgpu's own API validation runs either way.
            flags: wgpu::InstanceFlags::from_build_config()
                .difference(wgpu::InstanceFlags::VALIDATION)
                .with_env(),
            backend_options: wgpu::BackendOptions::default(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let (raw_window_handle, raw_display_handle) = window.to_raw()?;
        // SAFETY: the handles name a live window that outlives the surface,
        // which is this function's own contract with its caller.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(raw_display_handle),
                raw_window_handle,
            })
        }
        .map_err(|error| RendererError::SurfaceCreation {
            detail: error.to_string(),
        })?;
        // Step 2: Pick the adapter and the device, asking only for the
        // features the renderer uses.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|error| RendererError::Backend {
                detail: format!("no compatible GPU adapter: {error}"),
            })?;
        let adapter_info = adapter.get_info();
        let backend = adapter_info.backend;
        info!(target: telemetry_target::RENDERING, adapter = adapter_info.name.as_str(), backend = ?adapter_info.backend, "using GPU");
        let mut wanted_features = wgpu::Features::DEPTH_CLIP_CONTROL;
        // Only when GPU profiling was asked for: the query features change
        // nothing until a query is recorded, and the renderer asks the device
        // only for what it uses. Occlusion queries need no feature; they are
        // core in wgpu.
        if crate::profiler::gpu_profiling_requested() {
            wanted_features |= crate::profiler::GPU_PROFILE_FEATURES;
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("pill renderer device"),
                required_features: wanted_features & adapter.features(),
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .map_err(|error| RendererError::Backend {
                detail: format!("no GPU device from the adapter: {error}"),
            })?;
        crate::error::report_uncaptured_errors(&device);
        crate::error::latch_device_loss(&device);
        // Step 3: Choose the format and the alpha mode, then configure the
        // surface.
        let capabilities = surface.get_capabilities(&adapter);
        let color_format =
            pick_color_format(&capabilities.formats).ok_or_else(|| RendererError::Backend {
                detail: "the surface offers no texture format to render into".to_owned(),
            })?;
        // Pipelines draw through an sRGB view, so the hardware encodes the
        // linear colours they write. A surface with no sRGB format of its own
        // (a WebGPU canvas has none) is configured in its plain format, with
        // the sRGB variant allowed as a view format. Drawing straight into the
        // plain format would put linear values on screen, which looks far too
        // dark.
        let render_format = color_format.add_srgb_suffix();
        let alpha_mode =
            pick_alpha_mode(&capabilities.alpha_modes).ok_or_else(|| RendererError::Backend {
                detail: "the surface offers no alpha mode to present with".to_owned(),
            })?;
        // `Fifo` is the one present mode a surface is required to support, and
        // a mode listed by `Surface::get_capabilities` is not necessarily one
        // the driver can create: the NVIDIA Vulkan driver on Windows
        // advertises `Mailbox` and then fails the flip-model swapchain with
        // "Not enough memory left". A mode the driver refuses is retried as
        // `Fifo` below, so an uncapped mode stays an opt-in for a driver
        // verified to create one (`PILL_PRESENT_MODE`, see
        // `requested_present_mode`).
        let requested = requested_present_mode(&capabilities.present_modes);
        let present_mode = requested.unwrap_or(wgpu::PresentMode::Fifo);
        let mut surface_configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: color_format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            desired_maximum_frame_latency: 2,
            present_mode,
            alpha_mode,
            view_formats: vec![render_format],
        };
        // A requested mode the driver refuses falls back to `Fifo` instead of
        // failing the renderer: the request is a development knob, and vsync
        // is always creatable.
        if let Err(error) = configure_first(&surface, &device, &mut surface_configuration).await {
            if surface_configuration.present_mode == wgpu::PresentMode::Fifo {
                return Err(error);
            }
            warn!(
                target: telemetry_target::RENDERING,
                "present mode {:?} was refused ({error}); falling back to Fifo", surface_configuration.present_mode
            );
            surface_configuration.present_mode = wgpu::PresentMode::Fifo;
            configure_first(&surface, &device, &mut surface_configuration).await?;
        }
        info!(target: telemetry_target::RENDERING, present_mode = ?surface_configuration.present_mode, "present mode chosen");
        Ok((
            Self {
                surface,
                surface_configuration,
                render_format,
                backend,
            },
            device,
            queue,
            instance,
            adapter,
        ))
    }

    /// Builds a surface for another window on the device the first surface
    /// created, drawing in that surface's format.
    ///
    /// The format has to match: every pass pipeline declares the first
    /// surface's format, and one surface's pipelines cannot draw into another
    /// format. A window whose surface cannot offer it is refused. The present
    /// mode follows `present`, falling back to `Fifo` where the surface does
    /// not list the preferred mode or the driver refuses it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::SurfaceCreation`] when the window's surface
    /// cannot be created or the adapter cannot present to it,
    /// [`RendererError::UnsupportedSurface`] when it cannot draw in
    /// `render_format`, [`RendererError::Backend`] when it offers no alpha
    /// mode, and the configuration errors of [`configure`].
    ///
    /// # Safety
    ///
    /// `window` must name a live window that outlives the returned surface.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn create_additional(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        window: RawWindowData,
        width: u32,
        height: u32,
        render_format: wgpu::TextureFormat,
        present: pill_renderer_api::PresentPreference,
    ) -> Result<Self> {
        // Step 1: Create the window's surface and check that the adapter can
        // present to it.
        let (raw_window_handle, raw_display_handle) = window.to_raw()?;
        // SAFETY: the handles name a live window that outlives the surface,
        // which is this function's own contract with its caller.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(raw_display_handle),
                raw_window_handle,
            })
        }
        .map_err(|error| RendererError::SurfaceCreation {
            detail: error.to_string(),
        })?;
        if !adapter.is_surface_supported(&surface) {
            return Err(RendererError::SurfaceCreation {
                detail: "the renderer's GPU cannot present to this window".to_string(),
            });
        }
        // Step 2: Find the configured format whose sRGB view is the render
        // format, and pick an alpha mode.
        let capabilities = surface.get_capabilities(adapter);
        // A configured format whose sRGB view is the render format: the same
        // format, or its plain variant on a surface without sRGB formats.
        let color_format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.add_srgb_suffix() == render_format)
            .ok_or_else(|| RendererError::UnsupportedSurface {
                detail: format!(
                    "the window offers no format drawable as {render_format:?} ({:?})",
                    capabilities.formats
                ),
            })?;
        let alpha_mode =
            pick_alpha_mode(&capabilities.alpha_modes).ok_or_else(|| RendererError::Backend {
                detail: "the surface offers no alpha mode to present with".to_owned(),
            })?;
        let present_mode = preferred_present_mode(present, &capabilities.present_modes);
        let mut surface_configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: color_format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            desired_maximum_frame_latency: 2,
            present_mode,
            alpha_mode,
            view_formats: vec![render_format],
        };
        // Step 3: Configure the surface. A preferred mode the driver refuses
        // falls back to vsync rather than failing the surface.
        if let Err(error) = configure(&surface, device, &mut surface_configuration) {
            if surface_configuration.present_mode == wgpu::PresentMode::Fifo {
                return Err(error);
            }
            warn!(
                target: telemetry_target::RENDERING,
                "present mode {:?} was refused for an additional surface ({error}); falling back to Fifo",
                surface_configuration.present_mode
            );
            surface_configuration.present_mode = wgpu::PresentMode::Fifo;
            configure(&surface, device, &mut surface_configuration)?;
        }
        Ok(Self {
            surface,
            surface_configuration,
            render_format,
            backend: adapter.get_info().backend,
        })
    }

    /// The graphics API the device behind this surface runs on.
    pub fn backend(&self) -> wgpu::Backend {
        self.backend
    }

    /// The colour format every pipeline rendering to this surface declares,
    /// and the format of the view each frame is drawn through: the configured
    /// format's sRGB variant, or the configured format itself where the
    /// surface has no sRGB variant.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.render_format
    }

    /// The configured size, in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (
            self.surface_configuration.width,
            self.surface_configuration.height,
        )
    }

    /// Whether the swapchain image's alpha reaches the compositor.
    ///
    /// This decides whether a transparent pixel means "leave this pixel to
    /// whatever else paints this window" or is simply opaque black, because
    /// only a compositing mode carries alpha out of the image. [`configure`]
    /// records the mode the surface accepted, so an `Opaque` fallback shows up
    /// here instead of being assumed away.
    pub fn composites(&self) -> bool {
        matches!(
            self.surface_configuration.alpha_mode,
            wgpu::CompositeAlphaMode::PreMultiplied | wgpu::CompositeAlphaMode::PostMultiplied
        )
    }

    /// The whole configuration, for resources sized alongside the surface.
    pub fn configuration(&self) -> &wgpu::SurfaceConfiguration {
        &self.surface_configuration
    }

    /// Reconfigures the surface for a new size.
    ///
    /// The new size goes through [`configure`], with its `Opaque` fallback, and
    /// is only committed once the surface accepted it. A refusal therefore
    /// leaves the old configuration in place, instead of one that disagrees
    /// with the swapchain.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Backend`] when every candidate configuration
    /// was refused, with each refusal collected.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> Result<()> {
        let mut candidate_configuration = self.surface_configuration.clone();
        candidate_configuration.width = width;
        candidate_configuration.height = height;
        configure(&self.surface, device, &mut candidate_configuration)?;
        self.surface_configuration = candidate_configuration;
        Ok(())
    }

    /// Acquires the next frame, recovering a lost or outdated swapchain once.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Backend`] when the swapchain is still lost or
    /// outdated after one reconfiguration, and again with wgpu's status for any
    /// other acquisition failure.
    pub fn acquire(&mut self, device: &wgpu::Device) -> Result<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
            // A lost or outdated swapchain is recovered by reconfiguring and
            // retrying once; a second failure is reported, because waiting for
            // a later resize event leaves the renderer dead until one arrives.
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.reconfigure(device)?;
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
                    wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                        Err(RendererError::Backend {
                            detail: "the swapchain stayed lost or outdated after reconfiguring"
                                .to_owned(),
                        })
                    }
                    surface_status => Err(RendererError::Backend {
                        detail: format!("no surface texture: {surface_status:?}"),
                    }),
                }
            }
            surface_status => Err(RendererError::Backend {
                detail: format!("no surface texture: {surface_status:?}"),
            }),
        }
    }

    /// Reconfigures the surface from its current settings.
    ///
    /// The recovery path for a lost or outdated swapchain: the configuration is
    /// still what the window wants, only the platform-side surface needs
    /// recreating.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Backend`] when every candidate alpha mode was
    /// refused, as [`configure`] reports it.
    fn reconfigure(&mut self, device: &wgpu::Device) -> Result<()> {
        configure(&self.surface, device, &mut self.surface_configuration)
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The present mode `PILL_PRESENT_MODE` asks for, when the surface supports
/// it.
///
/// `immediate` presents without waiting, so the picture can tear. `mailbox`
/// also presents without waiting, but replaces a queued frame instead of
/// tearing. `fifo` is vsync, and is the default. Returns `None` when the
/// variable is unset, when it names an unknown mode, and when the surface does
/// not support the mode it names; both rejections are logged.
fn requested_present_mode(
    available_present_modes: &[wgpu::PresentMode],
) -> Option<wgpu::PresentMode> {
    let requested = std::env::var(PRESENT_MODE_VARIABLE).ok()?;
    let present_mode = match requested.trim().to_ascii_lowercase().as_str() {
        "immediate" => wgpu::PresentMode::Immediate,
        "mailbox" => wgpu::PresentMode::Mailbox,
        "fifo" => wgpu::PresentMode::Fifo,
        _ => {
            warn!(
                target: telemetry_target::RENDERING,
                "{PRESENT_MODE_VARIABLE}={requested} is not a present mode (immediate, mailbox, fifo); using Fifo"
            );
            return None;
        }
    };
    if !available_present_modes.contains(&present_mode) {
        warn!(
            target: telemetry_target::RENDERING,
            available = %available_present_modes
                .iter()
                .map(|mode| format!("{mode:?}"))
                .collect::<Vec<_>>()
                .join(", "),
            "{PRESENT_MODE_VARIABLE} asked for {present_mode:?}, which this surface does not offer; using Fifo"
        );
        return None;
    }
    Some(present_mode)
}

/// The present mode a surface asking for `present` gets: that mode when the
/// surface supports it, vsync otherwise.
fn preferred_present_mode(
    present: pill_renderer_api::PresentPreference,
    available_present_modes: &[wgpu::PresentMode],
) -> wgpu::PresentMode {
    let preferred = match present {
        pill_renderer_api::PresentPreference::Vsync => wgpu::PresentMode::Fifo,
        pill_renderer_api::PresentPreference::LowLatency => wgpu::PresentMode::Mailbox,
        pill_renderer_api::PresentPreference::Immediate => wgpu::PresentMode::Immediate,
    };
    if available_present_modes.contains(&preferred) {
        preferred
    } else {
        wgpu::PresentMode::Fifo
    }
}

/// The configured colour format to ask a surface for.
///
/// An sRGB format where the surface offers one, Rgba ahead of Bgra because it
/// needs no channel swizzle, and otherwise the first format the surface
/// advertises, since a poor format beats refusing to start.
fn pick_color_format(available_formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    [
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Bgra8Unorm,
    ]
    .into_iter()
    .find(|format| available_formats.contains(format))
    .or_else(|| available_formats.first().copied())
}

/// The alpha mode to ask a new surface for.
///
/// A compositing mode comes first because the editor paints its panels over
/// the same window the scene viewport lives in. That needs the swapchain image
/// to carry alpha, which a window created transparent can provide. The
/// renderer's own shaders write opaque pixels, so a window with nothing to
/// show through stays opaque either way, and [`configure`] and
/// [`configure_first`] fall back to `Opaque` when the compositor refuses.
fn pick_alpha_mode(
    available_alpha_modes: &[wgpu::CompositeAlphaMode],
) -> Option<wgpu::CompositeAlphaMode> {
    [
        wgpu::CompositeAlphaMode::PreMultiplied,
        wgpu::CompositeAlphaMode::PostMultiplied,
    ]
    .into_iter()
    .find(|mode| available_alpha_modes.contains(mode))
    .or_else(|| available_alpha_modes.first().copied())
}

/// The alpha modes to try, in order: the requested one, then `Opaque`.
///
/// A compositing alpha mode needs a compositing window, and a capability list
/// does not promise one. `Opaque` is the mode every surface must support, so
/// it is the fallback. When `Opaque` was requested, there is no second
/// candidate.
fn alpha_mode_candidates(
    requested_alpha_mode: wgpu::CompositeAlphaMode,
) -> Vec<wgpu::CompositeAlphaMode> {
    let mut alpha_modes = vec![requested_alpha_mode];
    if requested_alpha_mode != wgpu::CompositeAlphaMode::Opaque {
        alpha_modes.push(wgpu::CompositeAlphaMode::Opaque);
    }
    alpha_modes
}

/// Configures the surface for the first time, giving up the compositing alpha
/// mode if the driver refuses it.
///
/// `Surface::configure` returns nothing: a refused configuration only reaches
/// the device's error handler, which logs it, so the call would look like it
/// succeeded. A refusal is realistic because a compositing alpha mode needs a
/// compositing window, which a capability list does not promise. The requested
/// mode is therefore tried inside its own error scopes, and `Opaque`, the mode
/// every surface must support, is the fallback. The scopes are awaited: the
/// renderer is being built asynchronously, so every target can wait for them.
///
/// # Errors
///
/// Returns [`RendererError::Backend`] when the driver refused the
/// configuration in every candidate alpha mode, with each refusal in the text.
async fn configure_first(
    surface: &wgpu::Surface<'static>,
    device: &wgpu::Device,
    surface_configuration: &mut wgpu::SurfaceConfiguration,
) -> Result<()> {
    let mut failures = Vec::new();
    for alpha_mode in alpha_mode_candidates(surface_configuration.alpha_mode) {
        surface_configuration.alpha_mode = alpha_mode;
        let (probe_outcome, errors) = captured(device, || {
            apply_and_probe(surface, device, surface_configuration)
        })
        .await;
        match configuration_attempt_failure(probe_outcome, errors) {
            None => {
                info!(target: telemetry_target::RENDERING, alpha_mode = ?alpha_mode, "surface configured");
                return Ok(());
            }
            Some(failure) => failures.push(format!("{alpha_mode:?} ({failure})")),
        }
    }
    Err(RendererError::Backend {
        detail: format!(
            "no surface configuration was accepted by the driver: {}",
            failures.join("; ")
        ),
    })
}

/// Reconfigures the surface during a frame (on a resize, or for a lost
/// swapchain), with the same `Opaque` fallback as [`configure_first`].
///
/// A frame cannot await, so the error scopes are only blocked on in a native
/// development build. Elsewhere a refusal reaches the device's error handler,
/// which logs it, and the frame probe still reports it here. The alpha mode
/// reused is the one the first configuration proved, so this is the rare path.
///
/// # Errors
///
/// Returns [`RendererError::Backend`] when the driver refused the
/// configuration in every alpha mode it tried, with each refusal in the text.
fn configure(
    surface: &wgpu::Surface<'static>,
    device: &wgpu::Device,
    surface_configuration: &mut wgpu::SurfaceConfiguration,
) -> Result<()> {
    let mut failures = Vec::new();
    for alpha_mode in alpha_mode_candidates(surface_configuration.alpha_mode) {
        surface_configuration.alpha_mode = alpha_mode;
        let (probe_outcome, errors) = captured_now(device, || {
            apply_and_probe(surface, device, surface_configuration)
        });
        match configuration_attempt_failure(probe_outcome, errors) {
            None => return Ok(()),
            Some(failure) => failures.push(format!("{alpha_mode:?} ({failure})")),
        }
    }
    Err(RendererError::Backend {
        detail: format!(
            "no surface configuration was accepted by the driver: {}",
            failures.join("; ")
        ),
    })
}

/// Configures the surface and acquires a frame from it.
///
/// Acquiring a frame is what creates the swapchain for real: wgpu-core
/// accepting the configuration does not mean the driver created one, and a
/// refusal only shows up here.
///
/// # Errors
///
/// Returns the swapchain status as text when no frame could be acquired.
fn apply_and_probe(
    surface: &wgpu::Surface<'static>,
    device: &wgpu::Device,
    surface_configuration: &wgpu::SurfaceConfiguration,
) -> std::result::Result<wgpu::SurfaceTexture, String> {
    surface.configure(device, surface_configuration);
    match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
        other => Err(format!("{other:?}")),
    }
}

/// Why one configuration attempt failed, or `None` when it succeeded: a
/// captured error first, else the probe's own failure.
fn configuration_attempt_failure(
    probe_outcome: std::result::Result<wgpu::SurfaceTexture, String>,
    errors: CapturedErrors,
) -> Option<String> {
    let reported_error = errors
        .internal
        .or(errors.out_of_memory)
        .or(errors.validation)
        .map(|error| error.to_string());
    match probe_outcome {
        Ok(frame) => {
            // Dropped rather than presented: the frame loop acquires its own.
            drop(frame);
            reported_error
        }
        Err(error) => Some(reported_error.unwrap_or(error)),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // External crates
    use pill_renderer_api::PresentPreference;

    // Current crate
    use super::*;

    /// A preferred present mode is used when the surface lists it, and vsync
    /// otherwise.
    #[test]
    fn a_present_preference_falls_back_to_vsync() {
        let all_present_modes = [
            wgpu::PresentMode::Fifo,
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::Immediate,
        ];
        assert_eq!(
            preferred_present_mode(PresentPreference::LowLatency, &all_present_modes),
            wgpu::PresentMode::Mailbox
        );
        assert_eq!(
            preferred_present_mode(PresentPreference::Immediate, &[wgpu::PresentMode::Fifo]),
            wgpu::PresentMode::Fifo
        );
        assert_eq!(
            preferred_present_mode(PresentPreference::Vsync, &all_present_modes),
            wgpu::PresentMode::Fifo
        );
    }

    #[test]
    fn an_srgb_format_is_preferred_where_the_surface_offers_one() {
        assert_eq!(
            pick_color_format(&[
                wgpu::TextureFormat::Bgra8Unorm,
                wgpu::TextureFormat::Bgra8UnormSrgb,
            ]),
            Some(wgpu::TextureFormat::Bgra8UnormSrgb)
        );
    }

    #[test]
    fn rgba_is_preferred_over_bgra_because_it_needs_no_swizzle() {
        assert_eq!(
            pick_color_format(&[
                wgpu::TextureFormat::Bgra8UnormSrgb,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ]),
            Some(wgpu::TextureFormat::Rgba8UnormSrgb)
        );
    }

    #[test]
    fn a_surface_offering_none_of_the_preferred_formats_still_starts() {
        assert_eq!(
            pick_color_format(&[wgpu::TextureFormat::Rgb10a2Unorm]),
            Some(wgpu::TextureFormat::Rgb10a2Unorm),
            "the first advertised format beats refusing to start"
        );
        assert_eq!(pick_color_format(&[]), None, "no formats is an error");
    }

    #[test]
    fn a_compositing_alpha_mode_is_preferred_where_the_surface_offers_one() {
        assert_eq!(
            pick_alpha_mode(&[
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::Auto,
                wgpu::CompositeAlphaMode::PreMultiplied,
            ]),
            Some(wgpu::CompositeAlphaMode::PreMultiplied),
            "the scene shares its window with the editor's panels, so the image has to carry alpha"
        );
        assert_eq!(
            pick_alpha_mode(&[
                wgpu::CompositeAlphaMode::Auto,
                wgpu::CompositeAlphaMode::PostMultiplied,
            ]),
            Some(wgpu::CompositeAlphaMode::PostMultiplied),
            "the other compositing mode is taken when it is the only one on offer"
        );
        assert_eq!(
            pick_alpha_mode(&[wgpu::CompositeAlphaMode::Opaque]),
            Some(wgpu::CompositeAlphaMode::Opaque)
        );
        assert_eq!(
            pick_alpha_mode(&[]),
            None,
            "no alpha modes advertised is an error, not a mode"
        );
    }

    #[test]
    fn opaque_is_the_only_alpha_fallback_and_is_not_retried_against_itself() {
        assert_eq!(
            alpha_mode_candidates(wgpu::CompositeAlphaMode::Auto),
            [
                wgpu::CompositeAlphaMode::Auto,
                wgpu::CompositeAlphaMode::Opaque,
            ]
        );
        assert_eq!(
            alpha_mode_candidates(wgpu::CompositeAlphaMode::Opaque),
            [wgpu::CompositeAlphaMode::Opaque]
        );
    }
}
