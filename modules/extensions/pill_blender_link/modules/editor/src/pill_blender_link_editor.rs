//! The Blender live link: edit a mesh asset in Blender and see every change
//! in the editor at once, without a file in between.
//!
//! # Responsibilities
//!
//! - Register the link's two commands on `MeshRendererComponent`, each with
//!   its own draw function: the Blender button and the save button.
//! - Declare [`BlenderLinkServer`] and the per-frame [`blender_link_update`]
//!   system, which runs the steps in [`link_update`] in order.
//!
//! # Design
//!
//! Streamed geometry goes straight into the mesh asset through
//! `AssetManager::get_mut`, which moves the asset's content version, so the
//! renderer rebuilds the mesh's buffers on its next frame. Nothing reaches the
//! mesh's source file until the person saves: through the save button,
//! Blender's sidebar, or the quit prompt. Until then the mesh counts as
//! unsaved work - a dot on both buttons and on its Assets tile, and an entry
//! in the quit prompt. Each run of streamed updates becomes one step of the
//! editor's undo history, whose replay comes back through the
//! `ExtensionEditInbox` and is sent on to Blender.
//!
//! An editor-only extension: the folder holds only this crate, so the egui
//! editor loads it and no game build ever does. The Blender side is the
//! script in `blender_addon/`, which the editor starts Blender with, or which
//! can be installed as a Blender add-on.

// The crate's own name, so a patch crate's replayed `use` lines resolve.
extern crate self as pill_blender_link_editor;

// Standard library
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_editor_core::commands::{
    CommandInbox, CommandRegistration, CommandRegistrations, CommandSpec, CommandTarget,
};
use pill_editor_core::extension_edits::ExtensionEditInbox;
use pill_editor_core::history::CommandHistory;
use pill_editor_core::icons::Icon;
use pill_editor_core::panels::PanelCommands;
use pill_editor_core::placements::{CommandDrawRequest, CommandStates, UiSlot};
use pill_editor_core::session::EguiShellState;
use pill_editor_core::unsaved_work::UnsavedWork;
use pill_engine::component::ComponentRegistry;
use pill_engine::{pill_module, system, AssetManager, Commands, Engine, Entity, Query, ResMut};
use pill_master_renderer_data::assets::Mesh;
use pill_master_renderer_data::components::MeshRendererComponent;

// Current crate
pub use pill_blender_link_editor::link_server::{BlenderLinkServer, OpenOutcome};

/// Finding Blender and starting it with the link script.
pub mod blender_launch;
/// The link's buttons on the mesh renderer's Inspector header.
pub mod inspector_button;
/// The editor's end of the link: the socket server and the link table.
pub mod link_server;
/// The steps the link's system runs each frame.
pub mod link_update;
/// The Blender node graph a material's surface translates into.
pub mod material_graph;
/// What each link's material is, what it was last sent, and what changed.
pub mod material_link;
/// Reading a material's cooked fragment WGSL into that graph.
pub mod material_translate;
/// What the link knows about one mesh open in Blender.
pub mod mesh_link;
/// Writing a linked mesh back over its source file.
pub mod mesh_source;
/// The wire format both sides exchange.
pub mod protocol;

// The build script writes one address entry per function of this crate, which
// is what lets the host patch a function body instead of reloading the module.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Constants
// =============================================================================

/// The Blender button's action id; its clicks are taken under this id.
pub const BLENDER_ACTION_ID: &str = "pill_blender_link.open_in_blender";

/// The save button's action id.
pub const SAVE_ACTION_ID: &str = "pill_blender_link.save_to_source";

/// The component whose header shows the two buttons.
pub const MESH_RENDERER_COMPONENT: &str = "MeshRendererComponent";

/// The crate name: the module the editor ties the actions' liveness to.
const SOURCE_CRATE: &str = "pill_blender_link_editor";

/// How often the editor redraws while edits are streaming in.
const STREAMING_REPAINT_INTERVAL: Duration = Duration::from_millis(15);

// =============================================================================
// Registration
// =============================================================================

/// Registers the link against the editor's engine. Returns zero.
///
/// Runs on every generation of this module, so each step is safe to repeat:
/// the server is inserted only when the world does not hold one already, which
/// keeps Blender connected across a reload.
pub fn register(engine: &mut Engine) -> u32 {
    let world = engine.world_mut();
    // Step 1: the link server, kept across reloads.
    if world.get_resource::<BlenderLinkServer>().is_none() {
        world.insert_resource(BlenderLinkServer::from_environment());
    }
    // Step 2: the two buttons, as commands with their own draw functions. The
    // editor binds each to this module's liveness, so a draw function of an
    // unloaded image is never called. The queue may not exist yet when this
    // module loads before the shell, so it is claimed here the same way the
    // shell claims it. The first command registered sits rightmost on the
    // header.
    CommandRegistrations::ensure(world);
    if let Some(registrations) = world.get_resource_mut::<CommandRegistrations>() {
        registrations.push(CommandRegistration {
            owner: SOURCE_CRATE.to_string(),
            commands: vec![
                CommandSpec::new(BLENDER_ACTION_ID, "Edit in Blender")
                    .place(UiSlot::inspector_component_header(MESH_RENDERER_COMPONENT)),
                CommandSpec::new(SAVE_ACTION_ID, "Save Blender Edits")
                    .icon(Icon::FloppyDisk)
                    .place(UiSlot::inspector_component_header(MESH_RENDERER_COMPONENT)),
            ],
        });
        registrations.push_draw(CommandDrawRequest {
            id: BLENDER_ACTION_ID,
            slot: UiSlot::inspector_component_header(MESH_RENDERER_COMPONENT),
            source: SOURCE_CRATE,
            draw: inspector_button::blender_button,
        });
        registrations.push_draw(CommandDrawRequest {
            id: SAVE_ACTION_ID,
            slot: UiSlot::inspector_component_header(MESH_RENDERER_COMPONENT),
            source: SOURCE_CRATE,
            draw: inspector_button::save_button,
        });
    }
    pill_core::info!(
        target: telemetry_target::EDITOR,
        "[blender_link] pill_blender_link_editor loaded"
    );
    0
}

/// The artifact's one-time entry point: nothing is one-time here, so it runs
/// [`register`], which every generation repeats.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

// =============================================================================
// blender_link_update
// =============================================================================

/// Runs the link once per frame.
///
/// A post-project system that runs in edit mode, like the editor shell, so
/// the link keeps working while the editor edits and while the engine is
/// paused. It sits in the infrastructure band at order 0, and an editor
/// extension registers before the shell, so the shell draws the button states
/// published this frame. The editor's own resources arrive as one tuple: the
/// clicks, the replayed undo steps, the history, the unsaved work, the
/// buttons' states and the toast queue.
#[allow(clippy::type_complexity)]
#[system(
    name = "blender_link",
    run_phase = "post_project",
    run_in_edit_mode = true
)]
pub fn blender_link_update(
    mut link_server: ResMut<BlenderLinkServer>,
    mut asset_manager: ResMut<AssetManager>,
    mut mesh_renderers: Query<(Entity, &MeshRendererComponent)>,
    mut shell_state: ResMut<EguiShellState>,
    mut commands: Commands,
    editor: (
        ResMut<CommandInbox>,
        ResMut<ExtensionEditInbox>,
        ResMut<CommandHistory>,
        ResMut<UnsavedWork>,
        ResMut<CommandStates>,
        ResMut<PanelCommands>,
    ),
) {
    let (
        mut command_inbox,
        mut extension_edit_inbox,
        mut command_history,
        mut unsaved_work,
        mut command_states,
        mut panel_commands,
    ) = editor;
    let (
        Some(mut link_server),
        Some(mut asset_manager),
        Some(mut command_inbox),
        Some(mut extension_edit_inbox),
        Some(mut command_history),
        Some(mut unsaved_work),
        Some(mut command_states),
        Some(mut panel_commands),
    ) = (
        link_server.get_mut(),
        asset_manager.get_mut(),
        command_inbox.get_mut(),
        extension_edit_inbox.get_mut(),
        command_history.get_mut(),
        unsaved_work.get_mut(),
        command_states.get_mut(),
        panel_commands.get_mut(),
    )
    else {
        return;
    };
    let mut notices = Vec::new();
    // What every entity draws, read once: the button steps, the material step
    // and the published button states all need it.
    let components: HashMap<Entity, MeshRendererComponent> = mesh_renderers
        .iter_mut()
        .map(|(entity, component)| (entity, *component))
        .collect();

    // Step 1: bind the port, take Blender's connection, and take changes the
    // link did not make (a re-import after a save) as the saved state.
    link_server.poll_listener();
    link_update::adopt_outside_changes(&mut link_server, &asset_manager);

    // The add-on install's outcome, and the hint for a Blender that is open
    // but not linked.
    link_update::report_blender_setup(&mut link_server, &mut notices);

    // Step 2: the buttons the person clicked.
    let open_clicks = command_inbox.take(BLENDER_ACTION_ID);
    link_update::open_clicked_meshes(
        &mut link_server,
        &asset_manager,
        &open_clicks,
        &components,
        &mut notices,
    );
    let save_clicks = command_inbox.take(SAVE_ACTION_ID);
    link_update::save_clicked_meshes(
        &mut link_server,
        &asset_manager,
        &save_clicks,
        &components,
        &mut notices,
    );

    // Step 3: the states Undo and Redo handed back.
    let replayed = extension_edit_inbox.take(link_update::MESH_EDIT_CHANNEL);
    link_update::apply_replayed_edits(&mut link_server, &mut asset_manager, replayed);

    // Step 4: what Blender sent. Per mesh, only the newest full update and
    // the newest positions update after it matter, so a burst of updates in
    // one frame rewrites each mesh at most twice. A full update drops the
    // positions updates before it: they were written for an older topology.
    let mut newest_updates: HashMap<u64, protocol::LinkFrame> = HashMap::new();
    let mut newest_positions: HashMap<u64, protocol::LinkFrame> = HashMap::new();
    for frame in link_server.receive_frames() {
        match frame.message_type() {
            "hello" => {
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    blender = frame.header_text("blender_version").unwrap_or("unknown"),
                    "[blender_link] blender greeted"
                );
                for link_id in link_server.accept_greeting() {
                    let mesh_handle = link_server.mesh_of(link_id);
                    if let Some(mesh) =
                        mesh_handle.and_then(|handle| asset_manager.get::<Mesh>(handle))
                    {
                        link_server.send_open_mesh(link_id, mesh);
                    }
                }
            }
            "mesh_update" if link_server.is_this_session(&frame) => {
                if let Some(link_id) = frame.header_number("link_id") {
                    newest_positions.remove(&link_id);
                    newest_updates.insert(link_id, frame);
                }
            }
            "mesh_positions" if link_server.is_this_session(&frame) => {
                if let Some(link_id) = frame.header_number("link_id") {
                    newest_positions.insert(link_id, frame);
                }
            }
            // A mesh that lives only in Blender becomes an asset: the request
            // is checked here and written by a later step.
            "create_mesh" if link_server.is_this_session(&frame) => {
                link_update::accept_create_request(&mut link_server, &asset_manager, &frame);
            }
            // An object asks to be linked to a file the project already holds.
            "connect_asset" if link_server.is_this_session(&frame) => {
                link_update::connect_asset_request(&mut link_server, &mut asset_manager, &frame);
            }
            // Blender built a fresh object for the link - the one it had was
            // deleted - so the next material step sends the graph again.
            "material_request" if link_server.is_this_session(&frame) => {
                if let Some(link_id) = frame.header_number("link_id") {
                    if let Some(material_link) = link_server.material_link_mut(link_id) {
                        material_link.forget_sent_graph();
                    }
                }
            }
            // A message from an object an earlier editor run linked.
            "mesh_update" | "mesh_positions" | "material_request" | "create_mesh"
            | "connect_asset" => {}
            unknown_type => pill_core::debug!(
                target: telemetry_target::EDITOR,
                message_type = unknown_type,
                "[blender_link] message ignored"
            ),
        }
    }
    for (link_id, frame) in newest_updates {
        link_update::apply_streamed_update(&mut link_server, &mut asset_manager, link_id, &frame);
    }
    for (link_id, frame) in newest_positions {
        link_update::apply_streamed_positions(
            &mut link_server,
            &mut asset_manager,
            link_id,
            &frame,
        );
    }

    // Step 4b: the meshes that live only in Blender. They are written here,
    // before anything else reads the asset store, so a create is answered on
    // the frame it arrived; linking it and spawning its entity come next.
    link_update::create_pending_assets(&mut link_server, &mut asset_manager);
    link_update::link_created_assets(&mut link_server, &asset_manager, &mut commands);

    // Step 5: every linked entity's material, as whatever Blender is missing
    // of it. This runs after the received frames, so a click's open and its
    // material travel on the connection in that order.
    link_update::send_changed_materials(&mut link_server, &asset_manager, &components);

    // Step 6: every run of updates Blender finished becomes one undo step.
    link_update::record_finished_bursts(&mut link_server, &asset_manager, &mut command_history);

    // Step 7: the quit prompt's "Save and quit", answered once per request.
    let save_request = unsaved_work.save_request_number();
    match link_server.handled_save_request {
        Some(handled) if save_request > handled => {
            link_server.handled_save_request = Some(save_request);
            link_update::save_all_unsaved(&mut link_server, &asset_manager, &mut notices);
        }
        Some(_) => {}
        None => link_server.handled_save_request = Some(save_request),
    }

    // Step 8: publish what the editor and Blender show, then send.
    unsaved_work.report(
        link_update::UNSAVED_SOURCE,
        link_update::unsaved_items(&link_server),
    );
    let button_states = link_update::button_states(&link_server, &components);
    // Both buttons key their state by the registered component name, which is
    // what the Inspector header passes as the target's component.
    let component_name = ComponentRegistry::registered_name::<MeshRendererComponent>();
    let target = |entity| CommandTarget::Entity {
        entity,
        component: Some(component_name.clone()),
    };
    command_states.publish(
        BLENDER_ACTION_ID,
        button_states
            .open
            .into_iter()
            .map(|(entity, state)| (target(entity), state))
            .collect(),
    );
    command_states.publish(
        SAVE_ACTION_ID,
        button_states
            .save
            .into_iter()
            .map(|(entity, state)| (target(entity), state))
            .collect(),
    );
    link_server.flush();
    panel_commands.commands.extend(notices);

    // Step 9: an idle editor draws only a few times a second; while edits
    // stream in, ask for frames at once so the viewport follows Blender.
    if link_server.is_streaming() {
        if let Some(shell_state) = shell_state.get_mut() {
            shell_state
                .context
                .request_repaint_after(STREAMING_REPAINT_INTERVAL);
        }
    }
}
