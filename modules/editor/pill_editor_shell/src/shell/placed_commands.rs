//! The button every place draws a placed command with.
//!
//! # Responsibilities
//!
//! - Draw one command a module placed in a surface whose look no data
//!   describes: an icon button from the command's title, icon and the state
//!   published for its target, or the module's own draw function when it
//!   registered one for that command and place.
//! - Report the invocation a click produces, into the pass's request list, so
//!   the executable hands it to the module that owns the id.
//!
//! # Design
//!
//! The data path is the default: the button is drawn here, in the editor's own
//! look, with the hover fill, the icon, the published dots and the hover text.
//! A module draw function is the exception, for a command whose look no data
//! describes (the Blender logo); its click reports through the same invocation
//! path, so a place never learns which of the two drew it.
//!
//! The button reports the rectangle it drew at, so the caller can log where a
//! suite should click - the module draw path reports none, because the module
//! owns that geometry.

// Current crate
use pill_editor_shell::shell::prelude::*;
// Deliberately unused in this build. A patch crate replays this file's
// opening `use` lines, so a patched body that calls a sibling in the shell
// module finds it through this glob.
#[allow(unused_imports)]
use pill_editor_shell::shell::*;

// =============================================================================
// Types
// =============================================================================

/// What a place needs to draw the commands modules put there: the registered
/// commands, the state published for each target, the draw functions, and the
/// queue a click joins.
pub struct PlacementHost<'a> {
    /// Every registered command, as the frame's snapshot carries it.
    pub commands: &'a [CommandView],
    /// The states the modules published for their commands.
    pub states: &'a CommandStates,
    /// The draw functions the modules registered for a free-form place.
    pub draws: &'a CommandDraws,
    /// The commands this pass asked modules to run.
    pub invocations: &'a mut Vec<CommandInvocation>,
}

impl PlacementHost<'_> {
    /// The commands that place something in `slot`, in registration order.
    pub fn for_slot<'a>(
        &'a self,
        matches: impl Fn(&UiSlot) -> bool + 'a,
    ) -> impl Iterator<Item = &'a CommandView> + 'a {
        self.commands
            .iter()
            .filter(move |command| command.placements.iter().any(&matches))
    }

    /// The commands that place something in the slots `matches` accepts and
    /// are visible on `target`, in registration order, as owned copies a menu
    /// can hold while it draws them.
    pub fn rows_for_slot(
        &self,
        matches: &dyn Fn(&UiSlot) -> bool,
        target: &CommandTarget,
    ) -> Vec<CommandView> {
        rows_for_slot(self.commands, self.states, matches, target)
    }

    /// Draws one placed command with [`draw_placed_command`], through this
    /// host's data and queues.
    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        command: &CommandView,
        slot: &UiSlot,
        target: &CommandTarget,
    ) -> Option<egui::Rect> {
        draw_placed_command(
            ui,
            command,
            slot,
            target,
            self.states,
            self.draws,
            self.invocations,
        )
    }

    /// Draws one placed command as a menu row with [`draw_placed_menu_row`].
    pub fn draw_row(
        &mut self,
        ui: &mut egui::Ui,
        command: &CommandView,
        slot: &UiSlot,
        target: &CommandTarget,
    ) -> bool {
        draw_placed_menu_row(
            ui,
            command,
            slot,
            target,
            self.states,
            self.draws,
            self.invocations,
        )
    }
}

// =============================================================================
// Constants
// =============================================================================

/// The side of one placed command's square button, in points.
pub const PLACED_COMMAND_SIZE: f32 = 16.0;

/// The radius of a published state's dot, in points.
const DOT_RADIUS: f32 = 2.6;

// =============================================================================
// Free Functions
// =============================================================================

/// The commands that place something in the slots `matches` accepts and are
/// visible on `target`, in registration order.
///
/// A place collects its rows first and draws them second, so a hidden command
/// takes no row at all - no separator and no gap left behind it.
pub fn rows_for_slot(
    commands: &[CommandView],
    states: &CommandStates,
    matches: &dyn Fn(&UiSlot) -> bool,
    target: &CommandTarget,
) -> Vec<CommandView> {
    commands
        .iter()
        .filter(|command| command.placements.iter().any(matches))
        .filter(|command| states.is_visible(&command.id, target))
        .cloned()
        .collect()
}

/// The rows a component's Inspector header menu offers: the commands placed in
/// that component's `ComponentContextMenu`, in registration order.
pub fn component_menu_rows(
    commands: &[CommandView],
    states: &CommandStates,
    component: &str,
    target: &CommandTarget,
) -> Vec<CommandView> {
    rows_for_slot(
        commands,
        states,
        &|slot: &UiSlot| {
            matches!(slot, UiSlot::ComponentContextMenu { .. }) && slot.matches_component(component)
        },
        target,
    )
}

/// The rows an entity row's menu offers: the `EntityContextMenu` placements.
pub fn entity_menu_rows(
    commands: &[CommandView],
    states: &CommandStates,
    target: &CommandTarget,
) -> Vec<CommandView> {
    rows_for_slot(
        commands,
        states,
        &|slot: &UiSlot| matches!(slot, UiSlot::EntityContextMenu),
        target,
    )
}

/// The rows an asset file's menu offers: the `AssetContextMenu` placements
/// registered for that file's extension.
pub fn asset_menu_rows(
    commands: &[CommandView],
    states: &CommandStates,
    extension: &str,
    target: &CommandTarget,
) -> Vec<CommandView> {
    rows_for_slot(
        commands,
        states,
        &|slot: &UiSlot| slot.matches_extension(extension),
        target,
    )
}

/// The rows the floating toolbox offers: the `ViewportToolbox` placements.
pub fn toolbox_rows(
    commands: &[CommandView],
    states: &CommandStates,
    target: &CommandTarget,
) -> Vec<CommandView> {
    rows_for_slot(
        commands,
        states,
        &|slot: &UiSlot| matches!(slot, UiSlot::ViewportToolbox),
        target,
    )
}

/// Draws one placed command and reports a click on it.
///
/// Returns the rectangle the editor drew, or `None` when the command is hidden
/// or a module's own draw function drew it.
pub fn draw_placed_command(
    ui: &mut egui::Ui,
    command: &CommandView,
    slot: &UiSlot,
    target: &CommandTarget,
    states: &CommandStates,
    draws: &CommandDraws,
    invocations: &mut Vec<CommandInvocation>,
) -> Option<egui::Rect> {
    // Step 1: a hidden command draws nothing at all, so its place closes up.
    if !states.is_visible(&command.id, target) {
        return None;
    }
    let state = states.state(&command.id, target);
    let enabled = states.is_enabled(&command.id, target);
    // Step 2: a module's own draw is the exception, and reports its own click.
    if let Some(draw) = draws.draw_for(&command.id, slot) {
        let mut context = CommandDrawContext {
            id: &command.id,
            target,
            state,
            invocations,
        };
        ui.push_id((command.id.as_str(), slot.name()), |ui| {
            draw(ui, &mut context);
        });
        return None;
    }
    // Step 3: from data. The icon is the usual look; a command with no icon
    // draws its label instead.
    let label = state
        .and_then(|state| state.label.clone())
        .unwrap_or_else(|| command.title.clone());
    let hover_text = state
        .and_then(|state| state.tooltip.clone())
        .map(|lines| lines.join("\n"))
        .unwrap_or_else(|| command.title.clone());
    let rect = match command.icon {
        Some(icon) => {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(PLACED_COMMAND_SIZE, PLACED_COMMAND_SIZE),
                egui::Sense::click(),
            );
            if response.hovered() && enabled {
                ui.painter()
                    .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let checked = state.and_then(|state| state.checked) == Some(true);
            let color = if !enabled {
                style::text_muted()
            } else if checked {
                style::accent()
            } else {
                style::text_strong()
            };
            icons::paint_duotone(ui.painter(), rect.shrink(1.0), icon, color);
            if let Some(state) = state {
                paint_state_dots(ui.painter(), rect, state);
            }
            let response = hover_with(response, enabled, &hover_text);
            if response.clicked() && enabled {
                invocations.push(CommandInvocation {
                    id: command.id.clone(),
                    target: target.clone(),
                });
            }
            rect
        }
        None => {
            let response = ui.add_enabled(enabled, egui::Button::new(label.as_str()));
            let response = kit::tooltip::on_hover_text(response, hover_text.as_str());
            if response.clicked() && enabled {
                invocations.push(CommandInvocation {
                    id: command.id.clone(),
                    target: target.clone(),
                });
            }
            response.rect
        }
    };
    Some(rect)
}

/// Draws one placed command as a menu row and reports a click on it.
///
/// A row from data uses the icon button the menus already draw with; a command
/// whose module registered a draw function for this place draws its own row
/// instead. Returns whether a row was drawn, so a caller can add a separator
/// only when something follows it.
pub fn draw_placed_menu_row(
    ui: &mut egui::Ui,
    command: &CommandView,
    slot: &UiSlot,
    target: &CommandTarget,
    states: &CommandStates,
    draws: &CommandDraws,
    invocations: &mut Vec<CommandInvocation>,
) -> bool {
    if !states.is_visible(&command.id, target) {
        return false;
    }
    let state = states.state(&command.id, target);
    let enabled = states.is_enabled(&command.id, target);
    if let Some(draw) = draws.draw_for(&command.id, slot) {
        let mut context = CommandDrawContext {
            id: &command.id,
            target,
            state,
            invocations,
        };
        ui.push_id((command.id.as_str(), slot.name()), |ui| {
            draw(ui, &mut context);
        });
        return true;
    }
    let label = state
        .and_then(|state| state.label.clone())
        .unwrap_or_else(|| command.title.clone());
    let hover_text = state
        .and_then(|state| state.tooltip.clone())
        .map(|lines| lines.join("\n"))
        .unwrap_or_else(|| command.title.clone());
    let icon = command.icon.unwrap_or(icons::Icon::Plus);
    let checked = state.and_then(|state| state.checked).unwrap_or(false);
    let response = ui.add_enabled(
        enabled,
        icons::IconButton::new(icon, label.as_str()).active(checked),
    );
    let response = kit::tooltip::on_hover_text(response, hover_text.as_str());
    if response.clicked() && enabled {
        invocations.push(CommandInvocation {
            id: command.id.clone(),
            target: target.clone(),
        });
        ui.close();
    }
    true
}

/// Paints a state's dots: the status in the lower-right corner and unsaved
/// work in the upper-right one.
pub fn paint_state_dots(painter: &egui::Painter, rect: egui::Rect, state: &CommandState) {
    let ring_color = style::panel();
    let paint_dot = |center: egui::Pos2, rgba: [u8; 4]| {
        painter.circle_filled(center, DOT_RADIUS + 1.0, ring_color);
        painter.circle_filled(center, DOT_RADIUS, style::published(rgba));
    };
    if let Some(indicator) = state.indicator {
        paint_dot(
            rect.right_bottom() - egui::vec2(DOT_RADIUS, DOT_RADIUS),
            indicator,
        );
    }
    if let Some(attention) = state.attention {
        paint_dot(
            egui::pos2(rect.max.x - DOT_RADIUS, rect.min.y + DOT_RADIUS),
            attention,
        );
    }
}

/// Adds the hover text and, when the command can act, the pointing cursor.
pub fn hover_with(response: egui::Response, enabled: bool, text: &str) -> egui::Response {
    let response = if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    kit::tooltip::on_hover_text(response, text)
}

/// Logs one placed command's button when it moved, so a suite can click it.
///
/// `key` names the place and the command (`MeshRendererComponent@gadget.snap`);
/// `rects` is the session's memory, so a button that stays put logs once.
pub fn remember_command_rect(
    rects: &mut LoggedRects<String>,
    log_rects: bool,
    key: &str,
    command: &str,
    slot: &UiSlot,
    rect: egui::Rect,
) {
    if !rects.remember_exact(key.to_string(), rect) || !log_rects {
        return;
    }
    pill_core::debug!(
        target: telemetry_target::EDITOR,
        command,
        slot = slot.name(),
        x = rect.min.x,
        y = rect.min.y,
        width = rect.width(),
        height = rect.height(),
        "[editor] command button rect"
    );
}
