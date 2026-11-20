//! Applies the editor's engine controls: pause and step, the gizmo's tool,
//! space and snapping, the space cycle through the active tool's frames, the
//! scene grid's switch, and a system's on/off flag.
//!
//! # Responsibilities
//!
//! - Apply those controls, and answer a system toggle with the inverse that
//!   switches the system back.
//! - Remember the space a tool was last on, so choosing another tool and
//!   coming back finds it again.
//!
//! # Design
//!
//! One `match` over this family's commands, called from
//! [`EditorCommand::apply_one`](super::EditorCommand). A command that changed
//! something returns a [`CommandOutcome`]: the inverse the undo history
//! records, or `plain()` when there is nothing to undo. A command from another
//! family never arrives here; it hits `unreachable!`.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::gizmos::GizmoRegistry;
use pill_engine::Engine;

// Current crate
use super::CommandOutcome;
use super::*;
use crate::tools::BuiltinTool;

// =============================================================================
// Free Functions
// =============================================================================

/// Applies one engine command.
///
/// # Errors
///
/// Returns a message naming the command and the reason when:
///
/// - a system toggle names a registration index that has no system;
/// - the gizmo state cannot be read back after this function created it;
/// - the surface cameras cannot be read back after this function created
///   them.
///
/// # Panics
///
/// Panics when `command` belongs to another family; `EditorCommand::apply_one`
/// routes only engine commands here.
pub(super) fn apply(
    engine: &mut Engine,
    command: &EditorCommand,
) -> Result<CommandOutcome, String> {
    match command {
        EditorCommand::SetSystemEnabled { index, enabled } => {
            let was_enabled = engine
                .system_snapshots()
                .get(*index)
                .map(|system| system.enabled)
                .ok_or_else(|| format!("{command:?}: no system at registration index {index}"))?;
            if engine.set_system_enabled_at(*index, *enabled) {
                Ok(CommandOutcome::inverse(EditorCommand::SetSystemEnabled {
                    index: *index,
                    enabled: was_enabled,
                }))
            } else {
                Err(format!(
                    "{command:?}: no system at registration index {index}"
                ))
            }
        }

        EditorCommand::SetPaused { paused } => {
            engine.set_systems_paused(*paused);
            Ok(CommandOutcome::plain())
        }

        EditorCommand::SetGizmoTool { mode, space, snap } => {
            let world = engine.world_mut();
            if world.get_resource::<crate::gizmo::GizmoState>().is_none() {
                world.insert_resource(crate::gizmo::GizmoState::default());
            }
            // Step 1: write the settings this row names. A field the command
            // leaves as `None` keeps its current value, so a row that changes
            // one setting leaves the other two alone.
            let mut chosen = None;
            if let Some(gizmo_state) = world.get_resource_mut::<crate::gizmo::GizmoState>() {
                if let Some(mode) = mode {
                    gizmo_state.settings.mode = *mode;
                }
                if let Some(space) = space {
                    gizmo_state.settings.space = *space;
                    // A row that names World or Local asked for that space, so
                    // it leaves whichever tool frame the cycle had reached.
                    gizmo_state.tool_frame = None;
                }
                if let Some(snap) = snap {
                    gizmo_state.settings.snap = *snap;
                }
            } else {
                return Err(format!("{command:?}: the gizmo state could not be claimed"));
            }
            // Step 2: a built-in tool's row *is* a gizmo mode, so the
            // registry's active tool follows the mode the row set, and with it
            // the space that tool was last on. `None` (a space or snapping
            // row) says nothing about the tool and leaves it alone.
            if let Some(mode) = mode {
                let tool_name = BuiltinTool::from(*mode).name();
                let _ = crate::tools::activate_tool(world, Some(tool_name));
            }
            // Step 3: the choice belongs to the tool that was active when it
            // was made, so choosing another tool and coming back finds it
            // again.
            if let Some(gizmo_state) = world.get_resource::<crate::gizmo::GizmoState>() {
                chosen = Some((gizmo_state.settings.space, gizmo_state.tool_frame));
            }
            if let Some((chosen_space, chosen_frame)) = chosen {
                if space.is_some() || mode.is_some() {
                    remember_active_space(world, chosen_space, chosen_frame);
                }
            }
            Ok(CommandOutcome::plain())
        }

        EditorCommand::CycleGizmoSpace => {
            let world = engine.world_mut();
            if world.get_resource::<crate::gizmo::GizmoState>().is_none() {
                world.insert_resource(crate::gizmo::GizmoState::default());
            }
            // Step 1: Ask the active tool which frames it offers for what the
            // gizmo draws on.
            let frames = crate::tools::selection_frames(world);
            // Step 2: Walk the cycle one step and write both halves of the
            // space the gizmo is in.
            let Some(gizmo_state) = world.get_resource_mut::<crate::gizmo::GizmoState>() else {
                return Err(format!("{command:?}: the gizmo state could not be claimed"));
            };
            let (space, tool_frame) = crate::gizmo::next_space(
                gizmo_state.settings.space,
                gizmo_state.tool_frame,
                frames.len(),
            );
            let changed =
                gizmo_state.settings.space != space || gizmo_state.tool_frame != tool_frame;
            gizmo_state.settings.space = space;
            gizmo_state.tool_frame = tool_frame;
            let frame_name = tool_frame
                .and_then(|index| frames.get(index as usize))
                .map(|frame| frame.name.clone());
            // Step 3: The row is only ever in one place, so a step that changes
            // nothing logs nothing - pressing it again after a reload that
            // rebuilt the frames stays quiet.
            if changed {
                let label = frame_name.as_deref().unwrap_or(match space {
                    crate::gizmo::GizmoSpace::World => "world",
                    crate::gizmo::GizmoSpace::Local => "local",
                });
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    space = label,
                    "[editor] gizmo space"
                );
            }
            remember_active_space(world, space, tool_frame);
            Ok(CommandOutcome::plain())
        }

        EditorCommand::SetGrid { enabled } => {
            let world = engine.world_mut();
            if world
                .get_resource::<pill_renderer_api::SurfaceCameras>()
                .is_none()
            {
                world.insert_resource(pill_renderer_api::SurfaceCameras::default());
            }
            let Some(surface_cameras) =
                world.get_resource_mut::<pill_renderer_api::SurfaceCameras>()
            else {
                return Err(format!(
                    "{command:?}: the surface cameras could not be claimed"
                ));
            };
            // Log only a real change, so sending the current state again adds
            // no line.
            if surface_cameras.grid.enabled != *enabled {
                surface_cameras.grid.enabled = *enabled;
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    enabled = *enabled,
                    "[editor] scene grid"
                );
            }
            Ok(CommandOutcome::plain())
        }

        EditorCommand::SetGizmos { enabled } => {
            let world = engine.world_mut();
            let Some(gizmo_registry) = world.get_resource_mut::<GizmoRegistry>() else {
                // No component declared a gizmo, so there is nothing to switch
                // and nothing to report.
                return Ok(CommandOutcome::plain());
            };
            // Log only a real change, so sending the current state again adds
            // no line.
            if gizmo_registry.is_enabled() != *enabled {
                gizmo_registry.set_enabled(*enabled);
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    enabled = *enabled,
                    "[editor] gizmos"
                );
            }
            Ok(CommandOutcome::plain())
        }

        EditorCommand::SetTool { tool } => {
            // One call changes both halves: the registry's active name and the
            // space the gizmo is on, which the arriving tool was last on.
            crate::tools::activate_tool(engine.world_mut(), tool.as_deref())
                .map_err(|reason| format!("{command:?}: {reason}"))?;
            pill_core::info!(
                target: telemetry_target::EDITOR,
                tool = tool.as_deref().unwrap_or("none"),
                "[editor] tool activated"
            );
            Ok(CommandOutcome::plain())
        }

        EditorCommand::SetGizmoTypeVisible { type_name, visible } => {
            let world = engine.world_mut();
            let Some(gizmo_registry) = world.get_resource_mut::<GizmoRegistry>() else {
                return Ok(CommandOutcome::plain());
            };
            if gizmo_registry.is_type_visible(type_name) != *visible {
                gizmo_registry.set_type_visible(type_name, *visible);
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    gizmo_type = type_name.as_str(),
                    visible = *visible,
                    "[editor] gizmo type"
                );
            }
            Ok(CommandOutcome::plain())
        }

        EditorCommand::RunElementMenu { item } => {
            // The frame boundary asks the tool what the row means and applies
            // those commands in this batch's place (`drain::apply_pending`),
            // because only the tool knows. Reaching this arm means the command
            // was applied straight through, so the row would run nothing.
            Err(format!(
                "{command:?}: '{item}' runs its tool's commands at the frame boundary, not here"
            ))
        }

        EditorCommand::StepOnce => {
            engine.request_single_step();
            Ok(CommandOutcome::plain())
        }

        EditorCommand::EnterPlay => {
            if engine.run_mode() == pill_engine::RunMode::Playing {
                return Ok(CommandOutcome::plain());
            }
            // Step 1: Take the document's own state aside before the world is
            // copied, so the copy a game runs against cannot hold it.
            crate::play_session::EditorPlaySession::save(engine.world_mut());
            // Step 2: Copy the world. A refusal changes nothing, so the state
            // taken above is put straight back.
            match engine.enter_play() {
                Ok(()) => {
                    pill_core::info!(
                        target: telemetry_target::EDITOR,
                        mode = "playing",
                        "[editor] play toggled"
                    );
                    Ok(CommandOutcome::plain())
                }
                Err(refusal) => {
                    crate::play_session::EditorPlaySession::restore(engine.world_mut());
                    let reason = refusal.to_string();
                    pill_core::warn!(
                        target: telemetry_target::EDITOR,
                        mode = "editing",
                        reason = %reason,
                        "[editor] play toggled"
                    );
                    push_play_refusal_notice(engine.world_mut(), &reason);
                    Err(reason)
                }
            }
        }

        EditorCommand::ExitPlay => {
            if !engine.exit_play() {
                return Ok(CommandOutcome::plain());
            }
            // The engine put the world back; the editor puts its own document
            // state back in the same command, so the next frame sees one state.
            crate::play_session::EditorPlaySession::restore(engine.world_mut());
            pill_core::info!(
                target: telemetry_target::EDITOR,
                mode = "editing",
                "[editor] play toggled"
            );
            Ok(CommandOutcome::plain())
        }

        other => unreachable!("{:?} is not a engine command", other.family()),
    }
}

/// Adds one error notice for a refused Play, so the person sees the reason as
/// a toast instead of only in the Console.
fn push_play_refusal_notice(world: &mut pill_engine::World, reason: &str) {
    use crate::notices::{EditorNotices, NoticeLevel};
    if world.get_resource::<EditorNotices>().is_none() {
        world.insert_resource(EditorNotices::default());
    }
    let Some(notices) = world.get_resource_mut::<EditorNotices>() else {
        return;
    };
    notices.push_event(NoticeLevel::Error, format!("Play refused: {reason}"));
}

/// Records the gizmo's space as the active tool's own choice.
///
/// The space row, the menu rows and the chord all land here, so whichever
/// tool was active when a space was chosen is the one that remembers it.
fn remember_active_space(
    world: &mut pill_engine::World,
    space: crate::gizmo::GizmoSpace,
    frame: Option<u8>,
) {
    let tool = crate::tools::tool_registry_mut(world)
        .active()
        .map(str::to_string);
    let Some(tool) = tool else {
        return;
    };
    if world
        .get_resource::<crate::tools::ToolSpaceChoices>()
        .is_none()
    {
        world.insert_resource(crate::tools::ToolSpaceChoices::default());
    }
    if let Some(choices) = world.get_resource_mut::<crate::tools::ToolSpaceChoices>() {
        choices.remember(&tool, space, frame);
    }
}
