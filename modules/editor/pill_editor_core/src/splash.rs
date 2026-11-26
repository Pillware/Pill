//! The start-up and quit splash: a spinner, a bar of one section per module,
//! and the line naming the module being worked on.
//!
//! # Responsibilities
//!
//! - [`SplashProgress`] and its parts: the posture (`Loading` or `Closing`),
//!   one [`SplashModule`] per module the host plans to build or release, and
//!   which of them is being worked on right now.
//! - [`show`]: paint the panel, the spinner, the bar and the message into a
//!   `Ui`, and hand back the geometry it used.
//! - [`message`] and [`spinner_angle`]: the words and the rotation, as plain
//!   functions so a test reads them without a texture.
//!
//! # Design
//!
//! The splash is plain data plus one painter, so the executable decides when to
//! draw it and where the data comes from. It has no notion of a module type, a
//! host or a thread: the host publishes a snapshot of the same shape
//! (name and state per module) and the executable copies it across.
//!
//! Every colour comes from [`crate::style`], so the panel and the bar follow
//! the theme like the rest of the editor. A finished section is the theme's
//! red (`danger`), the colour the bar was asked for; a section not started is
//! the recessed `well` surface.

// External crates
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, TextStyle, Ui, Vec2};

// Current crate
use crate::style;

// =============================================================================
// Constants
// =============================================================================

/// The panel's width, in points. The splash keeps one size whatever the window
/// is, so a growing window does not move the bar under the pointer.
pub const PANEL_WIDTH: f32 = 420.0;
/// The panel's height, in points: the spinner, the bar and the message with
/// the padding they need.
pub const PANEL_HEIGHT: f32 = 190.0;
/// The panel's corner radius, in points.
pub const PANEL_CORNER: u8 = 10;
/// The spinner's radius, in points.
pub const SPINNER_RADIUS: f32 = 20.0;
/// How many segments the spinner's arc is drawn from.
pub const SPINNER_SEGMENTS: usize = 12;
/// How fast the spinner turns, in radians per second.
pub const SPINNER_SPEED: f32 = 3.0;
/// The bar's width, in points.
pub const BAR_WIDTH: f32 = PANEL_WIDTH - 64.0;
/// The bar's height, and so the corner radius of its sections, in points.
pub const BAR_HEIGHT: f32 = 10.0;
/// The gap between two sections, in points.
pub const SECTION_GAP: f32 = 4.0;
/// The gap between the spinner and the bar, and between the bar and the
/// message, in points.
pub const BAR_GAP: f32 = 22.0;
pub const MESSAGE_GAP: f32 = 18.0;

// =============================================================================
// Types
// =============================================================================

/// What the splash is doing: loading a project or releasing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplashPosture {
    /// The editor is starting: the modules are being built and loaded.
    Loading,
    /// The editor is quitting: the modules are being released.
    Closing,
}

/// What is happening to the module the bar is working on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplashStep {
    /// The module's sources are being compiled.
    Building,
    /// Its artifact is being loaded into the process.
    Loading,
}

/// How far one module has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplashModuleState {
    /// Planned, and nothing has happened to it yet.
    Pending,
    /// Being built, loaded or released right now.
    Working,
    /// Built and loaded, or released: its section is filled.
    Done,
}

/// One module, as the bar draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplashModule {
    /// The module's name, as the host reports it.
    pub name: String,
    /// How far it has got.
    pub state: SplashModuleState,
}

/// Everything the splash draws.
#[derive(Debug, Clone, PartialEq)]
pub struct SplashProgress {
    /// Whether the editor is starting or quitting.
    pub posture: SplashPosture,
    /// One entry per module the host plans to work through, in its order.
    pub modules: Vec<SplashModule>,
    /// The index of the module being worked on, if any.
    pub active: Option<usize>,
    /// What is being done to it.
    pub step: SplashStep,
    /// How long the splash has been showing, in seconds: what turns the
    /// spinner.
    pub elapsed_seconds: f32,
}

/// The rectangles one painted splash occupied.
#[derive(Debug, Clone, PartialEq)]
pub struct SplashLayout {
    /// The panel the splash drew on.
    pub panel: Rect,
    /// The bar, as drawn.
    pub bar: Rect,
    /// One rectangle per module, in the bar's order.
    pub sections: Vec<Rect>,
    /// The line of text under the bar.
    pub message: String,
}

// =============================================================================
// Data
// =============================================================================

impl SplashProgress {
    /// The splash before the host has planned anything: no sections, and a
    /// message that says the editor is starting.
    #[must_use]
    pub fn starting() -> Self {
        Self {
            posture: SplashPosture::Loading,
            modules: Vec::new(),
            active: None,
            step: SplashStep::Loading,
            elapsed_seconds: 0.0,
        }
    }

    /// The module being worked on, if any.
    #[must_use]
    pub fn active_module(&self) -> Option<&SplashModule> {
        self.active.and_then(|index| self.modules.get(index))
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The line under the bar: the module being worked on, or what the editor is
/// doing when none is.
#[must_use]
pub fn message(progress: &SplashProgress) -> String {
    let posture = progress.posture;
    let Some(module) = progress.active_module() else {
        return match posture {
            SplashPosture::Loading => "starting the editor".to_owned(),
            SplashPosture::Closing => "closing the editor".to_owned(),
        };
    };
    let verb = match (posture, progress.step) {
        (SplashPosture::Closing, _) => "unloading",
        (SplashPosture::Loading, SplashStep::Building) => "building",
        (SplashPosture::Loading, SplashStep::Loading) => "loading",
    };
    format!("{} {} module", verb, module.name)
}

/// How far the spinner has turned, in radians, after `elapsed_seconds`.
#[must_use]
pub fn spinner_angle(elapsed_seconds: f32) -> f32 {
    elapsed_seconds.max(0.0) * SPINNER_SPEED
}

/// The colour a section of the bar is filled with.
#[must_use]
pub fn section_color(state: SplashModuleState) -> Color32 {
    match state {
        SplashModuleState::Done => style::danger(),
        SplashModuleState::Pending | SplashModuleState::Working => style::well(),
    }
}

/// Paints the splash into `ui`, and answers with the geometry it used.
pub fn show(ui: &mut Ui, progress: &SplashProgress) -> SplashLayout {
    let area = ui.available_rect_before_wrap();
    let panel = Rect::from_center_size(area.center(), Vec2::new(PANEL_WIDTH, PANEL_HEIGHT));
    let painter = ui.painter();
    painter.rect_filled(panel, CornerRadius::same(PANEL_CORNER), style::panel());
    let spinner_center = Pos2::new(panel.center().x, panel.min.y + 46.0);
    draw_spinner(
        painter,
        spinner_center,
        spinner_angle(progress.elapsed_seconds),
    );
    let bar = Rect::from_center_size(
        Pos2::new(
            panel.center().x,
            spinner_center.y + SPINNER_RADIUS + BAR_GAP,
        ),
        Vec2::new(BAR_WIDTH, BAR_HEIGHT),
    );
    let sections = draw_bar(painter, bar, progress);
    let message = message(progress);
    let message_position = Pos2::new(panel.center().x, bar.max.y + MESSAGE_GAP);
    painter.text(
        message_position,
        Align2::CENTER_TOP,
        &message,
        TextStyle::Body.resolve(ui.style()),
        style::text(),
    );
    // The splash owns the whole window while it shows, so it takes the pointer
    // and keeps the widgets behind it from reacting.
    ui.allocate_rect(area, Sense::hover());
    SplashLayout {
        panel,
        bar,
        sections,
        message,
    }
}

/// Draws the spinner: a ring of segments, brightest at the head of the arc.
fn draw_spinner(painter: &egui::Painter, center: Pos2, angle: f32) {
    for segment in 0..SPINNER_SEGMENTS {
        let along = segment as f32 / SPINNER_SEGMENTS as f32;
        let start = angle + along * std::f32::consts::TAU;
        let end = start + std::f32::consts::TAU / (SPINNER_SEGMENTS as f32 * 1.8);
        let radius = SPINNER_RADIUS;
        let from = center + Vec2::angled(start) * radius;
        let to = center + Vec2::angled(end) * radius;
        // The head of the arc is the accent, and the tail fades to the well.
        let colour = style::mix(style::accent(), style::well(), along);
        painter.add(Shape::line_segment([from, to], (2.0, colour)));
    }
}

/// Draws the bar and answers with one rectangle per module.
fn draw_bar(painter: &egui::Painter, bar: Rect, progress: &SplashProgress) -> Vec<Rect> {
    let count = progress.modules.len();
    if count == 0 {
        painter.rect_filled(
            bar,
            CornerRadius::same((BAR_HEIGHT / 2.0) as u8),
            style::well(),
        );
        return Vec::new();
    }
    let total_gap = SECTION_GAP * (count.saturating_sub(1)) as f32;
    let section_width = (bar.width() - total_gap) / count as f32;
    let corner = CornerRadius::same((BAR_HEIGHT / 2.0) as u8);
    let mut sections = Vec::with_capacity(count);
    for (index, module) in progress.modules.iter().enumerate() {
        let left = bar.min.x + index as f32 * (section_width + SECTION_GAP);
        let section = Rect::from_min_size(
            Pos2::new(left, bar.min.y),
            Vec2::new(section_width, bar.height()),
        );
        painter.rect_filled(section, corner, section_color(module.state));
        sections.push(section);
    }
    sections
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, Context, RawInput, Rect};

    fn module(name: &str, state: SplashModuleState) -> SplashModule {
        SplashModule {
            name: name.to_owned(),
            state,
        }
    }

    /// Paints one splash and answers with its layout.
    fn painted(progress: &SplashProgress) -> SplashLayout {
        let context = Context::default();
        let mut layout = None;
        let mut output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 600.0))),
                ..RawInput::default()
            },
            |ui| layout = Some(show(ui, progress)),
        );
        output.textures_delta.clear();
        layout.expect("the splash painted")
    }

    fn loading(modules: Vec<SplashModule>, active: Option<usize>) -> SplashProgress {
        SplashProgress {
            posture: SplashPosture::Loading,
            modules,
            active,
            step: SplashStep::Building,
            elapsed_seconds: 0.0,
        }
    }

    #[test]
    fn the_bar_has_one_section_per_module() {
        let progress = loading(
            vec![
                module("pill_rendering", SplashModuleState::Done),
                module("pill_renderer", SplashModuleState::Working),
                module("scene", SplashModuleState::Pending),
            ],
            Some(1),
        );
        let layout = painted(&progress);
        assert_eq!(layout.sections.len(), 3, "one section per module");
        let widths: Vec<f32> = layout.sections.iter().map(Rect::width).collect();
        assert!((widths[0] - widths[1]).abs() < 0.01 && (widths[1] - widths[2]).abs() < 0.01);
        assert!(layout.sections[0].min.x < layout.sections[1].min.x);
        assert!(layout.sections[2].max.x <= layout.bar.max.x + 0.01);
        for section in &layout.sections {
            assert!((section.height() - BAR_HEIGHT).abs() < 0.01);
            assert!(layout.bar.contains_rect(*section));
        }
    }

    #[test]
    fn a_done_section_is_the_themes_red_and_a_pending_one_the_well() {
        assert_eq!(section_color(SplashModuleState::Done), style::danger());
        assert_eq!(section_color(SplashModuleState::Pending), style::well());
        assert_eq!(section_color(SplashModuleState::Working), style::well());
    }

    #[test]
    fn the_message_names_the_module_and_the_verb() {
        let mut progress = loading(
            vec![
                module("pill_rendering", SplashModuleState::Done),
                module("pill_renderer", SplashModuleState::Working),
            ],
            Some(1),
        );
        assert_eq!(message(&progress), "building pill_renderer module");
        progress.step = SplashStep::Loading;
        assert_eq!(message(&progress), "loading pill_renderer module");
        progress.posture = SplashPosture::Closing;
        assert_eq!(message(&progress), "unloading pill_renderer module");
        progress.active = None;
        assert_eq!(message(&progress), "closing the editor");
    }

    #[test]
    fn a_start_with_no_plan_says_the_editor_is_starting() {
        assert_eq!(message(&SplashProgress::starting()), "starting the editor");
        assert!(SplashProgress::starting().modules.is_empty());
    }

    #[test]
    fn the_spinner_turns_with_time() {
        assert_eq!(spinner_angle(0.0), 0.0);
        assert!(spinner_angle(0.5) > spinner_angle(0.2));
        assert_eq!(spinner_angle(-1.0), 0.0, "time never runs backwards");
    }

    #[test]
    fn the_panel_is_centred_and_holds_its_pieces_in_order() {
        let progress = loading(vec![module("scene", SplashModuleState::Working)], Some(0));
        let layout = painted(&progress);
        assert!((layout.panel.center().x - 400.0).abs() < 0.01);
        assert!((layout.panel.center().y - 300.0).abs() < 0.01);
        assert!(layout.bar.min.y > layout.panel.min.y);
        assert!(layout.bar.max.y < layout.panel.max.y);
        assert_eq!(layout.sections.len(), 1);
        assert_eq!(layout.sections[0], layout.bar);
    }
}
