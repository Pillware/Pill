//! The steps the link's system runs each frame, one function per step.
//!
//! # Responsibilities
//!
//! - Notice changes the link did not make, and take them as the saved state.
//! - Open a clicked mesh in Blender, and save a mesh to its source file.
//! - Send the material of every linked entity, as the whole node graph when
//!   it changed shape and as new values or images when only those moved.
//! - Apply what Blender streams - full updates, and positions-only updates
//!   for a drag that keeps the topology - grouping each run of updates into
//!   one undo step, and apply the states Undo and Redo hand back.
//! - Publish what the editor shows: the buttons' states, the unsaved work the
//!   quit prompt and the Assets tab read, and the save state Blender's
//!   sidebar shows.
//!
//! # Design
//!
//! Every function takes the resources it needs as plain references, so the
//! system in the crate root only borrows them and calls the steps in order.
//! Changes the person should hear about become `PanelCommand::Notify`
//! toasts.

// Standard library
use std::collections::HashMap;
use std::path::Path;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_editor_core::commands::CommandInvocation;
use pill_editor_core::extension_edits::{self, ExtensionEditDelivery, ExtensionPayload};
use pill_editor_core::history::CommandHistory;
use pill_editor_core::panels::PanelCommand;
use pill_editor_core::placements::CommandState;
use pill_editor_core::style;
use pill_editor_core::unsaved_work::UnsavedItem;
use pill_engine::{AssetManager, Commands, Entity, Handle, TransformComponent};
use pill_master_renderer_data::assets::Mesh;
use pill_master_renderer_data::MeshRendererComponent;

// Current crate
use crate::link_server::{BlenderLinkServer, CreatedAsset, OpenOutcome, PendingCreate};
use crate::material_link::{plan_material_update, MaterialAction, MaterialProblem};
use crate::mesh_link::MeshLink;
use crate::{mesh_source, protocol};

// =============================================================================
// Constants
// =============================================================================

/// The channel the link's undo steps travel on.
pub const MESH_EDIT_CHANNEL: &str = "pill_blender_link.mesh_geometry";

/// The crate that reports the link's unsaved work.
pub const UNSAVED_SOURCE: &str = "pill_blender_link_editor";

/// The Blender button's text for a mesh the link has never opened.
pub const OPEN_BUTTON_TOOLTIP: &str = "Edit this mesh in Blender (live link)";

// =============================================================================
// Outside changes
// =============================================================================

/// Takes every change the link did not make as that mesh's saved state.
///
/// The asset watcher re-imports a source file after a save (or after any
/// other program writes it), which moves the asset's content version; the
/// mesh then holds what its file holds.
pub fn adopt_outside_changes(link_server: &mut BlenderLinkServer, asset_manager: &AssetManager) {
    for mesh_link in link_server.mesh_links_mut() {
        let version = asset_manager.content_version(mesh_link.mesh_handle);
        if version == mesh_link.known_version {
            continue;
        }
        if let Some(mesh) = asset_manager.get::<Mesh>(mesh_link.mesh_handle) {
            mesh_link.adopt_outside_change(mesh, version);
            pill_core::info!(
                target: telemetry_target::EDITOR,
                link_id = mesh_link.link_id,
                mesh = mesh_link.asset_name.as_str(),
                "[blender_link] mesh changed outside the link; taken as saved"
            );
        } else {
            mesh_link.known_version = version;
        }
    }
}

// =============================================================================
// Create requests
// =============================================================================

/// Takes a `create_mesh` request from Blender and either queues it for the step
/// that writes the asset or answers with the reason it cannot be created.
///
/// What a `create_mesh` request resolves to.
#[derive(Clone, Debug)]
pub enum CreateDecision {
    /// The request can become an asset.
    Accepted(PendingCreate),
    /// The request cannot become an asset, and why.
    Refused {
        /// The name the reply names: the resolved asset name when the picked
        /// path was usable, or the picked path when it was not.
        asset_name: String,
        /// What the person has to change.
        reason: String,
    },
}

/// Resolves a `create_mesh` request against the path rules, the geometry
/// decoder and the asset store, without touching the link server.
///
/// Split from [`accept_create_request`] so the rules can be tested without a
/// socket: nothing here sends, logs or queues.
pub fn decide_create(asset_manager: &AssetManager, frame: &protocol::LinkFrame) -> CreateDecision {
    // Step 1: the picked path decides the file, so it is checked before the
    // geometry. Blender's dialog writes the file, so it may not exist yet.
    let picked_path = frame.header_text("path").unwrap_or_default();
    let asset_name = match mesh_source::created_mesh_name(Path::new(picked_path)) {
        Ok(asset_name) => asset_name,
        Err(reason) => {
            return CreateDecision::Refused {
                asset_name: picked_path.to_owned(),
                reason,
            };
        }
    };
    // Step 2: the body has to be a mesh an asset could hold.
    let vertex_count = frame.header_number("vertex_count").unwrap_or(0) as usize;
    let index_count = frame.header_number("index_count").unwrap_or(0) as usize;
    let (vertices, indices) =
        match protocol::decode_mesh_geometry(&frame.body, vertex_count, index_count) {
            Ok(geometry) => geometry,
            Err(error) => {
                return CreateDecision::Refused {
                    asset_name,
                    reason: error.to_string(),
                };
            }
        };
    // Step 3: the name has to be free.
    if asset_manager.handle_by_name::<Mesh>(&asset_name).is_some() {
        return CreateDecision::Refused {
            asset_name,
            reason: "an asset with that name is already loaded".to_owned(),
        };
    }
    // The mesh keeps the file's stem as its name, the name a person sees.
    let mesh_name = Path::new(&asset_name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| asset_name.clone());
    let mesh = Mesh::from_data(mesh_name, vertices, indices);
    CreateDecision::Accepted(PendingCreate {
        asset_name,
        geometry: protocol::encode_geometry_payload(&mesh),
    })
}

/// Takes a `create_mesh` request from Blender and either queues it for the step
/// that writes the asset or answers with the reason it cannot be created.
///
/// Nothing is written here: the request is checked against the name rules, the
/// geometry decoder and the asset store, and a refusal is the only frame this
/// step sends.
pub fn accept_create_request(
    link_server: &mut BlenderLinkServer,
    asset_manager: &AssetManager,
    frame: &protocol::LinkFrame,
) {
    match decide_create(asset_manager, frame) {
        CreateDecision::Accepted(pending) => {
            // A second request for the same name in one frame would write the
            // file twice; the first is kept and the second refused.
            if link_server.has_pending_create(&pending.asset_name) {
                refuse_create(
                    link_server,
                    &pending.asset_name,
                    "a create for that name is already in progress",
                );
                return;
            }
            pill_core::info!(
                target: telemetry_target::EDITOR,
                asset = pending.asset_name.as_str(),
                vertices = frame.header_number("vertex_count").unwrap_or(0),
                triangles = frame.header_number("index_count").unwrap_or(0) / 3,
                "[blender_link] create accepted"
            );
            link_server.push_pending_create(pending);
        }
        CreateDecision::Refused { asset_name, reason } => {
            refuse_create(link_server, &asset_name, &reason)
        }
    }
}

/// Writes and imports every create request the link accepted this frame, so a
/// refused write is answered before any entity exists for it.
///
/// A request that reaches here passed [`decide_create`]; the geometry is
/// decoded again because the pending list keeps it as the payload an undo step
/// holds, which keeps the resource's layout free of the renderer's types.
pub fn create_pending_assets(
    link_server: &mut BlenderLinkServer,
    asset_manager: &mut AssetManager,
) {
    for request in link_server.take_pending_creates() {
        let (vertices, indices) = match protocol::decode_geometry_payload(&request.geometry) {
            Ok(geometry) => geometry,
            Err(error) => {
                refuse_create(link_server, &request.asset_name, &error.to_string());
                continue;
            }
        };
        let asset_stem = Path::new(&request.asset_name)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| request.asset_name.clone());
        let mesh = Mesh::from_data(asset_stem, vertices, indices);
        match mesh_source::create_mesh_asset(&request.asset_name, &mesh, asset_manager) {
            Ok(created) => {
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    asset = request.asset_name.as_str(),
                    vertices = mesh.vertices.len(),
                    triangles = mesh.indices.len() / 3,
                    "[blender_link] created mesh asset"
                );
                link_server.push_created_asset(CreatedAsset {
                    asset_name: request.asset_name,
                    handle: created.handle,
                });
            }
            Err(reason) => refuse_create(link_server, &request.asset_name, &reason),
        }
    }
}

/// Links every mesh created this frame, spawns its entity at the origin and
/// answers the create request with the new link id.
///
/// The entity carries no material: the person assigns one, and until then the
/// material step reports that the link has none, once. The entity is created
/// through the command queue, so it appears in the world at the flush after
/// this system — which is why the material link is bound to the entity id the
/// builder returned rather than to a lookup.
pub fn link_created_assets(
    link_server: &mut BlenderLinkServer,
    asset_manager: &AssetManager,
    commands: &mut Commands<'_>,
) {
    for created in link_server.take_created_assets() {
        let Some(link_id) = link_created_mesh(link_server, asset_manager, &created) else {
            refuse_create(
                link_server,
                &created.asset_name,
                "the created mesh is not loaded",
            );
            continue;
        };
        let entity = commands
            .create_entity()
            .with(TransformComponent::default())
            .with(
                MeshRendererComponent::builder()
                    .mesh(&created.handle)
                    .build(),
            )
            .build();
        link_server.link_material(link_id, entity);
        pill_core::info!(
            target: telemetry_target::EDITOR,
            asset = created.asset_name.as_str(),
            link_id,
            entity = %entity,
            "[blender_link] created entity for the new mesh"
        );
        link_server.send_create_result(Some(link_id), &created.asset_name, "");
    }
}

/// Links a mesh created this frame against its asset, so the object in Blender
/// streams into it from now on, and returns the link id.
///
/// Returns `None` when the asset is not loaded, which the asset store should
/// never do right after an import.
pub fn link_created_mesh(
    link_server: &mut BlenderLinkServer,
    asset_manager: &AssetManager,
    created: &CreatedAsset,
) -> Option<u64> {
    let mesh = asset_manager.get::<Mesh>(created.handle)?;
    let version = asset_manager.content_version(created.handle);
    Some(link_server.link_mesh(created.handle, &created.asset_name, mesh, version))
}

/// Links a Blender object to an asset the project already holds.
///
/// The object names the file it picked; the asset name is that file below the
/// project's `res`, so the editor keeps the name rules and the Blender side
/// never has to know where `res` is. Edits from the object then stream into
/// that asset like any other link.
pub fn connect_asset_request(
    link_server: &mut BlenderLinkServer,
    asset_manager: &mut AssetManager,
    frame: &protocol::LinkFrame,
) {
    let object_name = frame.header_text("object").unwrap_or_default().to_owned();
    let picked_path = frame.header_text("path").unwrap_or_default();
    let asset_name = match mesh_source::asset_name_from_path(Path::new(picked_path)) {
        Ok(asset_name) => asset_name,
        Err(reason) => {
            connect_refused(link_server, &object_name, &reason);
            return;
        }
    };
    let handle = match mesh_source::load_mesh_asset(&asset_name, asset_manager) {
        Ok(handle) => handle,
        Err(reason) => {
            connect_refused(link_server, &object_name, &reason);
            return;
        }
    };
    let Some(mesh) = asset_manager.get::<Mesh>(handle) else {
        connect_refused(
            link_server,
            &object_name,
            "the asset is not loaded as a mesh",
        );
        return;
    };
    let version = asset_manager.content_version(handle);
    let link_id = link_server.link_mesh(handle, &asset_name, mesh, version);
    pill_core::info!(
        target: telemetry_target::EDITOR,
        asset = asset_name.as_str(),
        link_id,
        object = object_name.as_str(),
        "[blender_link] asset connected"
    );
    link_server.send_connect_result(&object_name, Some(link_id), &asset_name, "");
}

/// Answers a connect request that cannot be honoured, with the reason.
///
/// Public because the build script's function inventory takes the address of
/// every function in this crate.
pub fn connect_refused(link_server: &mut BlenderLinkServer, object_name: &str, reason: &str) {
    pill_core::warn!(
        target: telemetry_target::EDITOR,
        object = object_name,
        reason = reason,
        "[blender_link] connect refused"
    );
    link_server.send_connect_result(object_name, None, "", reason);
}

/// Answers a create request with the reason it cannot be created.
///
/// Public because the build script's function inventory takes the address of
/// every function in this crate.
pub fn refuse_create(link_server: &mut BlenderLinkServer, asset_name: &str, reason: &str) {
    pill_core::warn!(
        target: telemetry_target::EDITOR,
        asset = asset_name,
        reason = reason,
        "[blender_link] create refused"
    );
    link_server.send_create_result(None, asset_name, reason);
}

// =============================================================================
// Clicks
// =============================================================================

/// Opens the mesh of every entity whose Blender button was clicked, and links
/// the material that entity draws.
pub fn open_clicked_meshes(
    link_server: &mut BlenderLinkServer,
    asset_manager: &AssetManager,
    clicks: &[CommandInvocation],
    components: &HashMap<Entity, MeshRendererComponent>,
    notices: &mut Vec<PanelCommand>,
) {
    for click in clicks {
        let Some(entity) = click.entity() else {
            continue;
        };
        let Some((mesh_handle, mesh)) = loaded_mesh_of(asset_manager, components, entity) else {
            warn_no_mesh(entity, notices);
            continue;
        };
        let asset_name = asset_manager
            .name_of(mesh_handle)
            .unwrap_or(mesh.name.as_str())
            .to_owned();
        let version = asset_manager.content_version(mesh_handle);
        let link_id = link_server.link_mesh(mesh_handle, &asset_name, mesh, version);
        // The link shows what this entity draws, which is the newest click on
        // that mesh (D1): a second entity sharing the mesh takes the link.
        link_server.link_material(link_id, entity);
        let outcome = link_server.request_open(link_id, mesh);
        log_open_outcome(&asset_name, link_id, &outcome);
        if let OpenOutcome::LaunchFailed(detail) = outcome {
            notices.push(PanelCommand::Notify {
                warning: true,
                text: format!("Blender could not be started: {detail}"),
            });
        }
    }
}

/// Saves the mesh of every entity whose save button was clicked.
pub fn save_clicked_meshes(
    link_server: &mut BlenderLinkServer,
    asset_manager: &AssetManager,
    clicks: &[CommandInvocation],
    components: &HashMap<Entity, MeshRendererComponent>,
    notices: &mut Vec<PanelCommand>,
) {
    for click in clicks {
        let Some(entity) = click.entity() else {
            continue;
        };
        let link_id = components
            .get(&entity)
            .and_then(|component| link_server.link_of_mesh(component.mesh))
            .map(|mesh_link| mesh_link.link_id);
        match link_id {
            Some(link_id) => {
                let _ = save_link(link_server, asset_manager, link_id, notices);
            }
            None => warn_no_mesh(entity, notices),
        }
    }
}

/// Sends what each linked entity's material needs Blender to know this frame.
///
/// A link that shows nothing any more, or whose material cannot be read,
/// reports that once instead of on every frame; a material nothing changed
/// sends nothing. A parameter edit costs one small frame, not a rebuilt node
/// tree.
pub fn send_changed_materials(
    link_server: &mut BlenderLinkServer,
    asset_manager: &AssetManager,
    components: &HashMap<Entity, MeshRendererComponent>,
) {
    if !link_server.is_connected() {
        return;
    }
    let link_ids: Vec<u64> = link_server
        .material_links()
        .iter()
        .map(|material_link| material_link.link_id())
        .collect();
    for link_id in link_ids {
        let component = link_server
            .material_link(link_id)
            .and_then(|material_link| components.get(&material_link.entity()))
            .copied();
        let action = match link_server.material_link_mut(link_id) {
            Some(material_link) => {
                plan_material_update(material_link, asset_manager, component.as_ref())
            }
            None => continue,
        };
        match action {
            MaterialAction::Full {
                name,
                graph,
                fallback,
            } => {
                // A field or slot outside the supported subset stays
                // unconnected, and the reason names what the shader writes
                // there - or which slot has no standard name.
                for refusal in &graph.refusals {
                    pill_core::warn!(
                        target: telemetry_target::EDITOR,
                        link_id,
                        field = refusal.field.as_str(),
                        reason = refusal.reason.as_str(),
                        "[blender_link] material field not translated"
                    );
                }
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    link_id,
                    material = name.as_str(),
                    reason = "graph",
                    fallback,
                    nodes = graph.nodes.len(),
                    "[blender_link] material sent"
                );
                link_server.send_material(link_id, &name, &graph);
            }
            MaterialAction::Values { parameters } => {
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    link_id,
                    reason = "values",
                    parameters = parameters.len(),
                    "[blender_link] material sent"
                );
                link_server.send_material_values(link_id, &parameters);
            }
            MaterialAction::Images { images } => {
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    link_id,
                    reason = "images",
                    images = images.len(),
                    "[blender_link] material sent"
                );
                link_server.send_material_images(link_id, &images);
            }
            MaterialAction::Problem(problem) => match problem {
                MaterialProblem::MissingComponent => pill_core::debug!(
                    target: telemetry_target::EDITOR,
                    link_id,
                    "[blender_link] the linked entity no longer draws a mesh"
                ),
                problem => pill_core::warn!(
                    target: telemetry_target::EDITOR,
                    link_id,
                    reason = problem.to_string(),
                    "[blender_link] material not sent"
                ),
            },
            MaterialAction::None => {}
        }
    }
}

/// Writes a linked mesh over its source file and records the saved state.
///
/// The person hears about the outcome through a toast, and Blender's sidebar
/// through the status sent with the next flush.
///
/// # Errors
///
/// Returns the reason the mesh was not saved: the link or the mesh is gone,
/// the mesh has no project source file, or the write failed.
pub fn save_link(
    link_server: &mut BlenderLinkServer,
    asset_manager: &AssetManager,
    link_id: u64,
    notices: &mut Vec<PanelCommand>,
) -> Result<(), String> {
    let saved = (|| {
        let mesh_link = link_server
            .mesh_link(link_id)
            .ok_or_else(|| format!("link {link_id} is not open"))?;
        let mesh = asset_manager
            .get::<Mesh>(mesh_link.mesh_handle)
            .ok_or_else(|| format!("{} is no longer loaded", mesh_link.asset_name))?;
        mesh_source::save_mesh_to_source(mesh, &mesh_link.asset_name)
    })();
    let asset_name = link_server
        .mesh_link(link_id)
        .map(|mesh_link| mesh_link.asset_name.clone())
        .unwrap_or_default();
    match saved {
        Ok(source_path) => {
            if let Some(mesh_link) = link_server.mesh_link_mut(link_id) {
                mesh_link.mark_saved();
            }
            pill_core::info!(
                target: telemetry_target::EDITOR,
                link_id,
                path = %source_path.display(),
                "[blender_link] mesh saved to its source"
            );
            notices.push(PanelCommand::Notify {
                warning: false,
                text: format!("Saved {asset_name}"),
            });
            Ok(())
        }
        Err(reason) => {
            pill_core::warn!(
                target: telemetry_target::EDITOR,
                link_id,
                reason = reason.as_str(),
                "[blender_link] mesh not saved"
            );
            notices.push(PanelCommand::Notify {
                warning: true,
                text: format!("Not saved: {reason}"),
            });
            Err(reason)
        }
    }
}

/// Saves every linked mesh that holds unsaved changes, as the quit prompt's
/// "Save and quit" asks.
pub fn save_all_unsaved(
    link_server: &mut BlenderLinkServer,
    asset_manager: &AssetManager,
    notices: &mut Vec<PanelCommand>,
) {
    let unsaved_link_ids: Vec<u64> = link_server
        .mesh_links()
        .iter()
        .filter(|mesh_link| mesh_link.is_unsaved())
        .map(|mesh_link| mesh_link.link_id)
        .collect();
    for link_id in unsaved_link_ids {
        let _ = save_link(link_server, asset_manager, link_id, notices);
    }
}

/// Tells the person what the background add-on install did, and, once, how
/// to link a Blender that is running but has not connected.
pub fn report_blender_setup(link_server: &mut BlenderLinkServer, notices: &mut Vec<PanelCommand>) {
    match link_server.poll_addon_install() {
        Some(Ok(())) => {
            pill_core::info!(
                target: telemetry_target::EDITOR,
                "[blender_link] blender add-on installed"
            );
            notices.push(PanelCommand::Notify {
                warning: false,
                text: "Installed the Pill Live Link add-on in Blender: every Blender you open now connects to the editor".to_owned(),
            });
        }
        Some(Err(reason)) => {
            pill_core::warn!(
                target: telemetry_target::EDITOR,
                reason = reason.as_str(),
                "[blender_link] blender add-on install failed"
            );
            notices.push(PanelCommand::Notify {
                warning: true,
                text: format!("The Blender add-on could not be installed ({reason}); install blender_addon/pill_blender_link.py by hand"),
            });
        }
        None => {}
    }
    if link_server.unlinked_blender_reminder_due() {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            "[blender_link] the running blender has not connected"
        );
        notices.push(PanelCommand::Notify {
            warning: true,
            text: "Blender is open but not linked. In Blender, enable \"Pill Engine Live Link\" (Edit > Preferences > Add-ons), or restart Blender - the mesh opens as soon as it connects".to_owned(),
        });
    }
}

// =============================================================================
// Geometry changes
// =============================================================================

/// Writes one full streamed update into its mesh. Returns whether the mesh
/// changed.
///
/// The update's `topology_id` becomes the vertex order later positions-only
/// updates are applied to. The first update of a run opens an edit burst
/// holding the geometry before it. The same surface in another vertex order -
/// what Blender sends when the editor asked for a full update - is taken in
/// quietly: no undo step and no unsaved change, only Blender's vertex order.
/// An update identical to the asset is dropped.
pub fn apply_streamed_update(
    link_server: &mut BlenderLinkServer,
    asset_manager: &mut AssetManager,
    link_id: u64,
    frame: &protocol::LinkFrame,
) -> bool {
    let Some(mesh_handle) = link_server.mesh_of(link_id) else {
        return false;
    };
    let topology_id = frame.header_number("topology_id");
    let vertex_count = frame.header_number("vertex_count").unwrap_or(0) as usize;
    let index_count = frame.header_number("index_count").unwrap_or(0) as usize;
    let (vertices, indices) =
        match protocol::decode_mesh_geometry(&frame.body, vertex_count, index_count) {
            Ok(geometry) => geometry,
            Err(error) => {
                pill_core::warn!(
                    target: telemetry_target::EDITOR,
                    error = %error,
                    "[blender_link] streamed geometry refused"
                );
                return false;
            }
        };
    let Some(mesh_before) = asset_manager.get::<Mesh>(mesh_handle) else {
        return false;
    };
    let mut candidate = Mesh::from_data(mesh_before.name.clone(), vertices, indices);
    // Step 1: an exact copy changes nothing but the base later positions
    // apply to.
    if protocol::same_vertex_order(&candidate, mesh_before) {
        if let Some(mesh_link) = link_server.mesh_link_mut(link_id) {
            mesh_link.stream_topology = topology_id;
            mesh_link.resync_requested = false;
        }
        return false;
    }
    // Step 2: the same surface in Blender's order is taken without a step.
    let same_surface =
        protocol::geometry_fingerprint(&candidate) == protocol::geometry_fingerprint(mesh_before);
    if !same_surface {
        if let Some(mesh_link) = link_server.mesh_link_mut(link_id) {
            mesh_link.begin_update(mesh_before);
        }
    }
    // Step 3: replace the geometry; `get_mut` moves the content version,
    // which makes the renderer upload the mesh again.
    candidate.calculate_tangents();
    let Some(mesh) = asset_manager.get_mut::<Mesh>(mesh_handle) else {
        return false;
    };
    mesh.vertices = candidate.vertices;
    mesh.indices = candidate.indices;
    let version = asset_manager.content_version(mesh_handle);
    if let (Some(mesh_link), Some(mesh)) = (
        link_server.mesh_link_mut(link_id),
        asset_manager.get::<Mesh>(mesh_handle),
    ) {
        mesh_link.record_change(mesh, version, !same_surface);
        mesh_link.stream_topology = topology_id;
        mesh_link.resync_requested = false;
        pill_core::debug!(
            target: telemetry_target::EDITOR,
            mesh = mesh_link.asset_name.as_str(),
            vertices = vertex_count,
            triangles = index_count / 3,
            reordered_only = same_surface,
            "[blender_link] mesh streamed"
        );
    }
    !same_surface
}

/// Writes one positions-only update into its mesh: new positions and normals
/// for a drag that left the topology, the texture coordinates and the vertex
/// order alone. Returns whether the mesh changed.
///
/// The update names the full update whose vertex order it was written for.
/// When the asset is not in that order any more - an undo, a re-import, a
/// full update that has not arrived - nothing is written and Blender is asked
/// for a full update instead, once.
pub fn apply_streamed_positions(
    link_server: &mut BlenderLinkServer,
    asset_manager: &mut AssetManager,
    link_id: u64,
    frame: &protocol::LinkFrame,
) -> bool {
    let Some(mesh_link) = link_server.mesh_link(link_id) else {
        return false;
    };
    let mesh_handle = mesh_link.mesh_handle;
    let vertex_count = frame.header_number("vertex_count").unwrap_or(0) as usize;
    // Step 1: the update must be for the vertex order the asset is in.
    let topology_id = frame.header_number("topology_id");
    let base_matches = topology_id.is_some()
        && mesh_link.stream_topology == topology_id
        && mesh_link.known_version == asset_manager.content_version(mesh_handle)
        && asset_manager
            .get::<Mesh>(mesh_handle)
            .is_some_and(|mesh| mesh.vertices.len() == vertex_count);
    if !base_matches {
        request_resync(link_server, link_id);
        return false;
    }
    let (positions, normals) = match protocol::decode_mesh_positions(&frame.body, vertex_count) {
        Ok(positions_and_normals) => positions_and_normals,
        Err(error) => {
            pill_core::warn!(
                target: telemetry_target::EDITOR,
                error = %error,
                "[blender_link] streamed positions refused"
            );
            return false;
        }
    };
    // Step 2: positions and normals equal to the asset's are an echo.
    let Some(mesh_before) = asset_manager.get::<Mesh>(mesh_handle) else {
        return false;
    };
    let unchanged = mesh_before
        .vertices
        .iter()
        .zip(positions.iter().zip(&normals))
        .all(|(vertex, (position, normal))| {
            vertex.position == *position && vertex.normal == *normal
        });
    if unchanged {
        return false;
    }
    if let Some(mesh_link) = link_server.mesh_link_mut(link_id) {
        mesh_link.begin_update(mesh_before);
    }
    // Step 3: overwrite the two attributes in place; the indices and the
    // texture coordinates stay, so the renderer can rewrite its buffers in
    // place too.
    let Some(mesh) = asset_manager.get_mut::<Mesh>(mesh_handle) else {
        return false;
    };
    for (vertex, (position, normal)) in mesh
        .vertices
        .iter_mut()
        .zip(positions.into_iter().zip(normals))
    {
        vertex.position = position;
        vertex.normal = normal;
    }
    mesh.calculate_tangents();
    let version = asset_manager.content_version(mesh_handle);
    if let (Some(mesh_link), Some(mesh)) = (
        link_server.mesh_link_mut(link_id),
        asset_manager.get::<Mesh>(mesh_handle),
    ) {
        mesh_link.record_change(mesh, version, true);
    }
    true
}

/// Asks Blender for a full update of a link, once until one arrives.
pub fn request_resync(link_server: &mut BlenderLinkServer, link_id: u64) {
    let Some(mesh_link) = link_server.mesh_link_mut(link_id) else {
        return;
    };
    if mesh_link.resync_requested {
        return;
    }
    mesh_link.resync_requested = true;
    pill_core::info!(
        target: telemetry_target::EDITOR,
        link_id,
        "[blender_link] positions update for another vertex order; full update requested"
    );
    link_server.send_resync(link_id);
}

/// Turns every run of updates Blender has finished into one undo step.
pub fn record_finished_bursts(
    link_server: &mut BlenderLinkServer,
    asset_manager: &AssetManager,
    history: &mut CommandHistory,
) {
    for mesh_link in link_server.mesh_links_mut() {
        let Some(burst) = mesh_link.take_idle_burst() else {
            continue;
        };
        let Some(mesh) = asset_manager.get::<Mesh>(mesh_link.mesh_handle) else {
            continue;
        };
        let after = ExtensionPayload::new(protocol::encode_geometry_payload(mesh));
        let label = format!("Blender edit {}", mesh_link.asset_name);
        let entry = extension_edits::extension_edit_entry(
            label.as_str(),
            MESH_EDIT_CHANNEL,
            &mesh_link.link_id.to_string(),
            burst.before,
            after,
        );
        let pushed = history.push(entry, 0, CommandHistory::DEFAULT_COALESCE_WINDOW);
        pill_core::info!(
            target: telemetry_target::EDITOR,
            label = label.as_str(),
            updates = burst.update_count,
            steps = pushed.steps,
            "[editor] history push"
        );
    }
}

/// Applies the states Undo and Redo handed back, and sends each to Blender so
/// its object matches the editor again.
pub fn apply_replayed_edits(
    link_server: &mut BlenderLinkServer,
    asset_manager: &mut AssetManager,
    deliveries: Vec<ExtensionEditDelivery>,
) {
    for delivery in deliveries {
        let Some(link_id) = delivery.subject.parse::<u64>().ok() else {
            continue;
        };
        let Some(mesh_handle) = link_server.mesh_of(link_id) else {
            pill_core::warn!(
                target: telemetry_target::EDITOR,
                link_id,
                "[blender_link] undo step for a link that is gone"
            );
            continue;
        };
        let (vertices, indices) = match protocol::decode_geometry_payload(delivery.payload.bytes())
        {
            Ok(geometry) => geometry,
            Err(error) => {
                pill_core::warn!(
                    target: telemetry_target::EDITOR,
                    error = %error,
                    "[blender_link] undo step refused"
                );
                continue;
            }
        };
        let Some(mesh) = asset_manager.get_mut::<Mesh>(mesh_handle) else {
            continue;
        };
        mesh.vertices = vertices;
        mesh.indices = indices;
        mesh.calculate_tangents();
        let version = asset_manager.content_version(mesh_handle);
        let Some(mesh) = asset_manager.get::<Mesh>(mesh_handle) else {
            continue;
        };
        if let Some(mesh_link) = link_server.mesh_link_mut(link_id) {
            // A run still arriving started from a state the undo replaced,
            // and Blender's vertex order is gone until it sends a full
            // update again.
            mesh_link.burst = None;
            mesh_link.stream_topology = None;
            mesh_link.record_change(mesh, version, false);
        }
        link_server.send_replace_mesh(link_id, mesh);
        pill_core::info!(
            target: telemetry_target::EDITOR,
            link_id,
            "[blender_link] undo step applied"
        );
    }
}

// =============================================================================
// What the editor and Blender show
// =============================================================================

/// The states the two buttons show on each entity that draws a mesh.
pub struct ButtonStates {
    /// The Blender button's state, per entity.
    pub open: HashMap<Entity, CommandState>,
    /// The save button's state, per entity. An entity whose mesh was never
    /// linked publishes a hidden state, so its save button is not drawn.
    pub save: HashMap<Entity, CommandState>,
}

/// Builds both buttons' states for every entity that draws a mesh.
pub fn button_states(
    link_server: &BlenderLinkServer,
    components: &HashMap<Entity, MeshRendererComponent>,
) -> ButtonStates {
    let mut button_states = ButtonStates {
        open: HashMap::new(),
        save: HashMap::new(),
    };
    for (entity, component) in components {
        let Some(mesh_link) = link_server.link_of_mesh(component.mesh) else {
            // A mesh the link does not know yet still offers the Blender
            // button - that is how it is first opened - but no save button.
            button_states.open.insert(
                *entity,
                CommandState::default().with_tooltip(vec![OPEN_BUTTON_TOOLTIP.to_owned()]),
            );
            button_states.save.insert(*entity, CommandState::hidden());
            continue;
        };
        button_states
            .open
            .insert(*entity, open_button_status(link_server, mesh_link));
        button_states
            .save
            .insert(*entity, save_button_status(mesh_link));
    }
    button_states
}

/// The Blender button's state for a linked mesh.
pub fn open_button_status(link_server: &BlenderLinkServer, mesh_link: &MeshLink) -> CommandState {
    let (indicator, first_line) = if link_server.is_waiting(mesh_link.link_id) {
        (
            style::warning(),
            "Waiting for Blender to connect…".to_owned(),
        )
    } else if !link_server.is_connected() {
        (
            style::text_muted(),
            "Blender is not connected - click to start it again".to_owned(),
        )
    } else if mesh_link.is_streaming() {
        (style::success(), "Streaming from Blender".to_owned())
    } else {
        (
            style::accent(),
            "Linked to Blender - click to select it there".to_owned(),
        )
    };
    let mut tooltip = vec![
        first_line,
        format!(
            "{} · {} triangles",
            mesh_link.asset_name, mesh_link.triangle_count
        ),
    ];
    if let Some(streamed_at) = mesh_link.last_streamed_at {
        tooltip.push(format!(
            "Last update {:.1} s ago",
            streamed_at.elapsed().as_secs_f32()
        ));
    }
    if mesh_link.is_unsaved() {
        tooltip.push("Unsaved: the change is in memory only".to_owned());
    }
    CommandState {
        indicator: Some(indicator.to_array()),
        attention: mesh_link.is_unsaved().then(|| style::warning().to_array()),
        enabled: true,
        tooltip: Some(tooltip),
        ..CommandState::default()
    }
}

/// The save button's state for a linked mesh.
pub fn save_button_status(mesh_link: &MeshLink) -> CommandState {
    let source_problem = mesh_source::source_file_path(&mesh_link.asset_name).err();
    let tooltip = match (&source_problem, mesh_link.is_unsaved()) {
        (Some(problem), _) => vec!["Cannot save".to_owned(), problem.clone()],
        (None, true) => vec![format!("Save to {}", mesh_link.asset_name)],
        (None, false) => vec![format!("{} is saved", mesh_link.asset_name)],
    };
    CommandState {
        indicator: None,
        attention: mesh_link.is_unsaved().then(|| style::warning().to_array()),
        enabled: source_problem.is_none() && mesh_link.is_unsaved(),
        tooltip: Some(tooltip),
        ..CommandState::default()
    }
}

/// The unsaved work the quit prompt lists and the Assets tab marks.
pub fn unsaved_items(link_server: &BlenderLinkServer) -> Vec<UnsavedItem> {
    link_server
        .mesh_links()
        .iter()
        .filter(|mesh_link| mesh_link.is_unsaved())
        .map(|mesh_link| UnsavedItem {
            source: UNSAVED_SOURCE.to_owned(),
            label: format!("{} - edited in Blender", mesh_link.asset_name),
            asset_path: Some(mesh_link.asset_name.clone()),
        })
        .collect()
}

// =============================================================================
// Helpers
// =============================================================================

/// Returns an entity's mesh handle and the loaded mesh, when both exist.
pub fn loaded_mesh_of<'a>(
    asset_manager: &'a AssetManager,
    components: &HashMap<Entity, MeshRendererComponent>,
    entity: Entity,
) -> Option<(Handle<Mesh>, &'a Mesh)> {
    let mesh_handle = components.get(&entity)?.mesh;
    Some((mesh_handle, asset_manager.get::<Mesh>(mesh_handle)?))
}

/// Logs and toasts that a clicked entity has no loaded mesh.
pub fn warn_no_mesh(entity: Entity, notices: &mut Vec<PanelCommand>) {
    pill_core::warn!(
        target: telemetry_target::EDITOR,
        entity = %entity,
        "[blender_link] the entity has no loaded mesh"
    );
    notices.push(PanelCommand::Notify {
        warning: true,
        text: format!("Entity {entity} has no loaded mesh to edit"),
    });
}

/// Logs what an open request did, as one line per click.
pub fn log_open_outcome(mesh_name: &str, link_id: u64, outcome: &OpenOutcome) {
    match outcome {
        OpenOutcome::Sent => pill_core::info!(
            target: telemetry_target::EDITOR,
            mesh = mesh_name,
            link_id,
            "[blender_link] mesh opened in blender"
        ),
        OpenOutcome::LaunchedBlender(executable_path) => pill_core::info!(
            target: telemetry_target::EDITOR,
            mesh = mesh_name,
            link_id,
            executable = executable_path.as_str(),
            "[blender_link] starting blender"
        ),
        OpenOutcome::WaitingForRunningBlender => pill_core::info!(
            target: telemetry_target::EDITOR,
            mesh = mesh_name,
            link_id,
            "[blender_link] waiting for the running blender to connect"
        ),
        OpenOutcome::WaitingForBlender => pill_core::info!(
            target: telemetry_target::EDITOR,
            mesh = mesh_name,
            link_id,
            "[blender_link] waiting for blender to connect"
        ),
        OpenOutcome::LaunchFailed(detail) => pill_core::warn!(
            target: telemetry_target::EDITOR,
            mesh = mesh_name,
            detail = detail.as_str(),
            "[blender_link] blender could not be started"
        ),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An asset store holding one triangle, linked as `models/t.obj`.
    fn linked_triangle() -> (BlenderLinkServer, AssetManager, Handle<Mesh>, u64) {
        let mut asset_manager = AssetManager::new();
        let mesh_handle = asset_manager.add(Mesh::triangle());
        let mut link_server = BlenderLinkServer::from_environment();
        let version = asset_manager.content_version(mesh_handle);
        let mesh = asset_manager.get::<Mesh>(mesh_handle).unwrap();
        let link_id = link_server.link_mesh(mesh_handle, "models/t.obj", mesh, version);
        (link_server, asset_manager, mesh_handle, link_id)
    }

    fn full_update(link_id: u64, topology_id: u64, mesh: &Mesh) -> protocol::LinkFrame {
        protocol::LinkFrame {
            header: json!({
                "type": "mesh_update", "link_id": link_id, "topology_id": topology_id,
                "vertex_count": mesh.vertices.len(), "index_count": mesh.indices.len(),
            }),
            body: protocol::encode_mesh_geometry(mesh),
        }
    }

    fn positions_update(link_id: u64, topology_id: u64, mesh: &Mesh) -> protocol::LinkFrame {
        let mut body = Vec::new();
        for vertex in &mesh.vertices {
            body.extend(vertex.position.iter().flat_map(|value| value.to_le_bytes()));
        }
        for vertex in &mesh.vertices {
            body.extend(vertex.normal.iter().flat_map(|value| value.to_le_bytes()));
        }
        protocol::LinkFrame {
            header: json!({
                "type": "mesh_positions", "link_id": link_id, "topology_id": topology_id,
                "vertex_count": mesh.vertices.len(),
            }),
            body,
        }
    }

    /// A full update sets the vertex order; a positions update for that
    /// order moves the vertices and keeps the indices.
    #[test]
    fn a_positions_update_applies_to_the_full_update_it_was_written_for() {
        let (mut link_server, mut asset_manager, mesh_handle, link_id) = linked_triangle();
        let mut moved = Mesh::triangle();
        moved.vertices[0].position[1] -= 1.0;
        assert!(apply_streamed_update(
            &mut link_server,
            &mut asset_manager,
            link_id,
            &full_update(link_id, 3, &moved)
        ));
        assert_eq!(
            link_server.mesh_link(link_id).unwrap().stream_topology,
            Some(3)
        );

        moved.vertices[2].position[1] += 2.0;
        assert!(apply_streamed_positions(
            &mut link_server,
            &mut asset_manager,
            link_id,
            &positions_update(link_id, 3, &moved)
        ));
        let mesh = asset_manager.get::<Mesh>(mesh_handle).unwrap();
        assert_eq!(mesh.vertices[2].position, moved.vertices[2].position);
        assert_eq!(mesh.indices, moved.indices);
        assert!(link_server.mesh_link(link_id).unwrap().is_unsaved());
    }

    /// A positions update for another vertex order changes nothing and asks
    /// for a full update, once.
    #[test]
    fn a_positions_update_for_another_order_asks_for_a_full_update() {
        let (mut link_server, mut asset_manager, mesh_handle, link_id) = linked_triangle();
        let mut moved = Mesh::triangle();
        moved.vertices[0].position[1] += 5.0;
        // No full update arrived yet, so there is no vertex order to apply to.
        assert!(!apply_streamed_positions(
            &mut link_server,
            &mut asset_manager,
            link_id,
            &positions_update(link_id, 3, &moved)
        ));
        assert!(link_server.mesh_link(link_id).unwrap().resync_requested);
        let mesh = asset_manager.get::<Mesh>(mesh_handle).unwrap();
        assert_eq!(
            mesh.vertices[0].position,
            Mesh::triangle().vertices[0].position
        );
        // A full update clears the request, and an update for an older order
        // is refused again.
        assert!(!apply_streamed_update(
            &mut link_server,
            &mut asset_manager,
            link_id,
            &full_update(link_id, 4, &Mesh::triangle())
        ));
        assert!(!link_server.mesh_link(link_id).unwrap().resync_requested);
        assert!(!apply_streamed_positions(
            &mut link_server,
            &mut asset_manager,
            link_id,
            &positions_update(link_id, 3, &moved)
        ));
    }

    /// The same surface in Blender's vertex order is taken quietly: no undo
    /// run opens and the mesh stays saved.
    #[test]
    fn a_reordered_copy_is_adopted_without_an_edit() {
        let (mut link_server, mut asset_manager, mesh_handle, link_id) = linked_triangle();
        let mut reordered = Mesh::triangle();
        reordered.vertices.swap(0, 2);
        reordered.indices = vec![1, 0, 2];
        assert!(!apply_streamed_update(
            &mut link_server,
            &mut asset_manager,
            link_id,
            &full_update(link_id, 7, &reordered)
        ));
        let mesh_link = link_server.mesh_link(link_id).unwrap();
        assert!(!mesh_link.is_unsaved());
        assert!(mesh_link.burst.is_none());
        assert_eq!(mesh_link.stream_topology, Some(7));
        assert_eq!(
            asset_manager.get::<Mesh>(mesh_handle).unwrap().indices,
            vec![1, 0, 2]
        );
    }

    /// A create request for a path under `res` becomes a pending asset whose
    /// geometry is the mesh that was sent.
    #[test]
    fn a_create_request_is_accepted_with_its_geometry() {
        mesh_source::in_scratch_asset_directory(|root| {
            std::fs::create_dir_all(root.join("models")).expect("a models folder");
            let mesh = Mesh::triangle();
            let frame = protocol::LinkFrame {
                header: serde_json::json!({
                    "type": "create_mesh",
                    "path": root.join("models/teapot.obj").to_string_lossy(),
                    "vertex_count": mesh.vertices.len(),
                    "index_count": mesh.indices.len(),
                }),
                body: protocol::encode_mesh_geometry(&mesh),
            };
            match decide_create(&AssetManager::new(), &frame) {
                CreateDecision::Accepted(pending) => {
                    assert_eq!(pending.asset_name, "models/teapot.obj");
                    let (vertices, indices) =
                        protocol::decode_geometry_payload(&pending.geometry).unwrap();
                    assert_eq!(vertices.len(), mesh.vertices.len());
                    assert_eq!(indices, mesh.indices);
                }
                decision => panic!("expected an accepted create: {decision:?}"),
            }
        });
    }

    /// A path outside the project's `res` is refused, and the reply names the
    /// path as it was picked.
    #[test]
    fn a_create_request_outside_res_is_refused() {
        mesh_source::in_scratch_asset_directory(|root| {
            let mesh = Mesh::triangle();
            let outside = root
                .parent()
                .expect("the scratch folder has a parent")
                .join("pill-blender-escape.obj");
            let frame = protocol::LinkFrame {
                header: serde_json::json!({
                    "type": "create_mesh",
                    "path": outside.to_string_lossy(),
                    "vertex_count": mesh.vertices.len(),
                    "index_count": mesh.indices.len(),
                }),
                body: protocol::encode_mesh_geometry(&mesh),
            };
            match decide_create(&AssetManager::new(), &frame) {
                CreateDecision::Refused { asset_name, reason } => {
                    assert_eq!(asset_name, outside.to_string_lossy());
                    assert!(reason.contains("outside"), "{reason}");
                }
                decision => panic!("expected a refused create: {decision:?}"),
            }
        });
    }

    /// A body whose length does not match its counts is refused with the
    /// decoder's reason.
    #[test]
    fn a_create_request_with_a_short_body_is_refused() {
        mesh_source::in_scratch_asset_directory(|root| {
            std::fs::create_dir_all(root.join("models")).expect("a models folder");
            let mesh = Mesh::triangle();
            let mut body = protocol::encode_mesh_geometry(&mesh);
            body.truncate(body.len() - 4);
            let frame = protocol::LinkFrame {
                header: serde_json::json!({
                    "type": "create_mesh",
                    "path": root.join("models/teapot.obj").to_string_lossy(),
                    "vertex_count": mesh.vertices.len(),
                    "index_count": mesh.indices.len(),
                }),
                body,
            };
            match decide_create(&AssetManager::new(), &frame) {
                CreateDecision::Refused { reason, .. } => {
                    assert!(reason.contains("bytes"), "{reason}");
                }
                decision => panic!("expected a refused create: {decision:?}"),
            }
        });
    }

    /// A body that is not whole triangles is refused too.
    #[test]
    fn a_create_request_without_triangles_is_refused() {
        mesh_source::in_scratch_asset_directory(|root| {
            std::fs::create_dir_all(root.join("models")).expect("a models folder");
            let mut mesh = Mesh::triangle();
            mesh.indices = vec![0, 1];
            let frame = protocol::LinkFrame {
                header: serde_json::json!({
                    "type": "create_mesh",
                    "path": root.join("models/teapot.obj").to_string_lossy(),
                    "vertex_count": mesh.vertices.len(),
                    "index_count": mesh.indices.len(),
                }),
                body: protocol::encode_mesh_geometry(&mesh),
            };
            match decide_create(&AssetManager::new(), &frame) {
                CreateDecision::Refused { reason, .. } => {
                    assert!(reason.contains("triangles"), "{reason}");
                }
                decision => panic!("expected a refused create: {decision:?}"),
            }
        });
    }

    /// A name an asset already holds is refused, so a create can never shadow
    /// an asset the editor is drawing.
    #[test]
    fn a_create_request_for_a_loaded_asset_is_refused() {
        mesh_source::in_scratch_asset_directory(|root| {
            std::fs::create_dir_all(root.join("models")).expect("a models folder");
            let mesh = Mesh::triangle();
            let frame = protocol::LinkFrame {
                header: serde_json::json!({
                    "type": "create_mesh",
                    "path": root.join("models/teapot.obj").to_string_lossy(),
                    "vertex_count": mesh.vertices.len(),
                    "index_count": mesh.indices.len(),
                }),
                body: protocol::encode_mesh_geometry(&mesh),
            };
            let mut asset_manager = AssetManager::new();
            asset_manager
                .add_named("models/teapot.obj", Mesh::triangle())
                .expect("the name is free");
            match decide_create(&asset_manager, &frame) {
                CreateDecision::Refused { reason, .. } => {
                    assert!(reason.contains("already loaded"), "{reason}");
                }
                decision => panic!("expected a refused create: {decision:?}"),
            }
        });
    }

    /// A mesh created from Blender joins the link table against the asset it
    /// was imported into, with the geometry it was written from as the saved
    /// state.
    #[test]
    fn a_created_mesh_joins_the_link_table() {
        let mut asset_manager = AssetManager::new();
        let handle = asset_manager
            .add_named("models/teapot.obj", Mesh::triangle())
            .expect("the name is free");
        let mut link_server = BlenderLinkServer::from_environment();
        let created = CreatedAsset {
            asset_name: "models/teapot.obj".to_owned(),
            handle,
        };
        let link_id =
            link_created_mesh(&mut link_server, &asset_manager, &created).expect("the mesh loads");
        let mesh_link = link_server.mesh_link(link_id).expect("the link exists");
        assert_eq!(mesh_link.asset_name, "models/teapot.obj");
        assert!(
            !mesh_link.is_unsaved(),
            "a new link starts from the saved geometry"
        );
    }
}
