//! Applies the editor's engine controls: pause and step, the gizmo's tool,
//! space and snapping, the scene grid's switch, and a system's on/off flag.
//!
//! # Responsibilities
//!
//! - Apply those controls, and answer a system toggle with the inverse that
//!   switches the system back.
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
            let Some(gizmo_state) = world.get_resource_mut::<crate::gizmo::GizmoState>() else {
                return Err(format!("{command:?}: the gizmo state could not be claimed"));
            };
            // A field the command leaves as `None` keeps its current value, so
            // a row that changes one setting leaves the other two alone.
            if let Some(mode) = mode {
                gizmo_state.settings.mode = *mode;
            }
            if let Some(space) = space {
                gizmo_state.settings.space = *space;
            }
            if let Some(snap) = snap {
                gizmo_state.settings.snap = *snap;
            }
            // A built-in tool's row *is* a gizmo mode, so the registry's active
            // tool follows the mode the row set. `None` (a space or snapping
            // row) says nothing about the tool and leaves it alone.
            if let Some(mode) = mode {
                let tool_name = BuiltinTool::from(*mode).name();
                let _ = crate::tools::tool_registry_mut(world).set_active(Some(tool_name));
            }
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
