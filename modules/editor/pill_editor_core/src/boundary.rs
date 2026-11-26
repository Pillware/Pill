//! The plain data the egui frontend and the reloadable egui shell exchange.
//!
//! # Responsibilities
//!
//! - [`EguiShellInput`]: what the frontend writes for the shell each frame.
//!   That is the window's size and scale, its focus, time and frame rate, the
//!   window's events, the cursor and paste text, and the scene badges.
//! - [`EguiShellFrame`]: what the shell hands back. That is the tessellated
//!   meshes, the texture uploads, the interaction state, the cursor and
//!   clipboard text, the viewport rectangles and the requests the pass raised.
//!   The frontend drains it.
//! - [`frame_from_output`]: builds the frame from egui's own output and drops
//!   paint callbacks, because only plain data crosses the boundary.
//!
//! # Design
//!
//! Everything that crosses is plain data: no handles, no device objects and no
//! callbacks. The shell's input events travel in [`EguiShellInput`] as owned
//! [`InputEvent`]s, so the UI reads the events of its own windows and never
//! the game's.

// Standard library
use std::time::Duration;

// External crates
use egui::epaint::ClippedPrimitive;
use egui::{Context, CursorIcon, FullOutput, OutputCommand, TexturesDelta};
use pill_core::math::Vector2f;
use pill_engine::input::InputEvent;
use pill_engine::Resource;

// Current crate
use crate::requests::ShellRequests;

// =============================================================================
// Constants
// =============================================================================

/// How many badges one scene view draws: enough that a crowded scene still
/// shows where its entities are, few enough that the marks never cover the
/// viewport.
pub const BADGES_PER_VIEW_MAX: usize = 48;

/// How many scene views the frontend opens at startup: the two the default
/// dock carries. The shell reports how many view slots it needs, and the
/// frontend opens one view for each new slot.
pub const DEFAULT_SCENE_VIEWS: usize = 2;

/// The most scene views one editor opens. Each view needs a child window and a
/// renderer surface, so the frontend opens at most this many windows even when
/// the dock holds more scene tabs.
pub const SCENE_VIEWS_MAX: usize = 8;

// =============================================================================
// Types
// =============================================================================

/// Everything the frontend tells the shell about the frame it is about to
/// draw.
#[derive(Clone, Debug)]
pub struct EguiShellInput {
    /// Logical size (points) of the editor's main window client area.
    pub screen_size: Vector2f,
    /// Physical pixels per logical point.
    pub scale_factor: f32,
    /// Whether the editor window has keyboard focus.
    pub focused: bool,
    /// Whether the editor window is maximized.
    ///
    /// The window has no system frame: the shell draws the title strip and its
    /// maximize button, so only the executable can say which glyph that button
    /// shows. A maximized window draws the restore glyph and cannot be resized
    /// from its edges.
    pub maximized: bool,
    /// Whether the shell draws the window's title strip.
    ///
    /// The native editor's window has no system frame, so the shell draws the
    /// window buttons, the drag zones and the resize bands. A frontend whose
    /// window somebody else owns - the browser editor's page - sets this to
    /// `false` and draws none of them.
    pub has_caption: bool,
    /// The project's own name, as its settings file spells it.
    ///
    /// The window has no system title bar, so the strip's right end names the
    /// project where a title would be. Shared rather than copied per frame: it
    /// is fixed for the run.
    pub project_name: std::sync::Arc<str>,
    /// Seconds since the editor started, monotonic.
    pub time: f64,
    /// The editor's smoothed frame rate, for the shell's overlays.
    pub frames_per_second: f32,
    /// The events the editor's own windows reported since the last frame, in
    /// the main window's client coordinates.
    ///
    /// The game's window is not in here. Its events go to the engine's input
    /// instead, so a project system never sees a keystroke meant for the UI.
    pub events: Vec<InputEvent>,
    /// The pointer's last known position in the same coordinates, or `None`
    /// when the pointer is not over the editor's windows.
    ///
    /// egui button events carry their own position, and the input bridge needs
    /// one before the first move of a frame, because a release can arrive with
    /// no move in front of it. This value gives it that starting point.
    pub cursor: Option<Vector2f>,
    /// The OS clipboard's text, when this frame's events include a paste
    /// chord.
    ///
    /// egui pastes only from an [`egui::Event::Paste`], and only the frontend
    /// can read the OS clipboard. It reads the clipboard once per paste chord
    /// and the input bridge appends the event here; without it, a focused text
    /// field would ignore Ctrl+V.
    pub paste_text: Option<String>,
    /// The entities the executable projected into this frame's scene views,
    /// one entry per entity and view.
    ///
    /// The executable owns the views' cameras and the renderer's projection, so
    /// only it can say where an entity appears on screen. The shell owns the
    /// icon atlas and draws the badge on that point.
    pub badges: Vec<ShellBadge>,
    /// The marquee a drag in a scene view is holding open, when one is.
    ///
    /// The executable owns the gesture and the pixels it covers; the shell
    /// draws the rectangle over the viewport, which is the only surface the
    /// composition overlay paints above a scene's child window.
    pub marquee: Option<ShellMarquee>,
    /// Whether a scripted channel drives this run.
    ///
    /// The Inspector logs a rectangle for every row so a suite can drop an
    /// asset onto one without reading pixels. A person reading the console gets
    /// nothing from those lines, so only a scripted run turns them on.
    pub scripted: bool,
}

/// One entity's badge in one scene view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellBadge {
    /// The scene view the badge belongs to, as a view index.
    pub view: usize,
    /// Where the entity projects to along x, in the main window's logical
    /// points.
    pub x: f32,
    /// Where the entity projects to along y, in the main window's logical
    /// points.
    pub y: f32,
    /// The entity the badge names; a click selects it.
    pub entity: pill_engine::Entity,
}

/// The rectangle a marquee drag covers, in one scene view.
///
/// The rectangle is in the main window's logical points, like a badge, because
/// that is the space the shell lays its overlay out in. The executable converts
/// the view's own pixels into it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellMarquee {
    /// The scene view the drag belongs to, as a view index.
    pub view: usize,
    /// The drag's rectangle, in the main window's logical points.
    pub rect: ShellRect,
}

impl Resource for EguiShellInput {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

impl Default for EguiShellInput {
    fn default() -> Self {
        Self {
            screen_size: Vector2f::new(1.0, 1.0),
            scale_factor: 1.0,
            focused: true,
            maximized: false,
            has_caption: false,
            project_name: std::sync::Arc::from(""),
            time: 0.0,
            frames_per_second: 0.0,
            events: Vec::new(),
            cursor: None,
            paste_text: None,
            badges: Vec::new(),
            marquee: None,
            scripted: false,
        }
    }
}

/// One input record per popped-out tab's window.
///
/// A popped-out window's events must not reach the main window's pass, because
/// their coordinates belong to that window's own client area. The frontend
/// keeps them apart, and the shell reads the tab's record when it runs that
/// tab's pass. A tab without a record (a window the frontend has not created
/// yet) gets a default one.
#[derive(Clone, Debug, Default)]
pub struct EguiPopoutInputs {
    /// Each popped-out tab's input, keyed by the tab's wire name.
    pub by_tab: std::collections::BTreeMap<String, EguiShellInput>,
}

impl Resource for EguiPopoutInputs {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

/// A rectangle in logical pixels that crosses the boundary: a viewport body,
/// an overlay region or the IME candidate's place.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShellRect {
    /// Distance from the client area's left edge.
    pub x: f32,
    /// Distance from the client area's top edge.
    pub y: f32,
    /// Extent along x.
    pub width: f32,
    /// Extent along y.
    pub height: f32,
}

impl ShellRect {
    /// Whether a logical point lies inside the rectangle, edges included.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

/// One shell pass's output, as plain data.
pub struct EguiShellFrame {
    /// Tessellated meshes to draw. Paint callbacks are removed when the frame
    /// is built.
    pub meshes: Vec<ClippedPrimitive>,
    /// Texture uploads the renderer must apply before drawing.
    pub textures: TexturesDelta,
    /// The pixels-per-point the meshes were tessellated with.
    pub pixels_per_point: f32,
    /// Whether egui wants the pointer: when true, a click should go to the UI
    /// rather than to a scene viewport.
    pub wants_pointer: bool,
    /// Whether egui wants the keyboard: when true, key presses should go to
    /// the UI.
    pub wants_keyboard: bool,
    /// The cursor icon egui asked for.
    pub cursor_icon: CursorIcon,
    /// Text egui asked to copy to the clipboard.
    pub clipboard_text: Option<String>,
    /// The scene viewports' body rectangles in logical points, one slot per
    /// view index. The frontend places that view's child window here, so each
    /// viewport follows the dock layout. A slot left `None` hides that
    /// viewport's window, because its tab is not shown. The list has one slot
    /// per view the dock knows, one past its highest view index.
    pub scene_rects: Vec<Option<ShellRect>>,
    /// The game viewport's body rectangle in logical points. The frontend
    /// places the primary surface's child window here, so the game view
    /// follows the dock layout. `None` hides that window, because the Game tab
    /// is not shown.
    pub game_rect: Option<ShellRect>,
    /// The UI rectangles the shell drew this pass, in logical points. A click
    /// inside one belongs to the UI, not the scene, so the frontend never asks
    /// the scene what is under it.
    pub overlay_regions: Vec<ShellRect>,
    /// Whether IME composition should be enabled.
    pub ime_allowed: bool,
    /// Where the IME candidate window should appear, in logical points.
    pub ime_rect: Option<ShellRect>,
    /// The asset path a tile drag hands to the OS. The frontend starts an OLE
    /// drag with it, so the person can drop the file into Explorer.
    pub drag_out: Option<String>,
    /// The asks this pass raised for the executable: a Problems row to open, an
    /// undo or redo, a dock tab to show. The executable drains them at its
    /// frame boundary and never reads the shell session for them.
    pub requests: ShellRequests,
    /// Whether the shell's menu asked the editor to quit. The winit frontend
    /// ends the run when it sees this flag; a browser frontend ignores it,
    /// because a page cannot close itself safely.
    pub quit_requested: bool,
    /// How soon egui asked for the next pass, when it asked at all. `None`
    /// means no pass was requested. The frontend only checks whether a request
    /// was made, and draws the next frame at once when one was.
    pub repaint_after: Option<Duration>,
    /// One frame per tab popped out into its own OS window. The shell runs a
    /// pass in that tab's own context, and the frontend paints the result into
    /// the tab's window. A tab that appears here gets a window; a tab that
    /// disappears from the list has its window closed.
    pub popout_frames: Vec<PopoutFrame>,
}

/// One popped-out tab's frame: which tab it draws, and the pass's output.
pub struct PopoutFrame {
    /// The tab's wire name, which is also its window's identity.
    pub tab: String,
    /// The window's title, as the shell wants it shown.
    pub title: String,
    /// The pass's output, in the same shape the main window's frame takes.
    pub frame: Box<EguiShellFrame>,
}

impl Resource for EguiShellFrame {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

impl EguiShellFrame {
    /// Clears every texture update the frame still carries.
    ///
    /// epaint panics when a `TexturesDelta` with unapplied updates is dropped.
    /// A pass can replace a frame the executable never drew - a reload runs a
    /// pass while the previous frame is still queued - so the old frame's
    /// updates are discarded first. Popped-out windows carry frames of their
    /// own and are cleared with it.
    pub fn discard_unapplied_textures(&mut self) {
        self.textures.clear();
        for popout in &mut self.popout_frames {
            popout.frame.discard_unapplied_textures();
        }
    }
}

impl Default for EguiShellFrame {
    fn default() -> Self {
        Self {
            meshes: Vec::new(),
            textures: TexturesDelta::default(),
            pixels_per_point: 1.0,
            wants_pointer: false,
            wants_keyboard: false,
            cursor_icon: CursorIcon::Default,
            clipboard_text: None,
            scene_rects: Vec::new(),
            game_rect: None,
            overlay_regions: Vec::new(),
            ime_allowed: false,
            ime_rect: None,
            drag_out: None,
            requests: ShellRequests::default(),
            quit_requested: false,
            repaint_after: None,
            popout_frames: Vec::new(),
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Builds the frame the frontend drains, from egui's output for one pass.
///
/// egui describes its output as a list of primitives. A callback primitive
/// carries arbitrary user data, which cannot cross the DLL boundary, so every
/// primitive except a mesh is dropped here.
pub fn frame_from_output(context: &Context, egui_output: FullOutput) -> EguiShellFrame {
    let pixels_per_point = egui_output.pixels_per_point;
    // The earliest deadline any viewport named. `Duration::MAX` is egui's way
    // of saying it wants no new pass.
    let repaint_after = egui_output
        .viewport_output
        .values()
        .map(|viewport| viewport.repaint_delay)
        .filter(|repaint_delay| *repaint_delay != Duration::MAX)
        .min();
    // Tessellation is the shell's heaviest CPU step and runs in this shared
    // dylib, so its zone belongs here rather than in either caller.
    let meshes = {
        let _zone = pill_core::profile_scope!("egui tessellate");
        context
            .tessellate(egui_output.shapes, pixels_per_point)
            .into_iter()
            .filter(|clipped_primitive| {
                matches!(
                    clipped_primitive.primitive,
                    egui::epaint::Primitive::Mesh(_)
                )
            })
            .collect()
    };
    let wants_pointer = context.egui_wants_pointer_input();
    let wants_keyboard = context.egui_wants_keyboard_input();
    let platform_output = egui_output.platform_output;
    let mut clipboard_text = None;
    for command in &platform_output.commands {
        if let OutputCommand::CopyText(text) = command {
            clipboard_text = Some(text.clone());
        }
    }
    EguiShellFrame {
        meshes,
        textures: egui_output.textures_delta,
        pixels_per_point,
        wants_pointer,
        wants_keyboard,
        cursor_icon: platform_output.cursor_icon,
        clipboard_text,
        // The shell fills in the scene rectangles after its dock pass runs.
        // This function only knows egui's own output, so the list starts empty
        // and the shell gives it one slot per view the dock needs.
        scene_rects: Vec::new(),
        // The shell records the game rectangle the same way, as it draws the
        // Game tab.
        game_rect: None,
        // The shell records the overlay regions the same way, as it draws.
        overlay_regions: Vec::new(),
        // The engine reports IME composition in its input events, so this
        // frame only decides whether IME is allowed. It is allowed exactly
        // while egui wants the keyboard, because the candidate window belongs
        // to the focused text field.
        ime_allowed: wants_keyboard,
        ime_rect: None,
        // Only the shell asks for a drag out, and it records the path itself.
        drag_out: None,
        // The pass fills in its requests before the frame is drained. egui's
        // own output carries none of them.
        requests: ShellRequests::default(),
        // Only the shell's menu can ask to quit, and it sets the flag itself.
        quit_requested: false,
        repaint_after,
        // A popped-out tab's pass is its own; this frame is the main window's.
        popout_frames: Vec::new(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::ImageDelta;
    use egui::{ColorImage, RawInput};

    #[test]
    fn a_frame_reports_meshes_and_drops_callbacks() {
        let context = Context::default();
        let egui_output = context.run_ui(RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label("hello");
            });
        });
        let mut frame = frame_from_output(&context, egui_output);
        assert!(!frame.meshes.is_empty());
        assert!(frame.meshes.iter().all(|clipped_primitive| {
            matches!(
                clipped_primitive.primitive,
                egui::epaint::Primitive::Mesh(_)
            )
        }));
        assert!(frame.pixels_per_point > 0.0);
        assert!(frame.overlay_regions.is_empty());
        // There is no GPU in this test, and epaint's debug check fails if a
        // texture delta is dropped unapplied. Clear it the documented way.
        frame.textures.clear();
    }

    #[test]
    fn a_shell_rect_contains_its_edges() {
        let shell_rect = ShellRect {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        assert!(shell_rect.contains(10.0, 20.0));
        assert!(shell_rect.contains(110.0, 70.0));
        assert!(shell_rect.contains(60.0, 45.0));
        assert!(!shell_rect.contains(9.9, 45.0));
        assert!(!shell_rect.contains(60.0, 70.1));
    }

    #[test]
    fn a_frame_exposes_clipboard_commands() {
        let context = Context::default();
        let egui_output = context.run_ui(RawInput::default(), |ui| {
            ui.ctx().copy_text("copied".to_owned());
        });
        let mut frame = frame_from_output(&context, egui_output);
        assert_eq!(frame.clipboard_text.as_deref(), Some("copied"));
        frame.textures.clear();
    }

    /// A frame the executable never draws discards its texture updates instead
    /// of tripping epaint's drop assertion, popped-out frames included.
    #[test]
    fn a_discarded_frame_clears_its_texture_updates() {
        // The pixel's colour is irrelevant; it only has to be a real colour,
        // so it comes from the theme rather than being written here.
        let image = || ColorImage {
            size: [1, 1],
            source_size: egui::vec2(1.0, 1.0),
            pixels: vec![crate::style::panel()],
        };
        let mut frame = EguiShellFrame::default();
        frame.textures.push(
            egui::TextureId::User(7),
            ImageDelta::full(image(), egui::TextureOptions::NEAREST),
        );
        let mut popout = EguiShellFrame::default();
        popout.textures.push(
            egui::TextureId::User(8),
            ImageDelta::full(image(), egui::TextureOptions::NEAREST),
        );
        frame.popout_frames.push(PopoutFrame {
            tab: "assets".to_string(),
            title: "Pill Editor - Assets".to_string(),
            frame: Box::new(popout),
        });

        frame.discard_unapplied_textures();

        assert!(frame.textures.set.is_empty());
        assert!(frame.popout_frames[0].frame.textures.set.is_empty());
        // Dropping the frame is what used to panic.
    }
}
