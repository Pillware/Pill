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
//! [chrome](DebugStyle::chrome) in an editor, so a game's own surface never
//! draws them, and plain debug lines in a development host, whose single
//! surface is a game's.
//!
//! The registry holds owned strings and function pointers. The strings are
//! copies because a module's `&'static str` would outlive nothing once its image
//! is unmapped, and the pointers are replaced by the publishing image on every
//! reload before the old image goes away.

// Standard library
use std::collections::BTreeSet;

// Current crate
use crate::ecs::common_components::TransformComponent;
use crate::ecs::component::Component;
use crate::ecs::entity::Entity;
use crate::ecs::resource::{PlayState, Resource};
use crate::ecs::world::World;
use crate::engine::{Engine, SystemOptions};
use crate::error::SystemError;
use crate::services::debug_draw::{DebugColor, DebugDraw, DebugLifetime, DebugStyle, LabelAnchor};
use crate::systems::scheduler::RunPhase;

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

/// The shapes one gizmo draws for one frame: lines, circles, boxes, points and
/// labels, all in world space.
///
/// The pass hands the buffer to [`DebugDraw`] once every gizmo ran, so a draw
/// call never needs the world's resource and a gizmo cannot interleave with
/// another one's drawing.
#[derive(Debug, Clone, Default)]
pub struct GizmoDraw {
    /// The segments, in the order they were drawn.
    segments: Vec<([[f32; 3]; 2], DebugColor)>,
    /// The labels, in the order they were drawn.
    labels: Vec<([f32; 3], String, DebugColor)>,
}

impl GizmoDraw {
    /// Creates an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether nothing was drawn yet.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty() && self.labels.is_empty()
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

    /// Draws a small cross at `position` - the mark a gizmo uses where it has
    /// no shape to outline.
    pub fn point(&mut self, position: [f32; 3], size: f32, color: DebugColor) {
        for axis in 0..3 {
            let mut start = position;
            let mut end = position;
            start[axis] -= size;
            end[axis] += size;
            self.line(start, end, color);
        }
    }

    /// Draws a text label at a world position.
    pub fn label(&mut self, position: [f32; 3], text: impl Into<String>, color: DebugColor) {
        self.labels.push((position, text.into(), color));
    }

    /// Hands everything drawn into `debug_draw`, as chrome (`true`) or as plain
    /// debug drawing (`false`).
    fn flush_into(&self, debug_draw: &mut DebugDraw, chrome: bool) {
        for (segment, color) in &self.segments {
            debug_draw.line(
                segment[0],
                segment[1],
                DebugStyle {
                    color: *color,
                    depth_test: true,
                    chrome,
                    lifetime: DebugLifetime::OneFrame,
                },
            );
        }
        for (position, text, color) in &self.labels {
            debug_draw.label(
                LabelAnchor::World(*position),
                text,
                DebugStyle {
                    color: *color,
                    depth_test: false,
                    chrome,
                    lifetime: DebugLifetime::OneFrame,
                },
            );
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
/// The derive macro emits one of these into the component descriptor, so it
/// lives in a `static` and every field is a constant.
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
        if declarations.is_empty() {
            return;
        }
        let owners: BTreeSet<&str> = declarations
            .iter()
            .map(|declaration| declaration.owner)
            .collect();
        let published: BTreeSet<&str> = declarations
            .iter()
            .map(|declaration| declaration.type_name)
            .collect();
        // Step 1: drop the entries these packages published before, so a
        // reloaded image never leaves a pointer to code that is being unmapped.
        self.entries.retain(|entry| {
            !owners.contains(entry.owner.as_str()) || published.contains(entry.type_name.as_str())
        });
        // Step 2: add the ones that are not there yet, keeping what a frame
        // already counted. A kept entry takes the arriving image's text as
        // well as its pointers: the text an earlier generation published lives
        // in that generation's statics.
        for declaration in declarations {
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

    /// Records what each gizmo drew in the frame that just ran.
    fn set_draw_counts(&mut self, counts: &[usize]) {
        for (entry, count) in self.entries.iter_mut().zip(counts) {
            entry.drew_last_frame = *count;
        }
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
// GizmoChrome
// =============================================================================

/// What the pass's drawing is, which decides the surfaces that show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoChrome {
    /// Editor chrome: drawn in the editor's own views and never in a game's.
    Editor,
    /// Plain debug drawing: drawn on every surface, which is what a
    /// development host wants - its only surface is a game's.
    Game,
}

// =============================================================================
// The pass
// =============================================================================

/// Installs the one pass that draws every registered gizmo.
///
/// Called by a host, never by a game: an editor passes
/// [`GizmoChrome::Editor`], a development host [`GizmoChrome::Game`], and a
/// shipping or web build installs nothing. The pass runs in the post-project
/// phase, also while editing, and belongs to the engine so no reload retires it.
///
/// Installing twice changes nothing: the second call finds the system already
/// registered.
pub fn install(engine: &mut Engine, chrome: GizmoChrome) {
    if engine.is_registered(SYSTEM_NAME) {
        return;
    }
    engine.register_engine_owned_system(
        SYSTEM_NAME,
        SystemOptions::new(RunPhase::PostProject, PASS_ORDER).with_run_in_edit_mode(true),
        move |world: &mut World, _queue| run_pass(world, chrome),
    );
}

/// One pass: every visible gizmo draws into one buffer, which then reaches the
/// world's [`DebugDraw`].
fn run_pass(world: &mut World, chrome: GizmoChrome) -> Result<(), SystemError> {
    // Step 1: read what this frame draws for, and which entries may draw at
    // all. The registry borrow ends here, before a gizmo reads the world.
    let Some(gizmo_registry) = world.get_resource::<GizmoRegistry>() else {
        return Ok(());
    };
    if gizmo_registry.is_empty() {
        return Ok(());
    }
    // A switched-off pass draws nothing and says so: the counts are what
    // `dump gizmos` reads, so they must not stay at the last frame's numbers.
    if !gizmo_registry.is_enabled() {
        let entry_count = gizmo_registry.len();
        if let Some(gizmo_registry) = world.get_resource_mut::<GizmoRegistry>() {
            gizmo_registry.set_draw_counts(&vec![0; entry_count]);
        }
        return Ok(());
    }
    let frame = world
        .get_resource::<GizmoSelection>()
        .map(|selection| GizmoFrame::new(selection.selected.clone(), selection.hovered))
        .unwrap_or_default();
    let mut drawable: Vec<(usize, GizmoDrawEntry)> = gizmo_registry
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, entry)| gizmo_registry.is_type_visible(&entry.type_name))
        .map(|(index, entry)| (index, entry.draw))
        .collect();
    let entry_count = gizmo_registry.len();
    if drawable.is_empty() {
        if let Some(gizmo_registry) = world.get_resource_mut::<GizmoRegistry>() {
            gizmo_registry.set_draw_counts(&vec![0; entry_count]);
        }
        return Ok(());
    }

    // Step 2: every visible gizmo draws into the one buffer.
    let mut buffer = GizmoDraw::new();
    let mut counts = vec![0_usize; entry_count];
    for (index, draw) in drawable.drain(..) {
        counts[index] = draw(world, &frame, &mut buffer);
    }

    // Step 3: the buffer becomes the world's debug drawing. A world with none
    // - a game that never asked for debug drawing - still records the counts,
    // so `dump gizmos` can tell "nothing drawn" from "not registered".
    if !buffer.is_empty() {
        if let Some(debug_draw) = world.get_resource_mut::<DebugDraw>() {
            buffer.flush_into(debug_draw, chrome == GizmoChrome::Editor);
        }
    }
    if let Some(gizmo_registry) = world.get_resource_mut::<GizmoRegistry>() {
        gizmo_registry.set_draw_counts(&counts);
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::world::World;

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

        run_pass(&mut world, GizmoChrome::Editor).expect("the pass runs");
        let drawn = world
            .get_resource::<GizmoRegistry>()
            .expect("the registry is there")
            .entries()[0]
            .drew_last_frame();
        assert_eq!(drawn, 3);

        if let Some(registry) = world.get_resource_mut::<GizmoRegistry>() {
            registry.set_type_visible("tests::Marker", false);
        }
        run_pass(&mut world, GizmoChrome::Editor).expect("the pass runs");
        let drawn = world
            .get_resource::<GizmoRegistry>()
            .expect("the registry is there")
            .entries()[0]
            .drew_last_frame();
        assert_eq!(drawn, 0, "a hidden type draws nothing");

        if let Some(registry) = world.get_resource_mut::<GizmoRegistry>() {
            registry.set_type_visible("tests::Marker", true);
        }
        run_pass(&mut world, GizmoChrome::Editor).expect("the pass runs");
        let drawn = world
            .get_resource::<GizmoRegistry>()
            .expect("the registry is there")
            .entries()[0]
            .drew_last_frame();
        assert_eq!(drawn, 3, "showing the type draws again");
    }

    /// The lines a gizmo drew reach the debug drawing, marked as chrome in an
    /// editor and as plain drawing in a development host.
    #[test]
    fn gizmo_lines_are_chrome_only_in_an_editor() {
        for (chrome, expected) in [(GizmoChrome::Editor, true), (GizmoChrome::Game, false)] {
            let (mut world, _entities) = world_with_markers();
            let mut registry = GizmoRegistry::default();
            registry.publish(&[marker_declaration(GizmoWhen::Always)]);
            world.insert_resource(registry);
            world.insert_resource(GizmoSelection::default());
            world.insert_resource(DebugDraw::new(true));

            run_pass(&mut world, chrome).expect("the pass runs");
            let frame = world
                .get_resource_mut::<DebugDraw>()
                .expect("debug drawing is there")
                .collect_frame(0.0);
            assert_eq!(frame.lines.len(), 3);
            assert!(
                frame.lines.iter().all(|line| line.chrome == expected),
                "chrome must be {expected} for {chrome:?}"
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

        run_pass(&mut world, GizmoChrome::Editor).expect("the pass runs");
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
        draw.point([0.0; 3], 0.1, [1.0; 4]);
        assert_eq!(draw.segment_count(), 12 + CIRCLE_SEGMENTS + 3);
        draw.line_loop(&[[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]], [1.0; 4]);
        assert_eq!(draw.segment_count(), 12 + CIRCLE_SEGMENTS + 3 + 3);
        draw.label([0.0; 3], "a label", [1.0; 4]);
        assert!(!draw.is_empty());
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
