//! The frame the renderer draws: plain data a project's world hands the
//! renderer once per update.
//!
//! # Responsibilities
//!
//! - Define [`RenderFrame`], the per-update renderer input, and the two values
//!   it carries: [`RenderInstance`] for each drawable and [`ResolvedPass`] for
//!   each pass of the chain.
//! - Define the vocabulary a resolved pass is written in: [`PassSettings`],
//!   [`PassKind`], [`PassTarget`], [`CullMode`] and [`MaterialParameter`].
//!
//! # Design
//!
//! Only the types live here. The system that fills them from the world belongs
//! to the renderer module.
//!
//! The frame crosses the artifact boundary, so it carries plain data only:
//! asset keys instead of handles, and a copy of the camera instead of a
//! borrow. The assets behind those keys are read from the world's
//! `AssetManager` when the frame is drawn, so the frame never copies the asset
//! store.
//!
//! **This module is renderer contract.** `PillRenderer::render` takes a
//! [`RenderFrame`], so everything the frame names has to be available to every
//! renderer and to the host, whichever renderer's data crate a project uses.
//! That is why it depends on nothing in the renderer's own data crate: the pass
//! and parameter types it needs are defined here, and the asset types that use
//! them import them from here. The camera is contract for the same reason. The
//! transform is the engine's.

// Standard library
use std::collections::BTreeMap;

// External crates
use pill_engine::common_components::TransformComponent;
use pill_engine::resource;

// =============================================================================
// PassKind
// =============================================================================

/// What a pass draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum PassKind {
    /// Draws the scene's meshes, one instanced batch for each shader, material
    /// and mesh.
    #[default]
    Geometry,
    /// Draws one fullscreen triangle, which is the shape a post-processing pass
    /// takes.
    Fullscreen,
    /// Draws one fullscreen triangle at the far plane, depth-tested against
    /// what the geometry passes left, so it shows only where no mesh was drawn.
    /// Its shader - usually the one its material names - turns each pixel into
    /// a view direction and samples a sky by it.
    Skybox,
}

// =============================================================================
// PassTarget
// =============================================================================

/// Where a pass reads from and writes to.
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum PassTarget {
    /// The swapchain image. No pass can sample it, so a pass that writes here
    /// belongs at the end of the chain: the renderer warns when a chain has
    /// several surface passes, or has its one anywhere but last.
    #[default]
    Surface,
    /// An offscreen colour target, named so that later passes can sample it.
    ///
    /// The renderer owns these targets. A name that no earlier pass declares
    /// makes the build of the pass fail, and the log names the pass and the
    /// target, so the mistake never reaches the window as a flat picture.
    Offscreen(String),
}

// =============================================================================
// CullMode
// =============================================================================

/// Which faces a pass drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CullMode {
    /// Back faces, which is the default for solid geometry.
    #[default]
    Back,
    /// Front faces, for looking at the inside of a shape.
    Front,
    /// No faces. A fullscreen triangle has no outside to drop, and two-sided
    /// geometry has no inside.
    None,
}

// =============================================================================
// PassSettings
// =============================================================================

/// How one pass draws: what it renders, where it writes, and how the draw is
/// set up.
///
/// All three places that write a pass down use this one block: the `RenderPass`
/// asset a game builds, the document its `.render_pass` file holds, and the
/// [`ResolvedPass`] the frame carries. A new setting is therefore one field
/// here plus one line in [`Self::default`], instead of a field with its own
/// default in each of those types.
///
/// The block is serializable and flattened into the pass document, so a
/// `.render_pass` file carries these fields at its top level, beside the shader
/// and parameter maps.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PassSettings {
    /// What the pass draws.
    pub kind: PassKind,
    /// Where the pass reads and writes.
    pub target: PassTarget,
    /// Further targets the pass writes in the same draw, in the order the
    /// shader's `SV_TARGET1`, `SV_TARGET2` and so on name them.
    ///
    /// This is how a geometry pass leaves a normal buffer behind for a later
    /// pass to read: one draw writes several pictures.
    pub extra_targets: Vec<PassTarget>,
    /// Divisor for the size of the targets this pass writes: 2 makes them half
    /// the surface, and 1 (the default) keeps them at full size.
    ///
    /// A target belongs to the chain rather than to one pass, so the first pass
    /// that names it decides its size. Drawing at half size is how the
    /// expensive, blurry steps - ambient occlusion, depth of field, a bloom
    /// level - cost a quarter of the pixels.
    ///
    /// Only a fullscreen pass can be scaled. A pass's attachments must all have
    /// one size, and a geometry or skybox pass pairs its colour target with the
    /// surface's depth buffer, so the renderer refuses a scaled pass of either
    /// kind rather than draw it at the wrong size.
    pub target_scale: u32,
    /// Whether the pass blends its output over what the target already holds,
    /// instead of replacing it.
    pub blend: bool,
    /// Whether the pass writes depth. A translucent pass reads depth without
    /// writing it, so it does not hide what is drawn after it.
    pub depth_write: bool,
    /// Which faces the pass drops.
    pub cull: CullMode,
    /// Ordering key: a lower value runs first. Passes with equal orders keep
    /// the order the pipeline lists them in.
    pub order: u8,
}

impl Default for PassSettings {
    /// The default is a geometry pass that writes the surface at full size,
    /// culls back faces, writes depth, does not blend, and runs first.
    fn default() -> Self {
        Self {
            kind: PassKind::Geometry,
            target: PassTarget::Surface,
            extra_targets: Vec::new(),
            target_scale: 1,
            blend: false,
            depth_write: true,
            cull: CullMode::Back,
            order: 0,
        }
    }
}

// =============================================================================
// MaterialParameter
// =============================================================================

/// One named value a shader reads.
///
/// Parameters are keyed by the slot name the shader declares. The packer walks
/// the shader's own slots rather than this map, so a name the shader never
/// declares is ignored instead of shifting the values after it. Every value
/// fills one 16-byte uniform slot, padding included.
///
/// The type is serializable because a material file stores its parameters as
/// written: `{"Scalar": 1.0}`, `{"Bool": true}` or
/// `{"Color": [1.0, 0.5, 0.0]}`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MaterialParameter {
    /// One `f32`, in the first four bytes of the slot.
    Scalar(f32),
    /// A boolean, packed as a `u32` of 0 or 1.
    Bool(bool),
    /// An RGB colour: three `f32`s, with the rest of the slot as padding.
    Color([f32; 3]),
}

// =============================================================================
// AssetKey
// =============================================================================

/// The key of one asset, as a frame and a renderer name it.
///
/// A key packs the asset's generation in its high 32 bits and its index in the
/// low 32 bits. A slot that is reused for a new asset gets a new generation, so
/// a key from before that reuse never equals a key from the live store. A
/// renderer keys its GPU objects by this value, which is how it notices that an
/// object was built for an asset that is gone.
///
/// The asset layer computes a key from a handle. The contract only carries it,
/// which is why the type lives here beside the frame rather than with the
/// store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetKey(pub u64);

// =============================================================================
// RenderInstance
// =============================================================================

/// One drawable the frame collected: where it stands and what it draws with.
///
/// The frame writer adds an instance only for a mesh renderer whose mesh and
/// material both still resolve. The renderer turns each instance into GPU work,
/// and runs that share a shader, a material and a mesh become one instanced
/// draw.
#[derive(Clone, Debug)]
pub struct RenderInstance {
    /// The entity this instance belongs to, which is what outlining and picking
    /// address.
    pub entity: pill_engine::Entity,
    /// World transform the instance is drawn at.
    pub transform: TransformComponent,
    /// Key of the mesh to draw.
    pub mesh: AssetKey,
    /// Key of the material to draw it with.
    pub material: AssetKey,
    /// The rendering order of the instance's material, copied here so the
    /// renderer can sort draws without reading the material asset again for
    /// each one.
    pub rendering_order: u8,
}

// =============================================================================
// ResolvedPass
// =============================================================================

/// One pass of the chain the renderer runs, with every handle resolved to a
/// key.
///
/// Its maps are `BTreeMap`s, never `HashMap`s. An empty `HashMap` does not
/// allocate: it points at a static inside the binary that created it. This
/// value lives in the world and outlives that binary - a project or module that
/// built it is reloaded, and its retired image is eventually unmapped - so
/// reading such a map afterwards faults. A `BTreeMap` holds no such pointer.
///
/// Equality is the renderer's rebuild test: it rebuilds a chain when a pass is
/// no longer equal to the pass its GPU objects were built from. A field added
/// here is therefore compared without anything else having to learn about it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPass {
    /// Label used in logs, profiling and error messages.
    pub name: String,
    /// Key of the shader the pass draws with.
    ///
    /// `None` leaves a geometry pass drawing each instance through the shader
    /// its own material names. A fullscreen or skybox pass with no shader is
    /// not drawn.
    pub shader: Option<AssetKey>,
    /// What the pass draws, where it writes, and how the draw is set up.
    pub settings: PassSettings,
    /// Uniform parameters, packed exactly as a material packs its own.
    pub parameters: BTreeMap<String, MaterialParameter>,
    /// Offscreen targets the pass samples, by the texture slot they bind to.
    pub inputs: BTreeMap<String, String>,
    /// Textures from loaded assets that the pass samples, by the texture slot
    /// they bind to. Each is an asset key, the same kind of key `shader` is.
    pub textures: BTreeMap<String, AssetKey>,
}

impl ResolvedPass {
    /// A geometry pass that draws every instance over the swapchain.
    ///
    /// A project that sets no pipeline runs this pass, and a project's own
    /// single-pass pipeline has the same shape.
    pub fn builtin() -> Self {
        Self {
            name: "builtin.default".to_owned(),
            shader: None,
            settings: PassSettings::default(),
            parameters: BTreeMap::new(),
            inputs: BTreeMap::new(),
            textures: BTreeMap::new(),
        }
    }
}

// =============================================================================
// ChainSource
// =============================================================================

/// What a frame's resolved pass chain was read from: the two values that, when
/// either moves, mean the chain has to be resolved again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainSource {
    /// The asset manager's revision when the chain was resolved.
    pub revision: u64,
    /// Key of the pipeline asset the chain came from, when one is selected.
    pub pipeline: Option<AssetKey>,
}

// =============================================================================
// RenderFrame
// =============================================================================

/// One frame's renderer input: the instances, the pass chain, the surface
/// cameras and the clock, extracted from the world and handed to the renderer.
///
/// Everything in it is an owned plain value, so a frame outlives the artifact
/// that filled it.
///
/// The shared name is a fixed identity rather than a path: the host and the
/// renderer module find the same resource by this string across the artifact
/// boundary. It is published under the renderer family's name, which the crate
/// that used to own the frame spelled differently, so the old spelling is
/// declared as a former name for the snapshots taken then.
#[resource(
    shared = "pill_rendering::RenderFrame",
    former = "pill_master_renderer::RenderFrame"
)]
#[derive(Clone, Debug)]
pub struct RenderFrame {
    /// Every drawable collected from the world this update, in traversal order.
    pub instances: Vec<RenderInstance>,
    /// The chain to run this frame, resolved from the pipeline the game
    /// selected. An empty chain asks for a cleared frame and nothing else,
    /// which is what a pipeline with every pass disabled means.
    pub passes: Vec<ResolvedPass>,
    /// What the current `passes` were resolved from.
    ///
    /// Resolving clones every pass's strings and maps, so it only happens again
    /// when this value moves. `chain_generation` is the renderer's cheap test
    /// that the chain it built its pipelines from is still this chain.
    pub chain_source: Option<ChainSource>,
    /// Increased by one each time `passes` is resolved again.
    pub chain_generation: u64,
    /// Seconds since the frame loop started.
    ///
    /// It travels in the engine's parameters, so a shader can animate without
    /// the game passing a value through a pass parameter. A pass parameter is
    /// read once, when the chain is built, not once per frame.
    pub seconds: f32,
    /// This frame's own duration: the amount `seconds` grows by.
    pub delta_seconds: f32,
    /// The camera each surface draws through this frame, by surface id.
    ///
    /// A surface that is missing here is not drawn, and that includes the
    /// primary one ([`crate::SurfaceId::PRIMARY`]): its camera is the one
    /// [`crate::SurfaceCameras`] binds to it, or the frame's chosen world
    /// camera when it has no binding. The renderer's frame writer fills this
    /// once per frame.
    pub surface_views: BTreeMap<crate::SurfaceId, crate::SurfaceView>,
    /// What systems debug-drew for this frame: lines, labels and gizmo handles.
    /// It is empty while debug drawing is off.
    pub debug: pill_engine::DebugFrame,
    /// The entities to outline this frame, and the outline colour.
    ///
    /// This is the [`crate::SelectionOutlines`] resource's value, carried whole
    /// instead of copied field by field, so an editor writes one resource and
    /// the renderer reads one value. An empty `entities` list means no outline.
    pub outlines: crate::SelectionOutlines,
    /// Whether the game asked for the frame's shadows to be traced with ray
    /// tracing.
    ///
    /// A renderer that cannot trace rays ignores it and shades the frame the
    /// way it always does. The value comes from [crate::RayTracedShadows].
    pub ray_traced_shadows: bool,
    /// The distance between the scene grid's minor lines, in world units, when
    /// a frontend turned the grid on. `None` draws no grid.
    ///
    /// The grid is drawn on the surfaces in [`Self::editor_views`], the same
    /// view-bound surfaces that carry the editor's other chrome, so the frame
    /// names that set once instead of twice.
    pub grid_cell_size: Option<f32>,
    /// The surfaces this frame marks as editor views, in ascending order.
    ///
    /// A surface bound to a view camera is an editor's own viewport: it draws
    /// the grid, when the grid is on, and carries the editor's chrome - the
    /// transform gizmo and the selection outlines. A surface drawn through a
    /// world camera is a game's window and never gets that chrome. An empty
    /// list means no editor is looking.
    pub editor_views: Vec<crate::SurfaceId>,
    /// The picks asked for since the last frame. The renderer answers them
    /// through [`crate::PillRenderer::take_pick_results`].
    pub pick_requests: Vec<crate::PickRequest>,
    /// The previews asked for since the last frame. The renderer draws at most
    /// one per frame and answers every request through
    /// [`crate::PillRenderer::take_preview_results`].
    pub preview_requests: Vec<crate::PreviewRequest>,
    /// The number of this frame, counting from one. It is packed into the
    /// engine parameters beside the clocks.
    pub sequence: u64,
    /// The UI to draw over the frame: the draw lists that a game's or a tool's
    /// UI pass built this update, per surface, and the textures they sample. An
    /// empty value means no UI.
    pub ui: pill_ui_frame::UiFrame,
}

impl Default for RenderFrame {
    fn default() -> Self {
        Self {
            instances: Vec::new(),
            passes: Vec::new(),
            chain_source: None,
            chain_generation: 0,
            seconds: 0.0,
            delta_seconds: 0.0,
            surface_views: BTreeMap::new(),
            debug: pill_engine::DebugFrame::default(),
            outlines: crate::SelectionOutlines::default(),
            ray_traced_shadows: false,
            grid_cell_size: None,
            editor_views: Vec::new(),
            pick_requests: Vec::new(),
            preview_requests: Vec::new(),
            sequence: 0,
            ui: pill_ui_frame::UiFrame::default(),
        }
    }
}
