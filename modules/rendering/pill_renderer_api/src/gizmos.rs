//! Component gizmos: a component type's own read-only draw function, run by one
//! engine pass for every entity that carries the component.
//!
//! # Responsibilities
//!
//! - Hold what a component declares about its gizmo: the draw function, and
//!   whether it draws for every entity or only for the selected ones
//!   ([`GizmoDeclaration`]).
//! - Keep the gizmos registered in this world, which types an editor has hidden,
//!   and how many entities each one drew last frame ([`GizmoRegistry`]).
//! - Publish an artifact's declarations into the world ([`publish_owner`]),
//!   replacing what that owner published before - including with an empty
//!   list, so an artifact that stops declaring a gizmo loses its entry.
//! - Hold what an editor has selected and what sits under its pointer
//!   ([`GizmoSelection`]), so a gizmo knows whose it is without the engine
//!   naming any editor type.
//! - Give each draw call its entity's context ([`GizmoContext`]) and a buffer to
//!   draw lines, shapes and labels into ([`GizmoDraw`]).
//! - Install the single post-project pass that walks every registered gizmo
//!   ([`install`]).
//!
//! # Design
//!
//! A gizmo is read-only: its wrapper borrows the world, reads the component and
//! draws into a buffer, so no gizmo can change the world it describes. The
//! buffer is handed to [`DebugDraw`] after every gizmo ran, which is why the
//! pass is one system rather than one per component.
//!
//! The pass is installed by a host, never by a game: a shipping build registers
//! the same declarations and pays nothing for them. Its lines are
//! [editor-only](DebugStyle::editor_only) in an editor, so a game's own surface
//! never draws them, and plain debug lines in a development host, whose single
//! surface is a game's.
//!
//! The registry holds owned strings and function pointers. The strings are
//! copies because a module's `&'static str` would outlive nothing once its image
//! is unmapped, and the pointers are replaced by the publishing image on every
//! reload before the old image goes away. Declarations are collected by
//! `inventory`, which scopes them to the artifact that links them, so an
//! artifact publishes exactly its own.

// Standard library
use std::collections::BTreeSet;

// External crates
use pill_engine::{
    Component, Engine, Entity, PlayState, Resource, RunPhase, SystemError, SystemOptions,
    TransformComponent, World,
};

// Current crate
use crate::debug_draw::{
    DebugColor, DebugDepth, DebugDraw, DebugLifetime, DebugStyle, GizmoPointShape, LabelAnchor,
};

// =============================================================================
// Constants
// =============================================================================

/// The registration name of the one gizmo pass.
pub const SYSTEM_NAME: &str = "component_gizmos";

/// The pass's order within the post-project phase.
///
/// It must run before the renderer turns [`DebugDraw`] into a frame, and that
/// system sits at the default order of zero, so the pass takes a negative one.
const PASS_ORDER: i32 = -1;

// =============================================================================
// GizmoWhen
// =============================================================================

/// Which entities a component's gizmo draws for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GizmoWhen {
    /// Every entity that carries the component - the default.
    #[default]
    Always,
    /// Only the entities an editor has selected.
    ForSelected,
}

impl GizmoWhen {
    /// The word a report prints for this choice.
    pub fn label(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::ForSelected => "selected",
        }
    }
}

// =============================================================================
// GizmoFrame / GizmoContext
// =============================================================================

/// What the pass gives every gizmo once per frame: whose gizmos draw and what
/// is under the pointer.
#[derive(Debug, Clone, Default)]
pub struct GizmoFrame {
    /// The entities an editor has selected.
    selected: Vec<Entity>,
    /// The entity under the editor's pointer, when there is one.
    hovered: Option<Entity>,
}

impl GizmoFrame {
    /// Builds one frame's context.
    pub fn new(selected: Vec<Entity>, hovered: Option<Entity>) -> Self {
        Self { selected, hovered }
    }

    /// Whether an editor has this entity selected.
    pub fn is_selected(&self, entity: Entity) -> bool {
        self.selected.contains(&entity)
    }

    /// The entity under the editor's pointer, when there is one.
    pub fn hovered(&self) -> Option<Entity> {
        self.hovered
    }

    /// Builds one entity's context by looking up its transform in `world`.
    pub fn context<'a>(&self, world: &'a World, entity: Entity) -> GizmoContext<'a> {
        GizmoContext {
            world,
            entity,
            selected: self.is_selected(entity),
            hovered: self.hovered == Some(entity),
        }
    }
}

/// What one component's draw call knows about its entity.
///
/// It is the read-only half of a system's access: the entity, its place in an
/// editor's selection, and the world it belongs to, so a gizmo reads any
/// component or resource it needs for its drawing.
pub struct GizmoContext<'a> {
    /// The world the entity lives in, which every read goes through.
    world: &'a World,
    /// The entity the component belongs to.
    pub entity: Entity,
    /// Whether an editor has this entity selected.
    pub selected: bool,
    /// Whether this entity is under the editor's pointer.
    pub hovered: bool,
}

impl<'a> GizmoContext<'a> {
    /// The entity's transform, when it has one.
    pub fn transform(&self) -> Option<&'a TransformComponent> {
        self.world.get_component::<TransformComponent>(self.entity)
    }

    /// The entity's position, or the origin when it has no transform.
    pub fn translation(&self) -> [f32; 3] {
        self.transform()
            .map_or([0.0, 0.0, 0.0], |transform| transform.translation)
    }

    /// The entity's rotation as a `(x, y, z, w)` quaternion, or the identity
    /// when it has no transform.
    pub fn rotation(&self) -> [f32; 4] {
        self.transform()
            .map_or([0.0, 0.0, 0.0, 1.0], |transform| transform.rotation)
    }

    /// The entity's scale, or `1` on every axis when it has no transform.
    pub fn scale(&self) -> [f32; 3] {
        self.transform()
            .map_or([1.0, 1.0, 1.0], |transform| transform.scale)
    }

    /// The entity's transform as a row-major matrix, which turns a point in the
    /// entity's own space into world space.
    pub fn to_world(&self) -> [[f32; 4]; 4] {
        let rotation = rotation_matrix(self.rotation());
        let scale = self.scale();
        let translation = self.translation();
        [
            [
                rotation[0][0] * scale[0],
                rotation[0][1] * scale[1],
                rotation[0][2] * scale[2],
                translation[0],
            ],
            [
                rotation[1][0] * scale[0],
                rotation[1][1] * scale[1],
                rotation[1][2] * scale[2],
                translation[1],
            ],
            [
                rotation[2][0] * scale[0],
                rotation[2][1] * scale[1],
                rotation[2][2] * scale[2],
                translation[2],
            ],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }

    /// Reads another component of the same entity, for a gizmo whose drawing
    /// depends on a value beside its own.
    pub fn get_component<T: Component>(&self) -> Option<&'a T> {
        self.world.get_component::<T>(self.entity)
    }

    /// Reads a resource of the world, for a gizmo whose drawing depends on a
    /// manager or a setting.
    pub fn get_resource<T: Resource>(&self) -> Option<&'a T> {
        self.world.get_resource::<T>()
    }

    /// Reads a component of any entity, for a gizmo that draws a link between
    /// two of them.
    pub fn get_component_of<T: Component>(&self, entity: Entity) -> Option<&'a T> {
        self.world.get_component::<T>(entity)
    }
}

/// Turns a point in an entity's own space into world space through
/// [`GizmoContext::to_world`].
pub fn transform_point(to_world: &[[f32; 4]; 4], point: [f32; 3]) -> [f32; 3] {
    let mut world_point = [0.0_f32; 3];
    for (axis, row) in to_world.iter().take(3).enumerate() {
        world_point[axis] = row[0] * point[0] + row[1] * point[1] + row[2] * point[2] + row[3];
    }
    world_point
}

/// The row-major rotation matrix of a `(x, y, z, w)` quaternion.
fn rotation_matrix(rotation: [f32; 4]) -> [[f32; 3]; 3] {
    let length = (rotation[0] * rotation[0]
        + rotation[1] * rotation[1]
        + rotation[2] * rotation[2]
        + rotation[3] * rotation[3])
        .sqrt();
    let [x, y, z, w] = if length > f32::EPSILON {
        [
            rotation[0] / length,
            rotation[1] / length,
            rotation[2] / length,
            rotation[3] / length,
        ]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Turns `vector` by a `(x, y, z, w)` quaternion.
fn rotate_vector(rotation: [f32; 4], vector: [f32; 3]) -> [f32; 3] {
    let matrix = rotation_matrix(rotation);
    [
        matrix[0][0] * vector[0] + matrix[0][1] * vector[1] + matrix[0][2] * vector[2],
        matrix[1][0] * vector[0] + matrix[1][1] * vector[1] + matrix[1][2] * vector[2],
        matrix[2][0] * vector[0] + matrix[2][1] * vector[1] + matrix[2][2] * vector[2],
    ]
}

// =============================================================================
// GizmoDraw
// =============================================================================

/// The shapes one gizmo draws for one frame: lines, circles, boxes, point
/// marks and labels.
///
/// The segments are world-space geometry; a point mark keeps its size on
/// screen instead, which is what a handle wants. The pass hands the buffer to
/// [`DebugDraw`] once every gizmo ran, so a draw call never needs the world's
/// resource and a gizmo cannot interleave with another one's drawing.
#[derive(Debug, Clone, Default)]
pub struct GizmoDraw {
    /// The segments, in the order they were drawn.
    segments: Vec<([[f32; 3]; 2], DebugColor)>,
    /// The point marks: where, how wide in pixels, which silhouette, what
    /// colour.
    points: Vec<PointMark>,
    /// The labels, in the order they were drawn.
    labels: Vec<([f32; 3], String, DebugColor)>,
}

/// One point mark a gizmo drew: a filled handle shape of a fixed pixel size.
#[derive(Debug, Clone, Copy)]
struct PointMark {
    /// Where it sits, in world space.
    position: [f32; 3],
    /// How wide it draws, in pixels, in every view.
    size_pixels: f32,
    /// The silhouette it draws as.
    shape: GizmoPointShape,
    /// Its colour.
    color: DebugColor,
}

impl GizmoDraw {
    /// Creates an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether nothing was drawn yet.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty() && self.points.is_empty() && self.labels.is_empty()
    }

    /// How many segments the buffer holds.
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Draws a line segment in `color`.
    pub fn line(&mut self, start: [f32; 3], end: [f32; 3], color: DebugColor) {
        self.segments.push(([start, end], color));
    }

    /// Draws a sequence of segments, one between each neighbouring pair.
    pub fn polyline(&mut self, points: &[[f32; 3]], color: DebugColor) {
        for pair in points.windows(2) {
            self.line(pair[0], pair[1], color);
        }
    }

    /// Draws a closed loop through `points`, joining the last one back to the
    /// first.
    pub fn line_loop(&mut self, points: &[[f32; 3]], color: DebugColor) {
        self.polyline(points, color);
        if let (Some(first), Some(last)) = (points.first(), points.last()) {
            if points.len() > 1 {
                self.line(*last, *first, color);
            }
        }
    }

    /// Draws a circle around `center` in the plane whose normal is `normal`.
    pub fn circle(&mut self, center: [f32; 3], normal: [f32; 3], radius: f32, color: DebugColor) {
        let (axis_u, axis_v) = plane_axes(normal);
        let point = |index: usize| {
            let angle = index as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            [
                center[0] + radius * (cos * axis_u[0] + sin * axis_v[0]),
                center[1] + radius * (cos * axis_u[1] + sin * axis_v[1]),
                center[2] + radius * (cos * axis_u[2] + sin * axis_v[2]),
            ]
        };
        for index in 0..CIRCLE_SEGMENTS {
            self.line(point(index), point(index + 1), color);
        }
    }

    /// Draws a sphere around `center` as its three great circles.
    pub fn sphere(&mut self, center: [f32; 3], radius: f32, color: DebugColor) {
        for normal in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            self.circle(center, normal, radius, color);
        }
    }

    /// Draws the twelve edges of a box centred on `center`, turned by
    /// `rotation` and `half_extents` from it on each of its own axes.
    pub fn box_outline(
        &mut self,
        center: [f32; 3],
        rotation: [f32; 4],
        half_extents: [f32; 3],
        color: DebugColor,
    ) {
        let mut corners = [[0.0_f32; 3]; 8];
        for (index, corner) in corners.iter_mut().enumerate() {
            let sign = |axis: usize| {
                if index & (1 << axis) == 0 {
                    -1.0
                } else {
                    1.0
                }
            };
            let offset = rotate_vector(
                rotation,
                [
                    sign(0) * half_extents[0],
                    sign(1) * half_extents[1],
                    sign(2) * half_extents[2],
                ],
            );
            *corner = [
                center[0] + offset[0],
                center[1] + offset[1],
                center[2] + offset[2],
            ];
        }
        for (from, to) in BOX_EDGES {
            self.line(corners[from], corners[to], color);
        }
    }

    /// Draws a mark at `position`: a filled `shape`, `size_pixels` wide on
    /// screen in every view, drawn over the scene.
    ///
    /// The size is pixels rather than world units, so a mark on a far path
    /// reads the same as one under the camera. The mark is not pickable: a
    /// click passes through it to whatever is behind, because the handles a
    /// reader grabs are the ones the editor's tools report themselves.
    pub fn point(
        &mut self,
        position: [f32; 3],
        size_pixels: f32,
        shape: GizmoPointShape,
        color: DebugColor,
    ) {
        self.points.push(PointMark {
            position,
            size_pixels,
            shape,
            color,
        });
    }

    /// Draws a text label at a world position.
    pub fn label(&mut self, position: [f32; 3], text: impl Into<String>, color: DebugColor) {
        self.labels.push((position, text.into(), color));
    }

    /// Hands everything drawn into `debug_draw`, as editor-only (`true`) or as
    /// plain debug drawing (`false`).
    ///
    /// The segments are dimmed where geometry hides them, so a gizmo behind an
    /// object still reads without being mistaken for one in front.
    fn flush_into(&self, debug_draw: &mut DebugDraw, editor_only: bool) {
        for (segment, color) in &self.segments {
            debug_draw.line(
                segment[0],
                segment[1],
                DebugStyle {
                    color: *color,
                    depth: DebugDepth::Dimmed {
                        hidden: Default::default(),
                    },
                    editor_only,
                    lifetime: DebugLifetime::OneFrame,
                    ..DebugStyle::default()
                },
            );
        }
        for (position, text, color) in &self.labels {
            debug_draw.label(
                LabelAnchor::World(*position),
                text,
                DebugStyle {
                    color: *color,
                    depth: DebugDepth::OnTop,
                    editor_only,
                    lifetime: DebugLifetime::OneFrame,
                    ..DebugStyle::default()
                },
            );
        }
        for mark in &self.points {
            debug_draw.gizmo_mark(mark.position, mark.size_pixels, mark.shape, mark.color);
        }
    }
}

/// Segments a circle is drawn with.
const CIRCLE_SEGMENTS: usize = 32;

/// The twelve edges of a box, as index pairs into its eight corners.
const BOX_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (1, 3),
    (3, 2),
    (2, 0),
    (4, 5),
    (5, 7),
    (7, 6),
    (6, 4),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// Two unit vectors spanning the plane whose normal is `normal`.
fn plane_axes(normal: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let length = vector_length(normal);
    let normal = if length > f32::EPSILON {
        scale_vector(normal, 1.0 / length)
    } else {
        [0.0, 1.0, 0.0]
    };
    // The axis the normal is least aligned with gives the steadiest first side.
    let helper = if normal[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let axis_u = cross_product(normal, helper);
    let axis_u = scale_vector(axis_u, 1.0 / vector_length(axis_u));
    let axis_v = cross_product(normal, axis_u);
    (axis_u, axis_v)
}

fn vector_length(vector: [f32; 3]) -> f32 {
    (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt()
}

fn scale_vector(vector: [f32; 3], factor: f32) -> [f32; 3] {
    [vector[0] * factor, vector[1] * factor, vector[2] * factor]
}

fn cross_product(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

// =============================================================================
// GizmoDeclaration / GizmoEntry
// =============================================================================

/// The signature a component's generated gizmo wrapper has.
///
/// The wrapper borrows the world read-only, gets the frame's context and draws
/// into the buffer, and answers how many entities it drew for.
pub type GizmoDrawEntry = fn(&World, &GizmoFrame, &mut GizmoDraw) -> usize;

/// What one component type declares about its gizmo.
///
/// The attribute macro submits one of these into [`inventory`] for each
/// component that declares a gizmo, so an artifact can publish its whole list
/// at once. Every field is a constant, so the declaration can live in a
/// `static`.
#[derive(Clone, Copy, Debug)]
pub struct GizmoDeclaration {
    /// Fully-qualified component type name: the declaring module's path plus
    /// the type's name.
    pub type_name: &'static str,
    /// The package that declared the component, from `env!("CARGO_PKG_NAME")`
    /// at the declaration site.
    ///
    /// Publishing replaces an owner's entries, so this is what keeps a reload
    /// from stacking a second copy of the same gizmo.
    pub owner: &'static str,
    /// Which entities it draws for.
    pub when: GizmoWhen,
    /// The generated wrapper that walks the component's entities.
    pub draw: GizmoDrawEntry,
}

inventory::collect!(GizmoDeclaration);

/// Replaces the overrides `owner` registered, which may be none.
///
/// An editor crate calls this from its `register` on every generation, so a
/// reload replaces its draw pointers and a crate that stops overriding gives the
/// declared gizmos back.
pub fn override_gizmos(world: &mut World, owner: &str, declarations: &[GizmoOverrideDeclaration]) {
    if !world.has_resource::<GizmoRegistry>() {
        world.insert_resource(GizmoRegistry::default());
    }
    if let Some(registry) = world.get_resource_mut::<GizmoRegistry>() {
        registry.publish_overrides(owner, declarations);
    }
}

/// Replaces what `owner` publishes with the declarations this artifact carries
/// for it, which may be none.
///
/// Called by each artifact's registration path, once per owner it declares: a
/// reload therefore replaces the owner's function pointers in one step, and an
/// owner that stopped declaring a gizmo loses its entry instead of keeping a
/// pointer into the retired image.
pub fn publish_owner(world: &mut World, owner: &str) {
    let declarations: Vec<GizmoDeclaration> = inventory::iter::<GizmoDeclaration>
        .into_iter()
        .filter(|declaration| declaration.owner == owner)
        .copied()
        .collect();
    if !world.has_resource::<GizmoRegistry>() {
        world.insert_resource(GizmoRegistry::default());
    }
    if let Some(registry) = world.get_resource_mut::<GizmoRegistry>() {
        registry.publish_owner(owner, &declarations);
    }
}

/// One registered gizmo, owned by the world rather than by an image.
///
/// Every name here is an owned copy. A declaration's strings live in the
/// static of the image that declared them, and the reload unmaps a retired
/// image once two younger generations exist, so keeping the borrowed text
/// would leave the editor reading freed memory through it.
#[derive(Clone, Debug)]
pub struct GizmoEntry {
    /// Fully-qualified component type name, copied out of the declaring image.
    pub type_name: String,
    /// The declaring package, copied out of the declaring image.
    pub owner: String,
    /// Which entities it draws for.
    pub when: GizmoWhen,
    /// The generated wrapper.
    pub draw: GizmoDrawEntry,
    /// How many entities it drew for in the last frame.
    drew_last_frame: usize,
}

impl GizmoEntry {
    /// How many entities this gizmo drew for in the last frame; zero while its
    /// type is hidden, nothing is selected, or the pass never ran.
    pub fn drew_last_frame(&self) -> usize {
        self.drew_last_frame
    }
}

// =============================================================================
// GizmoOverrideMode / GizmoOverrideEntry
// =============================================================================

/// How an override changes the drawing of the type's declared gizmo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoOverrideMode {
    /// Draws instead of the declared gizmo.
    Replace,
    /// Draws on top of the declared gizmo.
    Add,
}

/// What an editor crate declares about the gizmo of one component type.
///
/// The attribute macro emits one of these per annotated draw function, with the
/// walking wrapper the registry calls.
#[derive(Clone, Copy, Debug)]
pub struct GizmoOverrideDeclaration {
    /// The component type whose gizmo this overrides, as its stored name.
    pub type_name: &'static str,
    /// The editor crate that declared it, from `env!("CARGO_PKG_NAME")`.
    pub owner: &'static str,
    /// Whether it replaces or adds to the declared gizmo.
    pub mode: GizmoOverrideMode,
    /// Which entities it draws for.
    pub when: GizmoWhen,
    /// Which override wins when two name one type: the highest number, then
    /// the last owner by name.
    pub order: i32,
    /// The generated wrapper that walks the type's entities.
    pub draw: GizmoDrawEntry,
}

/// One registered override, owned by the world rather than by an image.
///
/// The names are owned copies for the same reason [`GizmoEntry`]'s are.
#[derive(Clone, Debug)]
pub struct GizmoOverrideEntry {
    /// The component type whose gizmo this overrides.
    pub type_name: String,
    /// The editor crate that declared it.
    pub owner: String,
    /// Whether it replaces or adds to the declared gizmo.
    pub mode: GizmoOverrideMode,
    /// Which entities it draws for.
    pub when: GizmoWhen,
    /// Which override wins when two name one type.
    pub order: i32,
    /// The generated wrapper.
    pub draw: GizmoDrawEntry,
    /// How many entities it drew for in the last frame.
    drew_last_frame: usize,
}

impl GizmoOverrideEntry {
    /// How many entities this override drew for in the last frame.
    pub fn drew_last_frame(&self) -> usize {
        self.drew_last_frame
    }
}

/// What draws for one type, once the person's switches and the overrides are
/// resolved.
///
/// The pass draws what this says and `dump gizmos` reports it, so the two
/// cannot disagree about which gizmo is the one on screen.
#[derive(Debug, Clone, Copy)]
pub enum GizmoDrawing<'a> {
    /// The type's own declaration.
    Declared(&'a GizmoEntry),
    /// An override that replaces the declaration.
    Override(&'a GizmoOverrideEntry),
    /// The declaration, with an override drawn on top of it.
    DeclaredThenOverride(&'a GizmoEntry, &'a GizmoOverrideEntry),
}

impl<'a> GizmoDrawing<'a> {
    /// The override that draws, when there is one.
    pub fn overridden(self) -> Option<&'a GizmoOverrideEntry> {
        match self {
            Self::Declared(_) => None,
            Self::Override(entry) => Some(entry),
            Self::DeclaredThenOverride(_, entry) => Some(entry),
        }
    }

    /// The type's own declaration, when it still draws.
    pub fn declared(self) -> Option<&'a GizmoEntry> {
        match self {
            Self::Declared(entry) | Self::DeclaredThenOverride(entry, _) => Some(entry),
            Self::Override(_) => None,
        }
    }
}

// =============================================================================
// GizmoRegistry
// =============================================================================

/// Every gizmo registered in this world, the types an editor has hidden, and
/// what each gizmo drew last frame.
///
/// It is kept across a reload (`PlayState::Keep`) and refilled by the image that
/// is loading, so a reload replaces the declared function pointers instead of
/// reading through one that was unloaded.
#[derive(Debug, Clone)]
pub struct GizmoRegistry {
    /// Every registered gizmo, sorted by type name.
    entries: Vec<GizmoEntry>,
    /// The overrides an editor crate registered, sorted by type name and then
    /// by their order number.
    overrides: Vec<GizmoOverrideEntry>,
    /// The component type names an editor has hidden.
    hidden_types: BTreeSet<String>,
    /// Whether the pass draws at all.
    enabled: bool,
}

impl Default for GizmoRegistry {
    /// A registry that draws: the switch is what an editor turns off, so its
    /// absence means on.
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            overrides: Vec::new(),
            hidden_types: BTreeSet::new(),
            enabled: true,
        }
    }
}

impl Resource for GizmoRegistry {
    /// Infrastructure, not game state: Play leaves it alone, so a play session
    /// cannot lose the registered pointers.
    fn play_state() -> PlayState {
        PlayState::Keep
    }

    /// Pinned, because this crate is compiled into every artifact: an unnamed
    /// resource would be identified by a per-DLL type id and the editor would
    /// read a registry the gizmo pass never filled.
    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::GizmoRegistry")
    }
}

impl GizmoRegistry {
    /// Whether the pass draws.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Turns every gizmo on or off, which is the toolbox's switch.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    /// Registers `declarations`, replacing every entry the same packages
    /// published before.
    ///
    /// The replacement is per owner because one artifact publishes the
    /// declarations of every crate it links: a reload must drop the old image's
    /// pointers and take the new image's in one step.
    pub fn publish(&mut self, declarations: &[GizmoDeclaration]) {
        let owners: BTreeSet<&str> = declarations
            .iter()
            .map(|declaration| declaration.owner)
            .collect();
        for owner in owners {
            self.publish_owner(owner, declarations);
        }
    }

    /// Replaces what `owner` published with `declarations`, which may be empty.
    ///
    /// An empty list is how an artifact says "this package declares no gizmo
    /// any more", so its old entries - whose draw pointers live in the image
    /// being retired - are dropped rather than left to draw from dead code.
    pub fn publish_owner(&mut self, owner: &str, declarations: &[GizmoDeclaration]) {
        // Step 1: drop every entry this owner published before.
        self.entries.retain(|entry| entry.owner != owner);
        // Step 2: add the ones that are not there yet, keeping what a frame
        // already counted. A declaration and an entry of the same type name
        // can only belong to one owner, which step 1 has just cleared.
        for declaration in declarations {
            if declaration.owner != owner {
                continue;
            }
            if let Some(existing) = self
                .entries
                .iter_mut()
                .find(|entry| entry.type_name == declaration.type_name)
            {
                existing.owner = declaration.owner.to_owned();
                existing.draw = declaration.draw;
                existing.when = declaration.when;
                continue;
            }
            self.entries.push(GizmoEntry {
                type_name: declaration.type_name.to_owned(),
                owner: declaration.owner.to_owned(),
                when: declaration.when,
                draw: declaration.draw,
                drew_last_frame: 0,
            });
        }
        self.entries
            .sort_by(|left, right| left.type_name.cmp(&right.type_name));
    }

    /// Replaces what `owner` registered as overrides, which may be empty.
    ///
    /// Like [`Self::publish_owner`], the replacement is per owner: an editor
    /// crate that stops overriding gives the declared gizmo back, and a reload
    /// never leaves a pointer into the retired image.
    pub fn publish_overrides(&mut self, owner: &str, declarations: &[GizmoOverrideDeclaration]) {
        self.overrides.retain(|entry| entry.owner != owner);
        for declaration in declarations {
            if declaration.owner != owner {
                continue;
            }
            self.overrides.push(GizmoOverrideEntry {
                type_name: declaration.type_name.to_owned(),
                owner: declaration.owner.to_owned(),
                mode: declaration.mode,
                when: declaration.when,
                order: declaration.order,
                draw: declaration.draw,
                drew_last_frame: 0,
            });
        }
        // Sorted so the winning override is the last one of its type: the
        // highest order number, then the last owner by name.
        self.overrides.sort_by(|left, right| {
            left.type_name
                .cmp(&right.type_name)
                .then_with(|| left.order.cmp(&right.order))
                .then_with(|| left.owner.cmp(&right.owner))
        });
    }

    /// The override that wins for `type_name`, when an editor crate registered
    /// one.
    pub fn override_for(&self, type_name: &str) -> Option<&GizmoOverrideEntry> {
        self.overrides
            .iter()
            .rfind(|entry| entry.type_name == type_name)
    }

    /// Every override an editor crate registered, in the order they resolve.
    pub fn overrides(&self) -> &[GizmoOverrideEntry] {
        &self.overrides
    }

    /// What draws for one type: its declared gizmo, the override that wins, or
    /// both, and `None` when the type is hidden or declares nothing.
    ///
    /// The person's switches win over everything: a hidden type draws nothing
    /// at all, override included.
    pub fn drawing(&self, type_name: &str) -> Option<GizmoDrawing<'_>> {
        if !self.is_type_visible(type_name) {
            return None;
        }
        let declared = self
            .entries
            .iter()
            .find(|entry| entry.type_name == type_name);
        let overridden = self.override_for(type_name);
        match (declared, overridden) {
            (Some(entry), None) => Some(GizmoDrawing::Declared(entry)),
            (Some(entry), Some(over)) if over.mode == GizmoOverrideMode::Add => {
                Some(GizmoDrawing::DeclaredThenOverride(entry, over))
            }
            (_, Some(over)) => Some(GizmoDrawing::Override(over)),
            (None, None) => None,
        }
    }

    /// Records what each declared gizmo and each override drew in the frame
    /// that just ran.
    fn set_draw_counts(&mut self, declared: &[usize], overridden: &[usize]) {
        for (entry, count) in self.entries.iter_mut().zip(declared) {
            entry.drew_last_frame = *count;
        }
        for (entry, count) in self.overrides.iter_mut().zip(overridden) {
            entry.drew_last_frame = *count;
        }
    }

    /// Every registered gizmo, sorted by type name.
    pub fn entries(&self) -> &[GizmoEntry] {
        &self.entries
    }

    /// Whether any gizmo is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many gizmos are registered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Shows or hides a component type's gizmo.
    pub fn set_type_visible(&mut self, type_name: &str, visible: bool) {
        if visible {
            self.hidden_types.remove(type_name);
        } else {
            self.hidden_types.insert(type_name.to_owned());
        }
    }

    /// Whether a component type's gizmo draws, which is false once an editor
    /// hid it.
    pub fn is_type_visible(&self, type_name: &str) -> bool {
        !self.hidden_types.contains(type_name)
    }

    /// The component type names an editor has hidden.
    pub fn hidden_types(&self) -> impl Iterator<Item = &str> {
        self.hidden_types.iter().map(String::as_str)
    }
}

// =============================================================================
// GizmoSelection
// =============================================================================

/// The entities an editor has selected and the one under its pointer.
///
/// An editor writes it; the engine never does. A game has none, so every gizmo
/// sees an empty selection.
#[derive(Debug, Clone, Default)]
pub struct GizmoSelection {
    /// The selected entities, in the order the editor selected them.
    pub selected: Vec<Entity>,
    /// The entity under the editor's pointer, when there is one.
    pub hovered: Option<Entity>,
}

impl Resource for GizmoSelection {
    /// Editor state, not game state: a play session neither copies nor restores
    /// it.
    fn play_state() -> PlayState {
        PlayState::Keep
    }

    /// Pinned for the same reason as [`GizmoRegistry`]'s: the editor writes it
    /// and the gizmo pass reads it across the artifact boundary.
    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::GizmoSelection")
    }
}

impl GizmoSelection {
    /// Replaces what is selected and hovered, answering whether anything
    /// changed - which is what an editor publishes on.
    pub fn replace(&mut self, selected: Vec<Entity>, hovered: Option<Entity>) -> bool {
        if self.selected == selected && self.hovered == hovered {
            return false;
        }
        self.selected = selected;
        self.hovered = hovered;
        true
    }
}

// =============================================================================
// GizmoVisibility
// =============================================================================

/// What the pass's drawing is, which decides the surfaces that show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoVisibility {
    /// Editor-only: drawn in the editor's own views and never in a game's.
    EditorOnly,
    /// Plain debug drawing: drawn on every surface, which is what a
    /// development host wants - its only surface is a game's.
    Everywhere,
}

// =============================================================================
// The pass
// =============================================================================

/// Installs the one pass that draws every registered gizmo.
///
/// Called by a host, never by a game: an editor passes
/// [`GizmoVisibility::EditorOnly`], a development host
/// [`GizmoVisibility::Everywhere`], and a shipping or web build installs
/// nothing. The pass runs in the post-project phase, also while editing, and
/// belongs to the engine so no reload retires it.
///
/// Installing twice changes nothing: the second call finds the system already
/// registered.
pub fn install(engine: &mut Engine, visibility: GizmoVisibility) {
    if engine.is_registered(SYSTEM_NAME) {
        return;
    }
    // The registry is inserted here, when it is missing, so the editor can read
    // it even on a world whose modules declare no gizmo at all.
    let world = engine.world_mut();
    if !world.has_resource::<GizmoRegistry>() {
        world.insert_resource(GizmoRegistry::default());
    }
    engine.register_engine_owned_system(
        SYSTEM_NAME,
        SystemOptions::new(RunPhase::PostProject, PASS_ORDER).with_run_in_edit_mode(true),
        move |world: &mut World, _queue| run_pass(world, visibility),
    );
}

/// One type's drawings for this frame: the entry whose declared gizmo draws, and
/// the override index that draws for it.
struct DrawableGizmo {
    /// Index into the registry's entries, when the declaration itself draws.
    entry: Option<usize>,
    /// Index into the registry's overrides, when an override draws.
    overridden: Option<usize>,
}

/// The index of `over` in `overrides`, which is what the draw counts are keyed
/// by.
fn override_index(overrides: &[GizmoOverrideEntry], over: &GizmoOverrideEntry) -> Option<usize> {
    overrides
        .iter()
        .position(|entry| entry.owner == over.owner && entry.type_name == over.type_name)
}

/// One pass: every visible gizmo draws into one buffer, which then reaches the
/// world's [`DebugDraw`].
fn run_pass(world: &mut World, visibility: GizmoVisibility) -> Result<(), SystemError> {
    // Step 1: read what this frame draws for, and which entries may draw at
    // all. The registry borrow ends here, before a gizmo reads the world.
    let Some(gizmo_registry) = world.get_resource::<GizmoRegistry>() else {
        return Ok(());
    };
    if gizmo_registry.is_empty() && gizmo_registry.overrides().is_empty() {
        return Ok(());
    }
    let entry_count = gizmo_registry.len();
    let override_count = gizmo_registry.overrides().len();
    // A switched-off pass draws nothing and says so: the counts are what
    // `dump gizmos` reads, so they must not stay at the last frame's numbers.
    if !gizmo_registry.is_enabled() {
        if let Some(gizmo_registry) = world.get_resource_mut::<GizmoRegistry>() {
            gizmo_registry.set_draw_counts(&vec![0; entry_count], &vec![0; override_count]);
        }
        return Ok(());
    }
    let frame = world
        .get_resource::<GizmoSelection>()
        .map(|selection| GizmoFrame::new(selection.selected.clone(), selection.hovered))
        .unwrap_or_default();
    // What each type draws, resolved once: the declared gizmo, the override
    // that wins for it, or both, with the person's switches already applied.
    let mut drawable: Vec<DrawableGizmo> = Vec::new();
    for (index, entry) in gizmo_registry.entries().iter().enumerate() {
        match gizmo_registry.drawing(&entry.type_name) {
            Some(GizmoDrawing::Declared(_)) => drawable.push(DrawableGizmo {
                entry: Some(index),
                overridden: None,
            }),
            Some(GizmoDrawing::DeclaredThenOverride(_, over)) => drawable.push(DrawableGizmo {
                entry: Some(index),
                overridden: override_index(gizmo_registry.overrides(), over),
            }),
            Some(GizmoDrawing::Override(over)) => drawable.push(DrawableGizmo {
                entry: None,
                overridden: override_index(gizmo_registry.overrides(), over),
            }),
            None => {}
        }
    }
    // An override for a type nothing declares still draws, which is how an
    // editor crate visualises a component the project no longer declares.
    for over in gizmo_registry.overrides().iter() {
        let declared = gizmo_registry
            .entries()
            .iter()
            .any(|entry| entry.type_name == over.type_name);
        let wins = gizmo_registry
            .override_for(&over.type_name)
            .is_some_and(|winner| winner.owner == over.owner && winner.order == over.order);
        if !declared && wins && gizmo_registry.is_type_visible(&over.type_name) {
            drawable.push(DrawableGizmo {
                entry: None,
                overridden: override_index(gizmo_registry.overrides(), over),
            });
        }
    }
    if drawable.is_empty() {
        if let Some(gizmo_registry) = world.get_resource_mut::<GizmoRegistry>() {
            gizmo_registry.set_draw_counts(&vec![0; entry_count], &vec![0; override_count]);
        }
        return Ok(());
    }

    // Step 2: the declared gizmos draw into the one buffer, then the overrides
    // that won for their type, so an added drawing lands on top.
    let mut buffer = GizmoDraw::new();
    let mut counts = vec![0_usize; entry_count];
    let mut override_counts = vec![0_usize; override_count];
    for gizmo in &drawable {
        if let Some(index) = gizmo.entry {
            let entry = &gizmo_registry.entries()[index];
            counts[index] = (entry.draw)(world, &frame, &mut buffer);
        }
    }
    for gizmo in &drawable {
        if let Some(index) = gizmo.overridden {
            let over = &gizmo_registry.overrides()[index];
            override_counts[index] = (over.draw)(world, &frame, &mut buffer);
        }
    }

    // Step 3: the buffer becomes the world's debug drawing. A world with none
    // - a game that never asked for debug drawing - still records the counts,
    // so `dump gizmos` can tell "nothing drawn" from "not registered".
    if !buffer.is_empty() {
        if let Some(debug_draw) = world.get_resource_mut::<DebugDraw>() {
            buffer.flush_into(debug_draw, visibility == GizmoVisibility::EditorOnly);
        }
    }
    if let Some(gizmo_registry) = world.get_resource_mut::<GizmoRegistry>() {
        gizmo_registry.set_draw_counts(&counts, &override_counts);
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::World;

    /// A component whose gizmo draws one line per entity, for every entity.
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    struct Marker {
        size: f32,
    }

    impl Component for Marker {}

    /// The wrapper the derive macro generates for `Marker`.
    fn draw_marker(world: &World, frame: &GizmoFrame, draw: &mut GizmoDraw) -> usize {
        let mut drawn = 0;
        for archetype in world.archetypes_iter() {
            if !archetype.has_component::<Marker>() {
                continue;
            }
            for &entity in &archetype.entities {
                let Some(marker) = world.get_component::<Marker>(entity) else {
                    continue;
                };
                let context = frame.context(world, entity);
                draw.line(
                    context.translation(),
                    [marker.size, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 1.0],
                );
                drawn += 1;
            }
        }
        drawn
    }

    /// A declaration naming the wrapper above.
    fn marker_declaration(when: GizmoWhen) -> GizmoDeclaration {
        GizmoDeclaration {
            type_name: "tests::Marker",
            owner: "tests",
            when,
            draw: draw_marker,
        }
    }

    /// An override of the marker's gizmo, by one editor crate.
    fn marker_override(
        owner: &'static str,
        mode: GizmoOverrideMode,
        order: i32,
    ) -> GizmoOverrideDeclaration {
        GizmoOverrideDeclaration {
            type_name: "tests::Marker",
            owner,
            mode,
            when: GizmoWhen::Always,
            order,
            draw: draw_marker,
        }
    }

    /// A registry holding one declared gizmo and nothing else.
    fn registry_with_marker() -> GizmoRegistry {
        let mut registry = GizmoRegistry::default();
        registry.publish(&[marker_declaration(GizmoWhen::Always)]);
        registry
    }

    /// A world with three marked entities and one unmarked one.
    fn world_with_markers() -> (World, Vec<Entity>) {
        let mut world = World::new();
        world.register_component::<Marker>();
        world.register_component::<TransformComponent>();
        let mut entities = Vec::new();
        for index in 0..3 {
            let entity = world
                .create_entity()
                .with(Marker {
                    size: index as f32 + 1.0,
                })
                .with(TransformComponent {
                    translation: [index as f32, 0.0, 0.0],
                    ..TransformComponent::default()
                })
                .build()
                .expect("the entity builds");
            entities.push(entity);
        }
        world
            .create_entity()
            .build()
            .expect("the unmarked entity builds");
        (world, entities)
    }

    /// Publishing twice replaces an owner's entries instead of stacking them.
    #[test]
    fn publishing_replaces_an_owners_entries() {
        let mut registry = GizmoRegistry::default();
        registry.publish(&[marker_declaration(GizmoWhen::Always)]);
        registry.publish(&[marker_declaration(GizmoWhen::ForSelected)]);
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.entries()[0].when,
            GizmoWhen::ForSelected,
            "the second publish must replace the first"
        );
    }

    /// An owner that declares nothing any more loses the entries it published
    /// before, so no old draw pointer survives a reload that removed it.
    #[test]
    fn an_owner_that_publishes_nothing_loses_its_entries() {
        let mut registry = GizmoRegistry::default();
        let mut other = marker_declaration(GizmoWhen::Always);
        other.type_name = "other::Marker";
        other.owner = "other";
        registry.publish(&[marker_declaration(GizmoWhen::Always), other]);
        assert_eq!(registry.len(), 2);

        registry.publish_owner("tests", &[]);
        assert_eq!(registry.len(), 1, "only the other owner's entry is left");
        assert_eq!(registry.entries()[0].owner, "other");
    }

    /// A second owner's declaration is added beside the first one's.
    #[test]
    fn publishing_keeps_another_owners_entries() {
        let mut registry = GizmoRegistry::default();
        let mut other = marker_declaration(GizmoWhen::Always);
        other.type_name = "other::Marker";
        other.owner = "other";
        registry.publish(&[marker_declaration(GizmoWhen::Always)]);
        registry.publish(&[other]);
        assert_eq!(registry.len(), 2);
        let names: Vec<&str> = registry
            .entries()
            .iter()
            .map(|entry| entry.type_name.as_str())
            .collect();
        assert_eq!(names, ["other::Marker", "tests::Marker"]);
    }

    /// A kept entry takes the arriving declaration's text, not the text of the
    /// image that published it first.
    ///
    /// The registry survives a reload, and the names a declaration carries are
    /// pointers into the declaring image, which the reload unmaps. An entry
    /// that kept the first generation's text would be read out of freed memory
    /// by the next editor snapshot.
    #[test]
    fn publishing_replaces_the_text_a_previous_generation_published() {
        let mut registry = GizmoRegistry::default();
        registry.publish(&[marker_declaration(GizmoWhen::Always)]);
        // A later generation's declaration, with different text for the same
        // type: whatever it publishes is what the entry must answer with.
        let mut arriving = marker_declaration(GizmoWhen::Always);
        arriving.type_name = "tests::Marker";
        arriving.owner = "arriving-package";
        registry.publish(&[arriving]);
        assert_eq!(registry.len(), 1, "the type is still one entry");
        assert_eq!(registry.entries()[0].owner, "arriving-package");
    }

    /// An override replaces the declared gizmo while its owner is registered,
    /// and the declared gizmo comes back when the owner stops overriding.
    #[test]
    fn an_override_replaces_and_gives_the_declaration_back() {
        let mut registry = registry_with_marker();
        registry.publish_overrides(
            "spline-editor",
            &[marker_override(
                "spline-editor",
                GizmoOverrideMode::Replace,
                0,
            )],
        );
        let drawing = registry.drawing("tests::Marker").expect("it draws");
        assert!(drawing.declared().is_none(), "the declaration is replaced");
        assert_eq!(
            drawing.overridden().map(|entry| entry.owner.as_str()),
            Some("spline-editor")
        );

        registry.publish_overrides("spline-editor", &[]);
        let drawing = registry.drawing("tests::Marker").expect("it draws");
        assert!(drawing.declared().is_some(), "the declaration is back");
        assert!(drawing.overridden().is_none());
    }

    /// An adding override draws beside the declaration instead of instead of it.
    #[test]
    fn an_adding_override_draws_beside_the_declaration() {
        let mut registry = registry_with_marker();
        registry.publish_overrides(
            "an-editor",
            &[marker_override("an-editor", GizmoOverrideMode::Add, 0)],
        );
        let drawing = registry.drawing("tests::Marker").expect("it draws");
        assert!(drawing.declared().is_some());
        assert!(drawing.overridden().is_some());
    }

    /// Two overrides on one type resolve by order, then by owner, and the winner
    /// is the one thing the pass and the dump both see.
    #[test]
    fn two_overrides_on_one_type_resolve_to_one_winner() {
        let mut registry = registry_with_marker();
        registry.publish_overrides(
            "first",
            &[marker_override("first", GizmoOverrideMode::Add, 400)],
        );
        registry.publish_overrides(
            "second",
            &[marker_override("second", GizmoOverrideMode::Add, 500)],
        );
        assert_eq!(
            registry
                .override_for("tests::Marker")
                .map(|entry| entry.owner.as_str()),
            Some("second"),
            "the higher order number wins"
        );
        registry.publish_overrides(
            "second",
            &[marker_override("second", GizmoOverrideMode::Add, 300)],
        );
        assert_eq!(
            registry
                .override_for("tests::Marker")
                .map(|entry| entry.owner.as_str()),
            Some("first"),
            "a lower order number loses"
        );
    }

    /// The person's switch hides the override with the declaration.
    #[test]
    fn the_persons_switch_wins_over_an_override() {
        let mut registry = registry_with_marker();
        registry.publish_overrides(
            "an-editor",
            &[marker_override("an-editor", GizmoOverrideMode::Replace, 0)],
        );
        registry.set_type_visible("tests::Marker", false);
        assert!(
            registry.drawing("tests::Marker").is_none(),
            "a hidden type draws nothing, override included"
        );
    }

    /// The pass draws the override and counts what it drew, so `dump gizmos`
    /// reports the drawing that happened.
    #[test]
    fn the_pass_draws_the_override_and_counts_it() {
        let (mut world, _entities) = world_with_markers();
        world.insert_resource(registry_with_marker());
        world.insert_resource(GizmoSelection::default());
        world.insert_resource(DebugDraw::new(true));
        if let Some(registry) = world.get_resource_mut::<GizmoRegistry>() {
            registry.publish_overrides(
                "an-editor",
                &[marker_override("an-editor", GizmoOverrideMode::Replace, 0)],
            );
        }

        run_pass(&mut world, GizmoVisibility::EditorOnly).expect("the pass runs");
        let registry = world
            .get_resource::<GizmoRegistry>()
            .expect("the registry is there");
        assert_eq!(
            registry.entries()[0].drew_last_frame(),
            0,
            "the replaced declaration drew nothing"
        );
        assert_eq!(registry.overrides()[0].drew_last_frame(), 3);
    }

    /// Hiding a type keeps the entry registered but stops it drawing, and
    /// showing it again brings the drawing back.
    #[test]
    fn hiding_a_type_stops_its_gizmo() {
        let (mut world, _entities) = world_with_markers();
        let mut registry = GizmoRegistry::default();
        registry.publish(&[marker_declaration(GizmoWhen::Always)]);
        world.insert_resource(registry);
        world.insert_resource(GizmoSelection::default());
        world.insert_resource(DebugDraw::new(true));

        run_pass(&mut world, GizmoVisibility::EditorOnly).expect("the pass runs");
        let drawn = world
            .get_resource::<GizmoRegistry>()
            .expect("the registry is there")
            .entries()[0]
            .drew_last_frame();
        assert_eq!(drawn, 3);

        if let Some(registry) = world.get_resource_mut::<GizmoRegistry>() {
            registry.set_type_visible("tests::Marker", false);
        }
        run_pass(&mut world, GizmoVisibility::EditorOnly).expect("the pass runs");
        let drawn = world
            .get_resource::<GizmoRegistry>()
            .expect("the registry is there")
            .entries()[0]
            .drew_last_frame();
        assert_eq!(drawn, 0, "a hidden type draws nothing");

        if let Some(registry) = world.get_resource_mut::<GizmoRegistry>() {
            registry.set_type_visible("tests::Marker", true);
        }
        run_pass(&mut world, GizmoVisibility::EditorOnly).expect("the pass runs");
        let drawn = world
            .get_resource::<GizmoRegistry>()
            .expect("the registry is there")
            .entries()[0]
            .drew_last_frame();
        assert_eq!(drawn, 3, "showing the type draws again");
    }

    /// The lines a gizmo drew reach the debug drawing, marked editor-only in an
    /// editor and as plain drawing in a development host.
    #[test]
    fn gizmo_lines_are_editor_only_only_in_an_editor() {
        for (visibility, expected) in [
            (GizmoVisibility::EditorOnly, true),
            (GizmoVisibility::Everywhere, false),
        ] {
            let (mut world, _entities) = world_with_markers();
            let mut registry = GizmoRegistry::default();
            registry.publish(&[marker_declaration(GizmoWhen::Always)]);
            world.insert_resource(registry);
            world.insert_resource(GizmoSelection::default());
            world.insert_resource(DebugDraw::new(true));

            run_pass(&mut world, visibility).expect("the pass runs");
            let frame = world
                .get_resource_mut::<DebugDraw>()
                .expect("debug drawing is there")
                .collect_frame(0.0);
            assert_eq!(frame.lines.len(), 3);
            assert!(
                frame.lines.iter().all(|line| line.editor_only == expected),
                "editor_only must be {expected} for {visibility:?}"
            );
            assert!(
                frame.lines.iter().all(|line| line.depth
                    == DebugDepth::Dimmed {
                        hidden: Default::default()
                    }),
                "a gizmo's parts behind geometry draw in the hidden look"
            );
        }
    }

    /// A gizmo declared for the selection only draws nothing until an editor
    /// selects something.
    #[test]
    fn the_frame_reports_the_selection() {
        let (mut world, entities) = world_with_markers();
        let mut registry = GizmoRegistry::default();
        registry.publish(&[marker_declaration(GizmoWhen::Always)]);
        world.insert_resource(registry);
        world.insert_resource(GizmoSelection {
            selected: vec![entities[1]],
            hovered: Some(entities[2]),
        });
        world.insert_resource(DebugDraw::new(true));

        run_pass(&mut world, GizmoVisibility::EditorOnly).expect("the pass runs");
        let frame = world
            .get_resource::<GizmoSelection>()
            .map(|selection| GizmoFrame::new(selection.selected.clone(), selection.hovered))
            .expect("the selection is there");
        assert!(frame.is_selected(entities[1]));
        assert!(!frame.is_selected(entities[0]));
        assert_eq!(frame.hovered(), Some(entities[2]));
    }

    /// The buffer's shapes become the segments a renderer draws.
    #[test]
    fn shapes_become_segments() {
        let mut draw = GizmoDraw::new();
        assert!(draw.is_empty());
        draw.box_outline([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3], [1.0; 4]);
        assert_eq!(draw.segment_count(), 12, "a box has twelve edges");
        draw.circle([0.0; 3], [0.0, 1.0, 0.0], 1.0, [1.0; 4]);
        assert_eq!(draw.segment_count(), 12 + CIRCLE_SEGMENTS);
        draw.line_loop(&[[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]], [1.0; 4]);
        assert_eq!(draw.segment_count(), 12 + CIRCLE_SEGMENTS + 3);
        draw.label([0.0; 3], "a label", [1.0; 4]);
        assert!(!draw.is_empty());
    }

    /// A point is a mark measured in pixels, not three world-sized axis
    /// segments, and it reaches the frame unpickable.
    #[test]
    fn a_point_is_a_pixel_sized_mark_a_pick_passes_through() {
        let mut draw = GizmoDraw::new();
        draw.point([1.0, 2.0, 3.0], 9.0, GizmoPointShape::Square, [0.5; 4]);
        assert_eq!(
            draw.segment_count(),
            0,
            "a mark draws no segments, so its size cannot be world units"
        );
        assert!(!draw.is_empty(), "the mark is still something to draw");

        let mut debug_draw = DebugDraw::new(true);
        draw.flush_into(&mut debug_draw, true);
        let frame = debug_draw.collect_frame(0.0);
        assert_eq!(frame.gizmo_points.len(), 1);
        let mark = frame.gizmo_points[0];
        assert_eq!(mark.position, [1.0, 2.0, 3.0]);
        assert_eq!(mark.size_pixels, 9.0);
        assert_eq!(mark.shape, GizmoPointShape::Square);
        assert_eq!(mark.color, [0.5; 4]);
        assert!(
            !mark.pickable,
            "a mark shows a position; a pick must reach what is behind it"
        );
    }

    /// The entity transform reaches a world point, scale included.
    #[test]
    fn the_transform_turns_local_points_into_world_points() {
        let (mut world, entities) = world_with_markers();
        world.insert_resource(GizmoSelection::default());
        if let Some(transform) = world.get_component_mut::<TransformComponent>(entities[2]) {
            transform.translation = [1.0, 2.0, 3.0];
            transform.scale = [2.0, 2.0, 2.0];
        }
        let frame = GizmoFrame::default();
        let context = frame.context(&world, entities[2]);
        assert_eq!(context.translation(), [1.0, 2.0, 3.0]);
        assert_eq!(context.scale(), [2.0, 2.0, 2.0]);
        assert_eq!(
            transform_point(&context.to_world(), [1.0, 0.0, 0.0]),
            [3.0, 2.0, 3.0],
            "a scaled local point is scaled, then placed"
        );
        assert_eq!(
            context
                .get_component::<TransformComponent>()
                .map(|transform| transform.translation),
            Some([1.0, 2.0, 3.0]),
            "a gizmo reads a component beside its own"
        );
        assert!(
            context.get_resource::<GizmoSelection>().is_some(),
            "a gizmo reads a resource of its world"
        );
    }
}
