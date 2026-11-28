//! Geometry of the scene and game viewports, their overlay chips, and the
//! entity badges drawn over the scene.
//!
//! # Responsibilities
//!
//! - The measured scene body rectangle the frontend places the scene's child
//!   window at, and the game view's own rectangle.
//! - The per-viewport overlay chip (the frame counter) and the region it marks.
//! - The entity badges: the mark each entity gets, the shadow under it, and
//!   the pick region it marks.
//! - The marquee the executable holds open while a drag selects elements.
//!
//! The globs below are absolute paths: a generated patch crate replays this
//! file's top-level `use` lines, so a `super::`-relative glob would not
//! resolve there.

// Current crate
#[allow(unused_imports)]
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// A shell update slower than this logs its own `[editor] egui frame` line,
/// besides the 210-frame heartbeat. The limit is one whole 60 Hz frame
/// (16 milliseconds): a debug build's pass takes several milliseconds, so a
/// lower limit would log a line every frame. A pass that takes a whole frame
/// is worth its own record.
pub const SLOW_UPDATE_MICROS: u64 = 16_000;

/// The width of the scene's frame counter box, in logical points.
pub const SCENE_COUNTER_WIDTH: f32 = 76.0;

/// The height of the scene's frame counter box, in logical points.
pub const SCENE_COUNTER_HEIGHT: f32 = 20.0;

/// The gap, in logical points, between the frame counter box and the tab
/// body's top-right corner.
pub const SCENE_COUNTER_MARGIN: f32 = 8.0;

/// The space, in logical points, that a badge's mark leaves inside its
/// clickable rectangle. The icon size preference sets the mark's size, and
/// this padding makes up the rest of the rectangle.
pub const SCENE_BADGE_PADDING: f32 = 3.0;

/// How far a badge's shadow spreads, as a fraction of the mark's size.
pub const SCENE_BADGE_SHADOW_BLUR: f32 = 0.1;

/// How far down and right of the mark the whole shadow sits, as a fraction of
/// the mark's size. The direction matches a CSS shadow; the offset is small,
/// so the shadow stays under the mark instead of reading as a second, offset
/// copy.
pub const SCENE_BADGE_SHADOW_DISTANCE: f32 = 0.05;

/// The opacity of one shadow sample. The shadow draws the mark once per
/// sample of [`SCENE_BADGE_SHADOW_SAMPLES`], so the copies add up in the
/// middle and thin out at the rim.
pub const SCENE_BADGE_SHADOW_ALPHA: f32 = 0.22;

/// The unit offsets a badge's shadow is sampled at: the centre and a ring
/// around it. Together they read as one soft edge rather than a second hard
/// copy of the mark.
pub const SCENE_BADGE_SHADOW_SAMPLES: [(f32, f32); 9] = [
    (0.0, 0.0),
    (1.0, 0.0),
    (0.5, 0.5),
    (0.0, 1.0),
    (-0.5, 0.5),
    (-1.0, 0.0),
    (-0.5, -0.5),
    (0.0, -1.0),
    (0.5, -0.5),
];

/// How much of its full strength the marquee's fill draws at. The border
/// carries the rectangle; the fill only marks the area, so the points under it
/// stay readable.
pub const SCENE_MARQUEE_FILL_OPACITY: f32 = 0.14;

/// The width of the marquee's border, in logical points.
pub const SCENE_MARQUEE_STROKE_WIDTH: f32 = 1.0;

// =============================================================================
// Free Functions
// =============================================================================

/// Converts an egui rectangle into the boundary's plain rectangle.
pub fn shell_rect(rect: egui::Rect) -> ShellRect {
    ShellRect {
        x: rect.min.x,
        y: rect.min.y,
        width: rect.width(),
        height: rect.height(),
    }
}

/// Draws the marquee a drag in `view` is holding open, when one is.
///
/// The rectangle is in the main window's logical points, the space the shell
/// lays its overlay out in. It draws in the colour the scene already uses for
/// a selected entity's badge, so a rectangle and the marks inside it read as
/// one selection. It senses nothing: it is an indicator over a drag the
/// executable owns, and that drag's release reaches the viewport's own press
/// path.
pub fn scene_marquee(ui: &egui::Ui, marquee: Option<&ShellMarquee>, view: usize) {
    let Some(marquee) = marquee.filter(|marquee| marquee.view == view) else {
        return;
    };
    let rect = egui::Rect::from_min_size(
        egui::pos2(marquee.rect.x, marquee.rect.y),
        egui::vec2(marquee.rect.width, marquee.rect.height),
    );
    ui.painter().rect(
        rect,
        egui::CornerRadius::ZERO,
        icons::with_alpha(style::danger(), SCENE_MARQUEE_FILL_OPACITY),
        egui::Stroke::new(SCENE_MARQUEE_STROKE_WIDTH, style::danger()),
        egui::StrokeKind::Inside,
    );
}

/// Returns a scene tab's body rectangle in logical points.
///
/// This is the tab body itself, not a grown one: how far a view's window may
/// extend past it depends on what draws above that window, which the frontend
/// knows and a shell module cannot. A composition overlay paints the UI over
/// the child window and grows the window to hide the seam; the window surface
/// has the child above the UI, where any growth covers the tab bar and the
/// panels beside it.
pub fn scene_body_rect(ui: &egui::Ui) -> ShellRect {
    shell_rect(ui.max_rect())
}

/// Draws a scene tab's overlay chip: the frame counter in the viewport's
/// top-right corner, painted from the theme.
///
/// The editor's composition overlay paints above the scene's child window, so
/// the counter shows over the viewport. Its rectangle joins the frame's
/// overlay regions, so a click on it belongs to the UI and is never read as a
/// scene pick.
pub fn scene_overlay(
    ui: &mut egui::Ui,
    frames_per_second: f32,
    overlay_regions: &mut Vec<ShellRect>,
) {
    let counter_rect = frame_counter_rect(ui);
    overlay_regions.push(shell_rect(counter_rect));
    // The counter is painted, not laid out, so it takes no space in the tab
    // body. Its rectangle still senses clicks.
    let _ = ui.interact(
        counter_rect,
        ui.id().with("scene_frame_counter"),
        egui::Sense::click(),
    );
    ui.painter().rect(
        counter_rect,
        egui::CornerRadius::same(5),
        style::overlay(),
        egui::Stroke::new(1.0, style::overlay_border()),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        counter_rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("{frames_per_second:.0} fps"),
        egui::TextStyle::Monospace.resolve(ui.style()),
        style::text_strong(),
    );
}

/// Returns the frame counter's rectangle: a box whose top-right corner sits
/// one margin inside the tab body's top-right corner.
pub fn frame_counter_rect(ui: &egui::Ui) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(
            ui.max_rect().right() - SCENE_COUNTER_WIDTH - SCENE_COUNTER_MARGIN,
            ui.max_rect().top() + SCENE_COUNTER_MARGIN,
        ),
        egui::vec2(SCENE_COUNTER_WIDTH, SCENE_COUNTER_HEIGHT),
    )
}

/// Returns the centres a badge's shadow is drawn at: the mark's centre moved
/// to the shadow's place, then moved to each blur sample around that point.
pub fn badge_shadow_centres(mark: egui::Rect) -> [egui::Pos2; SCENE_BADGE_SHADOW_SAMPLES.len()] {
    let distance = mark.width() * SCENE_BADGE_SHADOW_DISTANCE;
    let blur = mark.width() * SCENE_BADGE_SHADOW_BLUR;
    let origin = mark.center() + egui::vec2(distance, distance);
    let mut centres = [origin; SCENE_BADGE_SHADOW_SAMPLES.len()];
    for (centre, (x, y)) in centres.iter_mut().zip(SCENE_BADGE_SHADOW_SAMPLES) {
        *centre = origin + egui::vec2(x * blur, y * blur);
    }
    centres
}

/// Draws one scene view's entity badges: for each point the executable
/// projected, the entity's component mark (a camera, a light), or a plain dot
/// when it has neither.
///
/// A badge is only the mark, with no chip behind it, on a short blurred drop
/// shadow in the theme's shadow colour. The shadow keeps the glyph readable
/// over a busy scene, as a CSS text shadow does. The mark's size is the
/// `editor.gizmo_icon_size` preference, and the rectangle that picks it grows
/// with it.
///
/// The mark's colour carries the state: a selected entity's uses the theme's
/// danger colour, and hovering brightens an unselected one. A badge belongs
/// to the UI: its rectangle joins the frame's overlay regions, so a click on
/// it selects its entity instead of pressing the scene behind it.
pub fn scene_badges(viewer: &mut EditorTabViewer<'_>, ui: &mut egui::Ui, view: usize) {
    // Step 1: collect this view's badges. A view with none draws nothing.
    let badges: Vec<ShellBadge> = viewer
        .badges
        .iter()
        .filter(|badge| badge.view == view)
        .copied()
        .collect();
    if badges.is_empty() {
        return;
    }
    // Step 2: draw one badge per entity.
    //
    // The snapshot has already parsed the icon size preference and clamped it
    // to its range, so this only sizes the mark and the rectangle around it.
    let icon_size = viewer.snapshot.preferences.gizmo_icon_size;
    let badge_size = icon_size + SCENE_BADGE_PADDING * 2.0;
    let mut picked_entity = None;
    for badge in badges {
        let rect = egui::Rect::from_center_size(
            egui::pos2(badge.x, badge.y),
            egui::Vec2::splat(badge_size),
        );
        let response = ui.interact(
            rect,
            egui::Id::new(("pill_editor_shell.scene_badge", badge.entity)),
            egui::Sense::click(),
        );
        let hovered = response.hovered();
        let selected = viewer.selection.entities().contains(&badge.entity);
        let badge_key = format!("{view}:{}", badge.entity);
        viewer.inspector.badge_seen.insert(badge_key.clone());
        if remember_moved_rect(&mut viewer.inspector.badge_rects, &badge_key, rect) {
            pill_core::debug!(
                target: telemetry_target::EDITOR,
                view = badge.view,
                entity = %badge.entity,
                min_x = rect.min.x,
                min_y = rect.min.y,
                max_x = rect.max.x,
                max_y = rect.max.y,
                "[editor] scene badge rect"
            );
        }
        let archetype_label = viewer
            .snapshot
            .entities
            .iter()
            .find(|entry| entry.entity == badge.entity)
            .map(|entry| entry.archetype_label.as_str())
            .unwrap_or_default();
        // The shadow always uses the theme's shadow colour. Hover and
        // selection only change the mark's own colour.
        let shadow_ink = icons::with_alpha(style::shadow(), SCENE_BADGE_SHADOW_ALPHA);
        let mark = rect.shrink(SCENE_BADGE_PADDING);
        let ink = match (selected, hovered) {
            (true, _) => style::danger(),
            (false, true) => style::text_strong(),
            (false, false) => style::text(),
        };
        match inspector::badge_icon(archetype_label) {
            Some(icon) => {
                // Each shadow copy is the mark drawn at one sample of
                // [`badge_shadow_centres`], so the copies add up to a soft
                // edge.
                for centre in badge_shadow_centres(mark) {
                    icons::paint_duotone(
                        ui.painter(),
                        egui::Rect::from_center_size(centre, mark.size()),
                        icon,
                        shadow_ink,
                    );
                }
                icons::paint_duotone(ui.painter(), mark, icon, ink);
            }
            None => {
                let dot_ink = match (selected, hovered) {
                    (true, _) => style::danger(),
                    (false, true) => style::text_strong(),
                    (false, false) => style::text_muted(),
                };
                for centre in badge_shadow_centres(mark) {
                    ui.painter()
                        .circle_filled(centre, icon_size * 0.3, shadow_ink);
                }
                ui.painter()
                    .circle_filled(mark.center(), icon_size * 0.3, dot_ink);
            }
        }
        if response.clicked() {
            picked_entity = Some(badge.entity);
        }
        viewer.overlay_regions.push(shell_rect(rect));
    }
    // Step 3: select the entity whose badge was clicked.
    if let Some(entity) = picked_entity {
        select_entity(viewer.selection, entity);
        pill_core::info!(
            target: telemetry_target::EDITOR,
            entity = %entity,
            "[editor] scene badge"
        );
    }
}
