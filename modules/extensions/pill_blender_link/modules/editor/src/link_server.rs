//! The editor's end of the Blender link: a local socket server and the table
//! of meshes open in Blender.
//!
//! # Responsibilities
//!
//! - Listen on `127.0.0.1` for the Blender script, and keep the one connection
//!   it opens; a newer connection replaces an older one.
//! - Give each linked mesh asset a link id, which is how both sides name it.
//! - Track, per link, the material Blender shows and what it was last sent,
//!   so a change reaches Blender once.
//! - Queue the frames the editor sends (the greeting, a mesh to open) and cut
//!   the frames Blender sends out of the stream.
//! - Remember the meshes waiting for Blender to connect. Use a Blender that
//!   is already running - it connects through the installed add-on - and start
//!   one only when none is running.
//! - Keep one Blender linked at a time: a second one is answered `busy` and
//!   waits, so two linked Blenders never take the connection from each other.
//! - Install the link add-on into the person's Blender once per editor run,
//!   in a background Blender, so every Blender they open connects by itself.
//!
//! # Design
//!
//! Every socket is non-blocking and polled once per frame by the module's
//! system, so the link starts no thread. A thread would keep running this
//! module's code after a reload unmapped it.
//!
//! The server is a shared resource with a declared layout, so a reload of
//! this module keeps the listening socket, the connection and the link table:
//! Blender stays connected while the module's code is swapped.

// Standard library
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::Duration;

// External crates
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_engine::{resource, Entity, Handle};
use pill_master_renderer_data::assets::Mesh;
use pill_master_renderer_data::MaterialParameter;
use serde_json::json;

// Current crate
use crate::blender_launch;
use crate::material_graph::MaterialGraph;
use crate::material_link::{MaterialImage, MaterialLink};
use crate::mesh_link::MeshLink;
use crate::protocol::{self, LinkFrame, PROTOCOL_VERSION};

// =============================================================================
// Constants
// =============================================================================

/// The port the editor listens on when `PILL_BLENDER_LINK_PORT` is not set.
pub const DEFAULT_LINK_PORT: u16 = 47390;

/// The environment variable that overrides [`DEFAULT_LINK_PORT`].
pub const LINK_PORT_ENVIRONMENT_VARIABLE: &str = "PILL_BLENDER_LINK_PORT";

/// How long after starting Blender the editor waits before it will start
/// another one: Blender takes a while to open, and a second click meanwhile
/// should not open a second Blender.
const LAUNCH_GRACE_PERIOD: Duration = Duration::from_secs(30);

/// How long a running Blender has to connect before the editor says how to
/// link it: a little more than the add-on's two-second retry.
const RUNNING_BLENDER_CONNECT_GRACE: Duration = Duration::from_secs(6);

/// How long a refused connection is kept for its Blender to read the `busy`
/// answer and close.
pub const REFUSED_CONNECTION_LINGER: Duration = Duration::from_secs(3);

/// The size of one socket read.
const READ_CHUNK_BYTES: usize = 64 * 1024;

// =============================================================================
// Types
// =============================================================================

/// What happened to a request to open a mesh in Blender.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenOutcome {
    /// Blender is connected and the mesh was sent.
    Sent,
    /// Blender was started and the mesh waits for it to connect.
    LaunchedBlender(String),
    /// Blender was started a moment ago; the mesh waits for it to connect.
    WaitingForBlender,
    /// A Blender is already running; the mesh waits for it to connect through
    /// the add-on instead of a second Blender being started.
    WaitingForRunningBlender,
    /// Blender is not connected and could not be started.
    LaunchFailed(String),
}

/// The connection the Blender script opened.
struct BlenderConnection {
    /// The socket, non-blocking.
    stream: TcpStream,
    /// Bytes read but not yet cut into frames.
    receive_buffer: Vec<u8>,
    /// Encoded frames the socket has not taken yet.
    send_buffer: Vec<u8>,
    /// Whether the script has sent its greeting, so it can open meshes.
    greeted: bool,
}

/// A create request the editor accepted, waiting to become an asset.
///
/// It holds the geometry as the self-contained payload an undo step keeps, so
/// the resource's layout stays independent of the renderer's mesh types.
#[derive(Clone, Debug)]
pub struct PendingCreate {
    /// The project-relative path the asset will be written to,
    /// `models/<name>.obj`.
    pub asset_name: String,
    /// The geometry to write, as [`protocol::encode_geometry_payload`] builds it.
    pub geometry: Vec<u8>,
}

/// A mesh created from Blender, written and imported, waiting for its entity.
#[derive(Clone, Debug)]
pub struct CreatedAsset {
    /// The project-relative path of the file it was written to.
    pub asset_name: String,
    /// The live asset, ready to link and to spawn.
    pub handle: Handle<Mesh>,
}

/// The editor's end of the Blender link.
///
/// It is a `Keep` resource: the listening socket and Blender's connection
/// cannot be copied at Play, and the link is editor state, so a play session
/// leaves it alone and Stop does not put an older copy back.
#[resource(
    shared = "pill_blender_link_editor::BlenderLinkServer",
    play_state = "keep",
    layout_sources = [
        "Cargo.toml",
        "src/link_server.rs",
        "src/material_graph.rs",
        "src/material_link.rs",
        "src/mesh_link.rs"
    ]
)]
pub struct BlenderLinkServer {
    /// The port the server listens on.
    port: u16,
    /// This editor run's identity. Blender stores it on each linked object, so
    /// an object linked by an earlier editor run is never streamed into a mesh
    /// that happens to reuse its link id.
    session_id: String,
    /// The listening socket, once it bound.
    listener: Option<TcpListener>,
    /// Whether a failure to bind was already logged, so it is logged once.
    bind_failure_logged: bool,
    /// The Blender script's connection, while one is open.
    connection: Option<BlenderConnection>,
    /// Every mesh opened in Blender during this editor run.
    mesh_links: Vec<MeshLink>,
    /// The material each link shows in Blender, and what it was last sent.
    material_links: Vec<MaterialLink>,
    /// Create requests that passed validation and have not become assets yet.
    pending_creates: Vec<PendingCreate>,
    /// Meshes created from Blender that have no entity yet.
    created_assets: Vec<CreatedAsset>,
    /// The id the next linked mesh takes.
    next_link_id: u64,
    /// Link ids whose mesh waits for Blender to connect and greet.
    waiting_link_ids: Vec<u64>,
    /// When the editor last started Blender.
    launched_at: Option<Instant>,
    /// The number of the last "save everything" request the link answered.
    /// `None` until the first frame reads it, so a server built after a
    /// request was made never answers that old request.
    pub handled_save_request: Option<u64>,
    /// The background Blender installing the add-on, while it runs.
    addon_install: Option<std::process::Child>,
    /// Whether the add-on install was started in this editor run.
    addon_install_started: bool,
    /// Since when meshes wait for a Blender that was already running.
    waiting_for_running_since: Option<Instant>,
    /// Whether the person was told how to link that Blender.
    unlinked_reminder_sent: bool,
    /// Connections answered `busy`, kept until the other side has read the
    /// answer and closed, each with when it was refused.
    refused_connections: Vec<(TcpStream, Instant)>,
}

impl BlenderLinkServer {
    /// Creates the server for the port in `PILL_BLENDER_LINK_PORT`, or
    /// [`DEFAULT_LINK_PORT`]. Nothing is bound until [`Self::poll_listener`].
    pub fn from_environment() -> Self {
        let port = std::env::var(LINK_PORT_ENVIRONMENT_VARIABLE)
            .ok()
            .and_then(|port_text| port_text.trim().parse().ok())
            .unwrap_or(DEFAULT_LINK_PORT);
        let session_seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since_epoch| since_epoch.as_nanos())
            .unwrap_or_default()
            ^ u128::from(std::process::id());
        Self {
            port,
            session_id: format!("{session_seed:x}"),
            listener: None,
            bind_failure_logged: false,
            connection: None,
            mesh_links: Vec::new(),
            material_links: Vec::new(),
            pending_creates: Vec::new(),
            created_assets: Vec::new(),
            next_link_id: 1,
            waiting_link_ids: Vec::new(),
            launched_at: None,
            handled_save_request: None,
            addon_install: None,
            addon_install_started: false,
            waiting_for_running_since: None,
            unlinked_reminder_sent: false,
            refused_connections: Vec::new(),
        }
    }

    /// Returns whether a Blender script is connected and has greeted.
    pub fn is_connected(&self) -> bool {
        self.connection
            .as_ref()
            .is_some_and(|connection| connection.greeted)
    }

    /// Returns whether any linked mesh received an update within
    /// [`mesh_link::STREAMING_WINDOW`](crate::mesh_link::STREAMING_WINDOW).
    pub fn is_streaming(&self) -> bool {
        self.mesh_links.iter().any(MeshLink::is_streaming)
    }

    /// Returns the mesh asset a link id names, when the id is this run's.
    pub fn mesh_of(&self, link_id: u64) -> Option<Handle<Mesh>> {
        self.mesh_link(link_id)
            .map(|mesh_link| mesh_link.mesh_handle)
    }

    /// Returns the link a link id names, when the id is this run's.
    pub fn mesh_link(&self, link_id: u64) -> Option<&MeshLink> {
        self.mesh_links
            .iter()
            .find(|mesh_link| mesh_link.link_id == link_id)
    }

    /// Returns the link a link id names, mutably.
    pub fn mesh_link_mut(&mut self, link_id: u64) -> Option<&mut MeshLink> {
        self.mesh_links
            .iter_mut()
            .find(|mesh_link| mesh_link.link_id == link_id)
    }

    /// Returns the link of a mesh asset, when the mesh was opened in Blender.
    pub fn link_of_mesh(&self, mesh_handle: Handle<Mesh>) -> Option<&MeshLink> {
        self.mesh_links
            .iter()
            .find(|mesh_link| mesh_link.mesh_handle == mesh_handle)
    }

    /// Returns every linked mesh.
    pub fn mesh_links(&self) -> &[MeshLink] {
        &self.mesh_links
    }

    /// Returns every linked mesh, mutably.
    pub fn mesh_links_mut(&mut self) -> &mut [MeshLink] {
        &mut self.mesh_links
    }

    /// Returns the create requests waiting to become assets.
    pub fn pending_creates(&self) -> &[PendingCreate] {
        &self.pending_creates
    }

    /// Returns whether a create for `asset_name` is already waiting.
    pub fn has_pending_create(&self, asset_name: &str) -> bool {
        self.pending_creates
            .iter()
            .any(|pending| pending.asset_name == asset_name)
    }

    /// Queues a validated create request for the step that writes it.
    pub fn push_pending_create(&mut self, pending: PendingCreate) {
        self.pending_creates.push(pending);
    }

    /// Takes the create requests waiting to be written.
    pub fn take_pending_creates(&mut self) -> Vec<PendingCreate> {
        std::mem::take(&mut self.pending_creates)
    }

    /// Returns the meshes created from Blender that have no entity yet.
    pub fn created_assets(&self) -> &[CreatedAsset] {
        &self.created_assets
    }

    /// Queues a written and imported mesh for the step that spawns its entity.
    pub fn push_created_asset(&mut self, created: CreatedAsset) {
        self.created_assets.push(created);
    }

    /// Takes the created meshes waiting for their entities.
    pub fn take_created_assets(&mut self) -> Vec<CreatedAsset> {
        std::mem::take(&mut self.created_assets)
    }

    /// Returns every link's material state.
    pub fn material_links(&self) -> &[MaterialLink] {
        &self.material_links
    }

    /// Returns a link's material state, when the link has one.
    pub fn material_link(&self, link_id: u64) -> Option<&MaterialLink> {
        self.material_links
            .iter()
            .find(|material_link| material_link.link_id() == link_id)
    }

    /// Returns a link's material state, mutably.
    pub fn material_link_mut(&mut self, link_id: u64) -> Option<&mut MaterialLink> {
        self.material_links
            .iter_mut()
            .find(|material_link| material_link.link_id() == link_id)
    }

    /// Returns whether a link waits for Blender to connect before its mesh
    /// can open.
    pub fn is_waiting(&self, link_id: u64) -> bool {
        self.waiting_link_ids.contains(&link_id)
    }

    /// Returns whether a frame from Blender belongs to this editor run.
    pub fn is_this_session(&self, frame: &LinkFrame) -> bool {
        frame.header_text("session") == Some(self.session_id.as_str())
    }

    /// Binds the listening socket when it is not bound yet, then accepts a
    /// waiting connection, which replaces the current one.
    pub fn poll_listener(&mut self) {
        // Step 1: bind once; a port in use is reported once and retried on
        // every frame, so freeing the port brings the link up.
        if self.listener.is_none() {
            match TcpListener::bind((Ipv4Addr::LOCALHOST, self.port))
                .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
            {
                Ok(listener) => {
                    pill_core::info!(
                        target: telemetry_target::EDITOR,
                        port = self.port,
                        "[blender_link] listening"
                    );
                    self.listener = Some(listener);
                }
                Err(error) => {
                    if !self.bind_failure_logged {
                        self.bind_failure_logged = true;
                        pill_core::warn!(
                            target: telemetry_target::EDITOR,
                            port = self.port,
                            error = %error,
                            "[blender_link] cannot listen; set PILL_BLENDER_LINK_PORT to a free port"
                        );
                    }
                    return;
                }
            }
        }
        // Step 2: let the refused connections finish closing.
        self.drain_refused_connections();
        // Step 3: take the newest waiting connection and greet it.
        let Some(listener) = self.listener.as_ref() else {
            return;
        };
        match listener.accept() {
            Ok((mut stream, peer_address)) => {
                // The linked Blender keeps the link; another one is told so
                // and waits, instead of the two taking it from each other.
                if self.is_connected() {
                    // The answer is sent, then only the sending side is shut.
                    // Dropping the socket with Blender's greeting still unread
                    // would reset the connection, and the reset discards the
                    // answer before Blender reads it.
                    let busy_frame = protocol::encode_frame(&json!({ "type": "busy" }), &[]);
                    let _ = stream.set_nonblocking(true);
                    let _ = stream.write_all(&busy_frame);
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    self.refused_connections.push((stream, Instant::now()));
                    pill_core::info!(
                        target: telemetry_target::EDITOR,
                        peer = %peer_address,
                        "[blender_link] another blender refused; one is already linked"
                    );
                    return;
                }
                if let Err(error) = stream.set_nonblocking(true) {
                    pill_core::warn!(
                        target: telemetry_target::EDITOR,
                        error = %error,
                        "[blender_link] connection refused"
                    );
                    return;
                }
                // Small frames (the greeting) should not wait for more bytes.
                let _ = stream.set_nodelay(true);
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    peer = %peer_address,
                    replaced = self.connection.is_some(),
                    "[blender_link] blender connected"
                );
                let mut connection = BlenderConnection {
                    stream,
                    receive_buffer: Vec::new(),
                    send_buffer: Vec::new(),
                    greeted: false,
                };
                connection.send_buffer.extend(protocol::encode_frame(
                    &json!({
                        "type": "welcome",
                        "protocol": PROTOCOL_VERSION,
                        "session": self.session_id,
                        // The add-on shows this as its connection status, so the
                        // person reads the editor's own name in Blender.
                        "editor_title": pill_editor_core::brand::PILL_EDITOR_WINDOW_TITLE,
                        // The folders its two pickers start in, and the only
                        // place a create may write: the project's `res`, empty
                        // when the editor has none mounted.
                        "res_directory": pill_engine::asset_store::mounted_directory()
                            .map(|directory| directory.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    }),
                    &[],
                ));
                self.connection = Some(connection);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => pill_core::warn!(
                target: telemetry_target::EDITOR,
                error = %error,
                "[blender_link] accept failed"
            ),
        }
    }

    /// Returns the link id of `mesh_handle`, creating the link the first time
    /// the mesh is opened. A new link takes the mesh's current geometry as
    /// its saved state.
    pub fn link_mesh(
        &mut self,
        mesh_handle: Handle<Mesh>,
        asset_name: &str,
        mesh: &Mesh,
        version: Option<u64>,
    ) -> u64 {
        if let Some(mesh_link) = self.link_of_mesh(mesh_handle) {
            return mesh_link.link_id;
        }
        let link_id = self.next_link_id;
        self.next_link_id += 1;
        self.mesh_links.push(MeshLink::new(
            link_id,
            mesh_handle,
            asset_name.to_owned(),
            mesh,
            version,
        ));
        link_id
    }

    /// Returns a link's material state, creating it, and records the entity
    /// whose material the link shows.
    ///
    /// A click on a different entity replaces the recorded one, so the next
    /// plan sends the whole graph: Blender's node tree belongs to the previous
    /// entity's material.
    pub fn link_material(&mut self, link_id: u64, entity: Entity) -> &mut MaterialLink {
        if let Some(index) = self
            .material_links
            .iter()
            .position(|material_link| material_link.link_id() == link_id)
        {
            let material_link = &mut self.material_links[index];
            if material_link.entity() != entity {
                material_link.change_entity(entity);
            }
            return material_link;
        }
        self.material_links.push(MaterialLink::new(link_id, entity));
        self.material_links
            .last_mut()
            .expect("the material link was just pushed")
    }

    /// Opens a linked mesh in Blender: sends it when Blender is connected,
    /// and otherwise keeps it waiting and starts Blender.
    pub fn request_open(&mut self, link_id: u64, mesh: &Mesh) -> OpenOutcome {
        if self.is_connected() {
            self.send_open_mesh(link_id, mesh);
            return OpenOutcome::Sent;
        }
        if !self.waiting_link_ids.contains(&link_id) {
            self.waiting_link_ids.push(link_id);
        }
        // A connection that has not greeted yet is a Blender already starting.
        let blender_starting = self.connection.is_some()
            || self
                .launched_at
                .is_some_and(|launched_at| launched_at.elapsed() < LAUNCH_GRACE_PERIOD);
        if blender_starting {
            return OpenOutcome::WaitingForBlender;
        }
        // A Blender that is already running is used, never a second one: it
        // connects through the add-on, and the mesh opens when it greets. The
        // processes are counted before the add-on installer starts, so its own
        // background Blender is never mistaken for one the person opened; an
        // installer still running from an earlier click is left out.
        let installer_count = usize::from(self.addon_install.is_some());
        let blender_running = blender_launch::running_blender_count() > installer_count;
        // The add-on is what lets a Blender the person opened connect.
        self.start_addon_install();
        if blender_running {
            self.waiting_for_running_since
                .get_or_insert_with(Instant::now);
            return OpenOutcome::WaitingForRunningBlender;
        }
        match blender_launch::launch_blender(self.port) {
            Ok(executable_path) => {
                self.launched_at = Some(Instant::now());
                OpenOutcome::LaunchedBlender(executable_path.display().to_string())
            }
            Err(error) => {
                self.waiting_link_ids.retain(|waiting| *waiting != link_id);
                OpenOutcome::LaunchFailed(format!(
                    "{} ({error}); set {} to blender.exe",
                    blender_launch::blender_executable_path().display(),
                    blender_launch::BLENDER_PATH_ENVIRONMENT_VARIABLE
                ))
            }
        }
    }

    /// Reads and discards what the refused connections still hold, and drops
    /// each one once its Blender has closed it, or after
    /// [`REFUSED_CONNECTION_LINGER`].
    pub fn drain_refused_connections(&mut self) {
        let mut discard_chunk = [0u8; 4096];
        self.refused_connections.retain_mut(|(stream, refused_at)| {
            if refused_at.elapsed() >= REFUSED_CONNECTION_LINGER {
                return false;
            }
            loop {
                match stream.read(&mut discard_chunk) {
                    Ok(0) => return false,
                    Ok(_) => continue,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => return true,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => return false,
                }
            }
        });
    }

    /// Starts the background add-on install, once per editor run, unless
    /// `PILL_BLENDER_INSTALL_ADDON` turns it off.
    pub fn start_addon_install(&mut self) {
        if self.addon_install_started || !blender_launch::addon_install_allowed() {
            return;
        }
        self.addon_install_started = true;
        match blender_launch::start_addon_install() {
            Ok(installer) => {
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    "[blender_link] installing the blender add-on"
                );
                self.addon_install = Some(installer);
            }
            Err(error) => pill_core::warn!(
                target: telemetry_target::EDITOR,
                error = %error,
                "[blender_link] blender add-on install could not start"
            ),
        }
    }

    /// Returns the add-on install's outcome once its background Blender has
    /// finished: `Ok` when the add-on is installed and enabled, `Err` with the
    /// reason otherwise. `None` while it runs, or when none was started.
    pub fn poll_addon_install(&mut self) -> Option<Result<(), String>> {
        let installer = self.addon_install.as_mut()?;
        let exit_status = match installer.try_wait() {
            Ok(Some(exit_status)) => exit_status,
            Ok(None) => return None,
            Err(error) => {
                self.addon_install = None;
                return Some(Err(error.to_string()));
            }
        };
        self.addon_install = None;
        if exit_status.success() {
            Some(Ok(()))
        } else {
            Some(Err(format!("the installer ended with {exit_status}")))
        }
    }

    /// Returns `true` once, when meshes have waited longer than
    /// [`RUNNING_BLENDER_CONNECT_GRACE`] for a running Blender that has not
    /// connected - it was opened before the add-on was enabled.
    pub fn unlinked_blender_reminder_due(&mut self) -> bool {
        let overdue = self
            .waiting_for_running_since
            .is_some_and(|waiting_since| waiting_since.elapsed() >= RUNNING_BLENDER_CONNECT_GRACE);
        if overdue && !self.unlinked_reminder_sent && !self.is_connected() {
            self.unlinked_reminder_sent = true;
            return true;
        }
        false
    }

    /// Marks the connection as greeted and returns the link ids that waited
    /// for it, so the caller sends their meshes.
    pub fn accept_greeting(&mut self) -> Vec<u64> {
        let Some(connection) = self.connection.as_mut() else {
            return Vec::new();
        };
        connection.greeted = true;
        self.launched_at = None;
        self.waiting_for_running_since = None;
        self.unlinked_reminder_sent = false;
        // A new Blender knows nothing yet, so every link's material graph is
        // sent again on this frame.
        for material_link in &mut self.material_links {
            material_link.forget_sent_graph();
        }
        std::mem::take(&mut self.waiting_link_ids)
    }

    /// Queues a mesh's geometry for Blender to open under `link_id`.
    pub fn send_open_mesh(&mut self, link_id: u64, mesh: &Mesh) {
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        let header = json!({
            "type": "open_mesh",
            "session": self.session_id,
            "link_id": link_id,
            "name": mesh.name,
            "vertex_count": mesh.vertices.len(),
            "index_count": mesh.indices.len(),
        });
        let body = protocol::encode_mesh_geometry(mesh);
        connection
            .send_buffer
            .extend(protocol::encode_frame(&header, &body));
    }

    /// Queues the mesh's geometry to replace Blender's object of `link_id`, so
    /// an undo or redo in the editor reaches Blender too.
    pub fn send_replace_mesh(&mut self, link_id: u64, mesh: &Mesh) {
        if !self.is_connected() {
            return;
        }
        let header = json!({
            "type": "replace_mesh",
            "session": self.session_id,
            "link_id": link_id,
            "vertex_count": mesh.vertices.len(),
            "index_count": mesh.indices.len(),
        });
        self.queue_frame(&header, &protocol::encode_mesh_geometry(mesh));
    }

    /// Asks Blender for a full update of `link_id`: the editor's mesh is not
    /// in the vertex order Blender's positions-only updates are written for.
    pub fn send_resync(&mut self, link_id: u64) {
        let header = json!({
            "type": "resync",
            "session": self.session_id,
            "link_id": link_id,
        });
        self.queue_frame(&header, &[]);
    }

    /// Queues the answer to a `connect_asset` request: an object asked to be
    /// linked to an asset the project already holds.
    ///
    /// `ok` follows `link_id`, so a refusal carries only the reason.
    pub fn send_connect_result(
        &mut self,
        object_name: &str,
        link_id: Option<u64>,
        asset_name: &str,
        detail: &str,
    ) {
        let mut header = json!({
            "type": "connect_result",
            "session": self.session_id,
            "object": object_name,
            "asset_name": asset_name,
            "ok": link_id.is_some(),
            "detail": detail,
        });
        if let Some(link_id) = link_id {
            header["link_id"] = json!(link_id);
        }
        self.queue_frame(&header, &[]);
    }

    /// Queues the answer to a `create_mesh` request.
    ///
    /// A `Some` link id means the asset was created and the Blender object can
    /// bind to it; `None` means the request was refused, and `detail` says why.
    pub fn send_create_result(&mut self, link_id: Option<u64>, asset_name: &str, detail: &str) {
        let mut header = json!({
            "type": "create_result",
            "session": self.session_id,
            "asset_name": asset_name,
            "ok": link_id.is_some(),
            "detail": detail,
        });
        if let Some(link_id) = link_id {
            header["link_id"] = json!(link_id);
        }
        self.queue_frame(&header, &[]);
    }

    /// Queues a link's whole material graph, which Blender builds as a new
    /// node tree.
    ///
    /// The graph travels as the frame's body, so a material with many nodes
    /// does not lengthen the header.
    pub fn send_material(&mut self, link_id: u64, material_name: &str, graph: &MaterialGraph) {
        let header = json!({
            "type": "material",
            "session": self.session_id,
            "link_id": link_id,
            "name": material_name,
            "node_count": graph.nodes.len(),
        });
        let body = serde_json::to_vec(graph).unwrap_or_default();
        self.queue_frame(&header, &body);
    }

    /// Queues a link's parameter values, which Blender applies to the nodes it
    /// already holds.
    pub fn send_material_values(
        &mut self,
        link_id: u64,
        parameters: &BTreeMap<String, MaterialParameter>,
    ) {
        let header = json!({
            "type": "material_values",
            "session": self.session_id,
            "link_id": link_id,
        });
        let body = serde_json::to_vec(parameters).unwrap_or_default();
        self.queue_frame(&header, &body);
    }

    /// Asks Blender to reload the listed textures of a link's material.
    ///
    /// Each entry carries the file and the colour space, so Blender can also
    /// load an image node the person deleted; an entry without a file means
    /// the texture's path does not resolve, and Blender reloads the image it
    /// already holds.
    pub fn send_material_images(&mut self, link_id: u64, images: &[MaterialImage]) {
        let header = json!({
            "type": "material_images",
            "session": self.session_id,
            "link_id": link_id,
            "images": images,
        });
        self.queue_frame(&header, &[]);
    }

    /// Queues one frame on the connection, when there is one.
    pub fn queue_frame(&mut self, header: &serde_json::Value, body: &[u8]) {
        if let Some(connection) = self.connection.as_mut() {
            connection
                .send_buffer
                .extend(protocol::encode_frame(header, body));
        }
    }

    /// Reads what the socket holds and returns the complete frames in it.
    ///
    /// A closed or broken connection, or a stream that is not frames, is
    /// dropped; Blender reconnects on its own.
    pub fn receive_frames(&mut self) -> Vec<LinkFrame> {
        let Some(connection) = self.connection.as_mut() else {
            return Vec::new();
        };
        // Step 1: read until the socket has nothing more this frame.
        let mut read_chunk = [0u8; READ_CHUNK_BYTES];
        let mut disconnect_reason = None;
        loop {
            match connection.stream.read(&mut read_chunk) {
                Ok(0) => {
                    disconnect_reason = Some("closed by blender".to_owned());
                    break;
                }
                Ok(byte_count) => connection
                    .receive_buffer
                    .extend_from_slice(&read_chunk[..byte_count]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => {
                    disconnect_reason = Some(error.to_string());
                    break;
                }
            }
        }
        // Step 2: cut the frames that are complete; keep a partial one.
        let mut frames = Vec::new();
        loop {
            match protocol::take_frame(&mut connection.receive_buffer) {
                Ok(Some(frame)) => frames.push(frame),
                Ok(None) => break,
                Err(error) => {
                    disconnect_reason = Some(error.to_string());
                    break;
                }
            }
        }
        if let Some(reason) = disconnect_reason {
            self.disconnect(&reason);
        }
        frames
    }

    /// Writes as much of the queued frames as the socket takes now.
    pub fn flush(&mut self) {
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        while !connection.send_buffer.is_empty() {
            match connection.stream.write(&connection.send_buffer) {
                Ok(0) => {
                    self.disconnect("the socket stopped taking bytes");
                    return;
                }
                Ok(byte_count) => {
                    connection.send_buffer.drain(..byte_count);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.disconnect(&error.to_string());
                    return;
                }
            }
        }
    }

    /// Drops the connection and says why.
    pub fn disconnect(&mut self, reason: &str) {
        if self.connection.take().is_some() {
            pill_core::info!(
                target: telemetry_target::EDITOR,
                reason,
                "[blender_link] blender disconnected"
            );
        }
    }
}
