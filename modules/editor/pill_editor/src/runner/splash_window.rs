//! The splash the editor draws while the host builds and loads the project.
//!
//! # Responsibilities
//!
//! - [`SplashWindow`]: one egui pass of the splash, presented through the same
//!   overlay the editor's own UI uses.
//! - [`progress`]: the host's published progress, turned into the splash's own
//!   plain data.
//!
//! # Design
//!
//! The host's start blocks in a cargo wait, so this draws from inside it: the
//! host calls a tick once per poll, and [`SplashWindow::draw`] runs one pass
//! and presents it. It owns the overlay and the egui renderer while the start
//! runs - the tick it hands the host must be `'static` - and
//! [`SplashWindow::into_ui_parts`] hands both back for the editor's own frame.
//!
//! It reads the host's snapshot every frame rather than being told when to
//! change: the snapshot is plain data behind a lock, and a frame that is one
//! poll behind is invisible.
//!
//! Nothing else of the editor runs here - no shell frame, no project world -
//! so a splash frame cannot depend on state the host has not built yet.

// The names the runner shares: the overlay, the GPU types and the log macros.
use super::*;

// =============================================================================
// Data
// =============================================================================

/// How long the start-up splash waits before its first frame: none at all.
///
/// The window is already dark while the host runs, so the splash is strictly
/// better than what it covers.
pub(super) const STARTUP_SPLASH_DELAY_SECONDS: f32 = 0.0;

/// How long the closing splash waits before its first frame.
///
/// A quit that releases its modules in a couple of frames would only flash
/// the splash, and a flash reads as a glitch next to a plain quit. Measured
/// teardown of a loaded editor releases its modules in about 100 ms, so this
/// is the smallest delay that still keeps a trivial quit looking like one.
pub(super) const CLOSING_SPLASH_DELAY_SECONDS: f32 = 0.06;

// =============================================================================
// Types
// =============================================================================

/// The splash, drawn through the editor's overlay while the host starts.
pub(super) struct SplashWindow {
    context: egui::Context,
    started: pill_core::platform::Instant,
    size: (u32, u32),
    scale_factor: f32,
    overlay: Option<OverlaySurface>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    show_after_seconds: f32,
    reported: bool,
}

impl SplashWindow {
    /// A splash for the window `size` at `scale_factor`, drawing through
    /// `overlay`.
    ///
    /// `show_after_seconds` suppresses the first frames: a quit that releases
    /// its modules in milliseconds is better left looking like a quit than
    /// flashing a splash.
    pub(super) fn new(
        size: (u32, u32),
        scale_factor: f32,
        overlay: Option<OverlaySurface>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        renderer: egui_wgpu::Renderer,
        show_after_seconds: f32,
    ) -> Self {
        Self {
            context: egui::Context::default(),
            started: pill_core::platform::Instant::now(),
            size,
            scale_factor,
            overlay,
            device,
            queue,
            renderer,
            show_after_seconds,
            reported: false,
        }
    }

    /// The overlay and the egui renderer, back for the editor's frame.
    pub(super) fn into_ui_parts(self) -> (Option<OverlaySurface>, egui_wgpu::Renderer) {
        (self.overlay, self.renderer)
    }

    pub(super) fn showed(&self) -> bool {
        self.reported
    }

    /// Runs one splash pass and presents it.
    ///
    /// A frame that fails to present is logged and dropped: the splash is a
    /// courtesy, and the start must not stop because it could not draw.
    pub(super) fn draw(&mut self) {
        if self.started.elapsed().as_secs_f32() < self.show_after_seconds {
            return;
        }
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        let progress = splash_progress(self.started.elapsed().as_secs_f32());
        let size_in_points = egui::vec2(
            self.size.0 as f32 / self.scale_factor,
            self.size.1 as f32 / self.scale_factor,
        );
        let mut raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size_in_points)),
            time: Some(f64::from(progress.elapsed_seconds)),
            ..egui::RawInput::default()
        };
        raw_input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(self.scale_factor);
        let output = self.context.run_ui(raw_input, |ui| {
            pill_editor_core::splash::show(ui, &progress);
        });
        let frame = pill_editor_core::boundary::frame_from_output(&self.context, output);
        if let Err(error) = overlay.render(
            &self.device,
            &self.queue,
            &mut self.renderer,
            &frame.meshes,
            frame.textures,
            frame.pixels_per_point,
        ) {
            warn!(
                target: telemetry_target::EDITOR,
                error = %error,
                "[editor] splash frame failed"
            );
            return;
        }
        if !self.reported {
            self.reported = true;
            info!(
                target: telemetry_target::EDITOR,
                posture = posture_name(progress.posture),
                modules = progress.modules.len(),
                "[editor] splash shown"
            );
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The splash's posture, as the log spells it.
fn posture_name(posture: pill_editor_core::splash::SplashPosture) -> &'static str {
    match posture {
        pill_editor_core::splash::SplashPosture::Loading => "loading",
        pill_editor_core::splash::SplashPosture::Closing => "closing",
    }
}

/// The splash's data, from the host's published progress and the time the
/// splash has been showing.
fn splash_progress(elapsed_seconds: f32) -> pill_editor_core::splash::SplashProgress {
    use pill_editor_core::splash::{SplashModule, SplashPosture, SplashProgress, SplashStep};

    let snapshot = pill_development_host::startup_progress::snapshot();
    let modules = snapshot
        .modules
        .iter()
        .map(|module| SplashModule {
            name: module.name.clone(),
            state: module_state(module.state, snapshot.closing),
        })
        .collect();
    SplashProgress {
        posture: if snapshot.closing {
            SplashPosture::Closing
        } else {
            SplashPosture::Loading
        },
        modules,
        active: snapshot.active,
        step: match snapshot.step {
            Some(pill_development_host::startup_progress::StartupStep::Building) | None => {
                SplashStep::Building
            }
            Some(pill_development_host::startup_progress::StartupStep::Loading) => {
                SplashStep::Loading
            }
        },
        elapsed_seconds,
    }
}

/// One bar section's state: a start fills it when the module has loaded, a
/// quit fills it when the module has been released.
fn module_state(
    state: pill_development_host::startup_progress::StartupModuleState,
    closing: bool,
) -> pill_editor_core::splash::SplashModuleState {
    use pill_development_host::startup_progress::StartupModuleState;
    use pill_editor_core::splash::SplashModuleState;

    match (closing, state) {
        (false, StartupModuleState::Loaded) | (true, StartupModuleState::Released) => {
            SplashModuleState::Done
        }
        (false, StartupModuleState::Building | StartupModuleState::Loading)
        | (true, StartupModuleState::Unloading) => SplashModuleState::Working,
        _ => SplashModuleState::Pending,
    }
}
