//! What the link knows about one mesh open in Blender.
//!
//! # Responsibilities
//!
//! - Tell an unsaved mesh from a saved one: the fingerprint of the geometry
//!   the asset holds now, against the fingerprint of the geometry its source
//!   file holds.
//! - Notice a change the link did not make (the asset watcher re-importing
//!   the file after a save, or anything else), through the asset's content
//!   version, and take that state as the saved one.
//! - Group a run of streamed updates into one edit burst, so one drag in
//!   Blender becomes one undo step in the editor.
//! - Keep what the Inspector button shows: triangle count and when the last
//!   update arrived.
//! - Remember which of Blender's full updates the asset's vertex order comes
//!   from, so a positions-only update is applied only to the vertex order it
//!   was written for.

// Standard library
use std::time::Duration;

// External crates
use pill_core::platform::Instant;
use pill_editor_core::extension_edits::ExtensionPayload;
use pill_engine::Handle;
use pill_master_renderer_data::assets::Mesh;

// Current crate
use crate::protocol;

// =============================================================================
// Constants
// =============================================================================

/// How long Blender has to stay quiet before a run of updates ends and
/// becomes one undo step.
pub const EDIT_BURST_IDLE: Duration = Duration::from_millis(400);

/// How long after the last update a mesh still counts as streaming, for the
/// button's state and the editor's redraw rate.
pub const STREAMING_WINDOW: Duration = Duration::from_secs(1);

// =============================================================================
// Types
// =============================================================================

/// A run of streamed updates that has not become an undo step yet.
#[derive(Clone, Debug)]
pub struct EditBurst {
    /// The geometry before the first update of the run, which Undo restores.
    pub before: ExtensionPayload,
    /// When the newest update of the run arrived.
    pub last_update_at: Instant,
    /// How many updates the run holds.
    pub update_count: u32,
}

/// One mesh asset open in Blender.
#[derive(Clone, Debug)]
pub struct MeshLink {
    /// The id both sides name the mesh by.
    pub link_id: u64,
    /// The asset the streamed geometry is written into.
    pub mesh_handle: Handle<Mesh>,
    /// The asset's name: the path below `res` for an imported mesh, which is
    /// also the file a save writes.
    pub asset_name: String,
    /// The asset's content version after the link's own last change, so a
    /// change by anyone else shows as a different version.
    pub known_version: Option<u64>,
    /// The fingerprint of the geometry the source file holds.
    pub saved_fingerprint: u64,
    /// The fingerprint of the geometry the asset holds now.
    pub current_fingerprint: u64,
    /// The run of updates still arriving, when there is one.
    pub burst: Option<EditBurst>,
    /// The mesh's triangle count, for the button's hover text.
    pub triangle_count: usize,
    /// When the last streamed update arrived.
    pub last_streamed_at: Option<Instant>,
    /// The topology id of Blender's full update the asset's vertices are in
    /// the order of. `None` once anything else rewrote the mesh (an undo, a
    /// re-import), which makes the next positions-only update ask for a full
    /// one.
    pub stream_topology: Option<u64>,
    /// Whether a full update was asked for and has not arrived yet, so it is
    /// asked for once.
    pub resync_requested: bool,
}

impl MeshLink {
    /// Links a mesh in its current state, which is taken as the saved one.
    pub fn new(
        link_id: u64,
        mesh_handle: Handle<Mesh>,
        asset_name: String,
        mesh: &Mesh,
        version: Option<u64>,
    ) -> Self {
        let fingerprint = protocol::geometry_fingerprint(mesh);
        Self {
            link_id,
            mesh_handle,
            asset_name,
            known_version: version,
            saved_fingerprint: fingerprint,
            current_fingerprint: fingerprint,
            burst: None,
            triangle_count: mesh.indices.len() / 3,
            last_streamed_at: None,
            stream_topology: None,
            resync_requested: false,
        }
    }

    /// Returns whether the asset holds geometry its source file does not.
    pub fn is_unsaved(&self) -> bool {
        self.current_fingerprint != self.saved_fingerprint
    }

    /// Returns whether an update arrived within [`STREAMING_WINDOW`].
    pub fn is_streaming(&self) -> bool {
        self.last_streamed_at
            .is_some_and(|streamed_at| streamed_at.elapsed() < STREAMING_WINDOW)
    }

    /// Opens a burst before the first streamed update of a run is applied,
    /// remembering the geometry it replaces; a burst already open is kept.
    pub fn begin_update(&mut self, mesh_before: &Mesh) {
        if self.burst.is_none() {
            self.burst = Some(EditBurst {
                before: ExtensionPayload::new(protocol::encode_geometry_payload(mesh_before)),
                last_update_at: Instant::now(),
                update_count: 0,
            });
        }
    }

    /// Records a change the link made to the mesh (a streamed update, an undo
    /// or a redo), with the asset's content version after it.
    pub fn record_change(&mut self, mesh: &Mesh, version: Option<u64>, streamed: bool) {
        self.known_version = version;
        self.current_fingerprint = protocol::geometry_fingerprint(mesh);
        self.triangle_count = mesh.indices.len() / 3;
        if streamed {
            let now = Instant::now();
            self.last_streamed_at = Some(now);
            if let Some(burst) = self.burst.as_mut() {
                burst.last_update_at = now;
                burst.update_count += 1;
            }
        }
    }

    /// Takes the current state as the saved one, after a save.
    pub fn mark_saved(&mut self) {
        self.saved_fingerprint = self.current_fingerprint;
    }

    /// Takes a change the link did not make as the new saved state: a
    /// re-import of the source file, or a change from game code. A burst in
    /// progress is dropped, because the state it started from is gone.
    pub fn adopt_outside_change(&mut self, mesh: &Mesh, version: Option<u64>) {
        self.known_version = version;
        self.current_fingerprint = protocol::geometry_fingerprint(mesh);
        self.saved_fingerprint = self.current_fingerprint;
        self.triangle_count = mesh.indices.len() / 3;
        self.burst = None;
        self.stream_topology = None;
    }

    /// Takes the burst when Blender has been quiet for [`EDIT_BURST_IDLE`].
    pub fn take_idle_burst(&mut self) -> Option<EditBurst> {
        let idle = self
            .burst
            .as_ref()
            .is_some_and(|burst| burst.last_update_at.elapsed() >= EDIT_BURST_IDLE);
        if idle {
            self.burst.take()
        } else {
            None
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn linked_triangle() -> (MeshLink, Mesh) {
        let mesh = Mesh::triangle();
        let link = MeshLink::new(
            1,
            Handle::default(),
            "models/t.obj".to_owned(),
            &mesh,
            Some(1),
        );
        (link, mesh)
    }

    #[test]
    fn a_change_is_unsaved_until_it_is_saved() {
        let (mut link, mut mesh) = linked_triangle();
        assert!(!link.is_unsaved());
        mesh.vertices[0].position[1] += 1.0;
        link.record_change(&mesh, Some(2), true);
        assert!(link.is_unsaved());
        link.mark_saved();
        assert!(!link.is_unsaved());
    }

    /// Undoing back to the saved geometry is not an unsaved change.
    #[test]
    fn returning_to_the_saved_geometry_is_saved() {
        let (mut link, mut mesh) = linked_triangle();
        let original = mesh.clone();
        mesh.vertices[0].position[1] += 1.0;
        link.record_change(&mesh, Some(2), true);
        link.record_change(&original, Some(3), false);
        assert!(!link.is_unsaved());
    }

    #[test]
    fn a_burst_keeps_the_geometry_from_before_its_first_update() {
        let (mut link, mut mesh) = linked_triangle();
        let before = protocol::encode_geometry_payload(&mesh);
        link.begin_update(&mesh);
        mesh.vertices[0].position[1] += 1.0;
        link.record_change(&mesh, Some(2), true);
        link.begin_update(&mesh);
        assert_eq!(
            link.burst.as_ref().unwrap().before.bytes(),
            before.as_slice()
        );
        assert_eq!(link.burst.as_ref().unwrap().update_count, 1);
        assert!(
            link.take_idle_burst().is_none(),
            "the burst is still active"
        );
    }

    #[test]
    fn an_outside_change_becomes_the_saved_state_and_drops_the_burst() {
        let (mut link, mut mesh) = linked_triangle();
        link.begin_update(&mesh);
        mesh.vertices[0].position[1] += 1.0;
        link.adopt_outside_change(&mesh, Some(5));
        assert!(!link.is_unsaved());
        assert!(link.burst.is_none());
        assert_eq!(link.known_version, Some(5));
    }
}
