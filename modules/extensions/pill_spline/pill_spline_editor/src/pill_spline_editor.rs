//! The spline extension's editor part: the tool that edits a `Spline`.
//!
//! # Responsibilities
//!
//! - Declares the `spline` tool: the `control point` and `tangent` kinds, the
//!   `Spline` coordinate frame, the rows its element context menu offers and
//!   the keys it answers.
//! - Reports a spline's points and tangents as elements, and writes a drag back
//!   into the component, so the editor's gizmo, frames, snapping and undo work
//!   on a spline exactly as they do on an entity's transform.
//! - Inserts and removes control points through the editor's recorded element
//!   commands, so one edit is one undo step.
//! - Registers the tool into the world's registry on every generation.
//!
//! # Design
//!
//! A spline is a component tool: it is offered while the selected entity
//! carries a [`Spline`], and it acts on that entity's element selection. The
//! tool owns the mapping between the component and the editor's plain element
//! vocabulary - an element id is a control point's index, or a tangent's index
//! offset by [`TANGENT_ID_BASE`] - so the editor never reads the component's
//! fields itself.
//!
//! Points live in the spline's own space and the entity's transform places
//! them, which is why every element the tool reports and every drag it takes is
//! converted through that transform here. The tool is therefore also the one
//! place that knows a spline is edited in the entity's space, not the world's.
//!
//! The stored tangents follow [`TangentMode`]: an auto spline refits them from
//! the points after every edit, so switching to a stored mode later starts from
//! the curve the reader was looking at, while an aligned or broken spline keeps
//! the tangents it has. Aligned mirrors a written tangent onto the point's
//! other side; broken leaves the two to part.
//!
//! Registration takes a [`ToolRegistry`] rather than an engine, so a suite can
//! drive the tool's hooks without starting a host. The record it writes holds
//! owned strings and plain function pointers, which is what lets it outlive the
//! image that declared it and be replaced by a reload.

// External crates
use pill_core::math::Vector3f;
use pill_editor_core::editor_state::{EditorCommand, SelectionClick};
use pill_editor_core::tools::{
    self, CoordinateFrame, EditorTool, ElementKind, ElementMenuEntry, ElementMove,
    ElementTransform, KeyResponse, PressResponse, ToolElement, ToolElementHooks, ToolRegistry,
};
use pill_engine::{pill_module, Engine, Entity, TransformComponent, World};
use pill_spline::{Spline, TangentMode};

// The build script scans this crate and emits one address entry per function
// into `function_inventory.rs`; the `include!` is what makes every function
// resolvable by qualified path with nothing in this file annotated.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Constants
// =============================================================================

/// The name `tool spline` activates and the toolbox row shows.
pub const TOOL_NAME: &str = "spline";

/// The label the toolbox row carries.
pub const TOOL_LABEL: &str = "Spline";

/// The kind a control point is reported under.
pub const POINT_KIND: &str = "control point";

/// The kind a tangent handle is reported under.
pub const TANGENT_KIND: &str = "tangent";

/// The first element id a tangent handle uses.
///
/// Control points take the low ids (their index), so the two ranges can never
/// collide however long a spline is.
pub const TANGENT_ID_BASE: u64 = 100;

/// The name of the coordinate frame the gizmo offers while this tool is active.
pub const FRAME_NAME: &str = "Spline";

/// How wide a control point's handle draws, in pixels.
const POINT_SIZE_PIXELS: f32 = 9.0;

/// How wide a tangent handle draws, in pixels. Smaller than a point, so a
/// reader tells the two kinds apart at a glance.
const TANGENT_SIZE_PIXELS: f32 = 7.0;

/// The colour a control point's handle draws in.
const POINT_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 1.0];

/// The colour a tangent handle draws in.
const TANGENT_COLOR: [f32; 4] = [0.35, 0.75, 0.95, 1.0];

/// The menu row that appends a control point after the selection.
///
/// The id is one word because the scripted channel names a row with one: the
/// label is what a reader sees.
pub const MENU_INSERT: &str = "insert";

/// The menu row that removes the selected control points.
pub const MENU_DELETE: &str = "delete";

/// The key that appends a control point after the selection.
pub const KEY_INSERT: &str = "i";

/// How far past the last point a new one is placed, along the curve's end
/// tangent, in spline units.
const INSERT_DISTANCE: f32 = 50.0;

// =============================================================================
// The Tool
// =============================================================================

/// The spline tool itself: a component tool over one entity's [`Spline`].
pub struct SplineTool;

impl EditorTool for SplineTool {
    fn name(&self) -> &'static str {
        TOOL_NAME
    }

    fn label(&self) -> &'static str {
        TOOL_LABEL
    }

    /// A component tool: it acts on the selected entity's spline, so it is not
    /// offered whatever the selection holds.
    fn is_global(&self) -> bool {
        false
    }
}

/// Registers the spline tool and its element hooks into `registry`.
///
/// Registration replaces a record of the same name, which is what a reload of
/// this crate relies on: the arriving image's hooks take the old ones' place.
pub fn register_into(registry: &mut ToolRegistry) {
    registry.register_plugin(
        &SplineTool,
        ToolElementHooks {
            kinds: Some(spline_kinds),
            elements: Some(spline_elements),
            set_transforms: Some(spline_set_transforms),
            frames: Some(spline_frames),
            menu: Some(spline_menu),
            menu_run: Some(spline_menu_run),
            insert: Some(spline_insert),
            remove: Some(spline_remove),
            press: Some(spline_press),
            key: Some(spline_key),
            selection_changed: None,
        },
    );
}

/// Registers the tool against a host engine, which runs on every generation.
///
/// # Errors
///
/// Returns 0 always: the tool registers no world state that can fail.
pub fn register(engine: &mut Engine) -> u32 {
    register_into(tools::tool_registry_mut(engine.world_mut()));
    0
}

/// The artifact's one-time entry point.
///
/// The module keeps nothing that has to run once, so this defers to
/// [`register`]: a statically linked build loads the module once and reaches
/// this alone.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

// =============================================================================
// Kinds and elements
// =============================================================================

/// The two kinds the tool exposes: the points the curve passes through and the
/// tangent handles beside them.
fn spline_kinds(out: &mut Vec<ElementKind>) {
    let mut point = ElementKind::new(POINT_KIND);
    point.size_pixels = POINT_SIZE_PIXELS;
    point.color = POINT_COLOR;
    out.push(point);

    let mut tangent = ElementKind::new(TANGENT_KIND);
    tangent.size_pixels = TANGENT_SIZE_PIXELS;
    tangent.color = TANGENT_COLOR;
    // A tangent handle is a single arm, so boxing a set of them would select
    // whatever happens to be inside the box rather than the arms the reader
    // meant.
    tangent.box_select = false;
    out.push(tangent);
}

/// The elements `entity`'s spline has: one per control point and one per
/// point's out-tangent.
///
/// Every place is turned into world space through the entity's transform, so
/// the editor's handles land where the curve is drawn.
fn spline_elements(entity: Entity, world: &World, out: &mut Vec<ToolElement>) {
    let Some(spline) = world.get_component::<Spline>(entity) else {
        return;
    };
    let placement = Placement::of(world, entity);
    for (index, point) in spline.control_points().iter().enumerate() {
        out.push(ToolElement {
            id: index as u64,
            kind: POINT_KIND.to_string(),
            transform: ElementTransform::at(placement.to_world(*point)),
        });
        out.push(ToolElement {
            id: TANGENT_ID_BASE + index as u64,
            kind: TANGENT_KIND.to_string(),
            transform: ElementTransform::at(
                placement.to_world(*point + spline.out_tangents[index]),
            ),
        });
    }
}

/// The `Spline` frame: the gizmo's axes follow the tangent of the point the
/// selection is on, so a drag along X follows the curve instead of the world.
fn spline_frames(entity: Entity, selected: &[u64], world: &World, out: &mut Vec<CoordinateFrame>) {
    let Some(spline) = world.get_component::<Spline>(entity) else {
        return;
    };
    if spline.control_points().is_empty() {
        return;
    }
    // The first selected control point names the place the frame is computed
    // at; a selection of tangent handles leaves the curve's first point to
    // speak for it.
    let focus = selected
        .iter()
        .find_map(|id| point_index_of(*id))
        .unwrap_or(0)
        .min(spline.control_points().len() - 1);
    let placement = Placement::of(world, entity);
    let tangent = placement.to_world_direction(spline.out_tangents[focus]);
    out.push(CoordinateFrame {
        name: FRAME_NAME.to_string(),
        rotation: rotation_following(tangent),
    });
}

// =============================================================================
// Writing edits back
// =============================================================================

/// Writes a drag's world transforms into the spline.
///
/// A control point takes the place the drag gave it, measured back into the
/// spline's own space. A tangent handle sets the vector from its point to where
/// it was dragged to, and an aligned spline mirrors it onto the point's other
/// side.
fn spline_set_transforms(entity: Entity, moves: &[ElementMove], world: &mut World) {
    let placement = Placement::of(world, entity);
    let Some(spline) = world.get_component_mut::<Spline>(entity) else {
        return;
    };
    for moved in moves {
        let local = placement.to_local(moved.transform.translation);
        if point_index_of(moved.element).is_some() {
            spline.set_control_point(moved.element as usize, local);
            continue;
        }
        let index = moved.element.saturating_sub(TANGENT_ID_BASE) as usize;
        if index >= spline.control_points().len() {
            continue;
        }
        let tangent = local - spline.control_points[index];
        spline.out_tangents[index] = tangent;
        if spline.tangent_mode() == TangentMode::Aligned {
            spline.in_tangents[index] = tangent;
        }
    }
    if spline.tangent_mode() == TangentMode::Auto {
        spline.refit_tangents();
    }
}

/// Inserts one control point, answering the index it took.
///
/// `id` is the index the editor asks the point to take, which is what an undo
/// restoring a removed point passes; `None` appends, which is what the menu row
/// and the key do.
fn spline_insert(
    entity: Entity,
    kind: &str,
    id: Option<u64>,
    transform: ElementTransform,
    world: &mut World,
) -> Option<u64> {
    if kind != POINT_KIND {
        return None;
    }
    let placement = Placement::of(world, entity);
    let local = placement.to_local(transform.translation);
    let spline = world.get_component_mut::<Spline>(entity)?;
    let index = match id {
        Some(requested) => spline.insert_control_point(requested as usize, local)?,
        None => spline.insert_control_point(spline.control_points().len(), local)?,
    };
    if spline.tangent_mode() == TangentMode::Auto {
        spline.refit_tangents();
    }
    Some(index as u64)
}

/// Removes control points, answering how many went.
///
/// The ids are removed from the highest down, so each index still names the
/// point it did when the caller collected them.
fn spline_remove(entity: Entity, kind: &str, elements: &[u64], world: &mut World) -> usize {
    if kind != POINT_KIND {
        return 0;
    }
    let Some(spline) = world.get_component_mut::<Spline>(entity) else {
        return 0;
    };
    let mut indices: Vec<usize> = elements
        .iter()
        .filter_map(|id| point_index_of(*id))
        .collect();
    indices.sort_unstable_by(|left, right| right.cmp(left));
    let removed = indices
        .into_iter()
        .filter(|index| spline.remove_control_point(*index))
        .count();
    if spline.tangent_mode() == TangentMode::Auto {
        spline.refit_tangents();
    }
    removed
}

/// Answers a press on one of the tool's elements.
///
/// The editor's own rule is what a spline wants: a press selects and starts a
/// drag, and a Ctrl press extends the selection. Only a press on a tangent of a
/// point that is not selected is taken over, because dragging a tangent is how
/// a reader bends the curve.
fn spline_press(
    _entity: Entity,
    _element: u64,
    _click: SelectionClick,
    _selected: &[u64],
    _world: &World,
) -> PressResponse {
    PressResponse::Default
}

// =============================================================================
// Menus and keys
// =============================================================================

/// The rows the tool adds to the element context menu.
fn spline_menu(
    entity: Entity,
    kind: &str,
    selected: &[u64],
    world: &World,
    out: &mut Vec<ElementMenuEntry>,
) {
    if kind != POINT_KIND {
        return;
    }
    let Some(spline) = world.get_component::<Spline>(entity) else {
        return;
    };
    if spline.control_points().len() < pill_spline::MAX_CONTROL_POINTS {
        out.push(ElementMenuEntry {
            id: MENU_INSERT.to_string(),
            label: "Insert point".to_string(),
        });
    }
    if !selected.is_empty() {
        out.push(ElementMenuEntry {
            id: MENU_DELETE.to_string(),
            label: "Delete point".to_string(),
        });
    }
}

/// Runs one of the tool's menu rows.
///
/// Both rows answer element commands, so the editor records them as one undo
/// step like any other edit.
fn spline_menu_run(
    entity: Entity,
    kind: &str,
    selected: &[u64],
    item: &str,
    world: &World,
) -> Result<Vec<EditorCommand>, String> {
    if kind != POINT_KIND {
        return Err(format!(
            "the spline tool adds no '{item}' row for {kind} elements"
        ));
    }
    match item {
        MENU_INSERT => Ok(vec![insert_point_command(entity, selected, world)?]),
        MENU_DELETE => {
            let points: Vec<u64> = selected
                .iter()
                .copied()
                .filter_map(point_index_of)
                .map(|index| index as u64)
                .collect();
            if points.is_empty() {
                return Err("no control point is selected to delete".to_string());
            }
            Ok(vec![EditorCommand::RemoveElements {
                entity,
                kind: POINT_KIND.to_string(),
                elements: points,
            }])
        }
        other => Err(format!("the spline tool has no menu row '{other}'")),
    }
}

/// The key hook: `i` appends a control point, the same edit the menu row makes.
fn spline_key(entity: Entity, key: &str, selected: &[u64], world: &World) -> KeyResponse {
    if !key.eq_ignore_ascii_case(KEY_INSERT) {
        return KeyResponse::Default;
    }
    match insert_point_command(entity, selected, world) {
        Ok(command) => KeyResponse::Commands(vec![command]),
        // A full spline cannot take another point, so the key falls through to
        // the editor instead of consuming it and doing nothing.
        Err(_) => KeyResponse::Default,
    }
}

/// The recorded insert a menu row and the key both mean: a point one insert
/// step past the last selected one, along the curve's end.
fn insert_point_command(
    entity: Entity,
    selected: &[u64],
    world: &World,
) -> Result<EditorCommand, String> {
    let spline = world
        .get_component::<Spline>(entity)
        .ok_or_else(|| "the entity carries no spline".to_string())?;
    let point_count = spline.control_points().len();
    if point_count >= pill_spline::MAX_CONTROL_POINTS {
        return Err("the spline is full".to_string());
    }
    let after = selected
        .iter()
        .filter_map(|id| point_index_of(*id))
        .max()
        .unwrap_or(point_count.saturating_sub(1));
    let after = after.min(point_count.saturating_sub(1));
    let anchor = spline
        .control_points()
        .get(after)
        .copied()
        .unwrap_or_default();
    let direction = if spline.out_tangents[after].length_squared() > 0.0 {
        spline.out_tangents[after].normalize_or_zero()
    } else {
        Vector3f::X
    };
    let placement = Placement::of(world, entity);
    Ok(EditorCommand::InsertElements {
        entity,
        kind: POINT_KIND.to_string(),
        elements: vec![ElementMove {
            element: (after + 1) as u64,
            transform: ElementTransform::at(
                placement.to_world(anchor + direction * INSERT_DISTANCE),
            ),
        }],
    })
}

// =============================================================================
// Element ids
// =============================================================================

/// The control point an element id names, or `None` when it names a tangent.
fn point_index_of(element: u64) -> Option<usize> {
    (element < TANGENT_ID_BASE).then_some(element as usize)
}

// =============================================================================
// Placement
// =============================================================================

/// An entity's transform, as the tool reads it: the spline's own space to the
/// world and back.
///
/// The maths is written once here so every hook agrees about where a point is,
/// and a spline under a rotated or scaled entity moves as one piece.
struct Placement {
    translation: Vector3f,
    rotation: [f32; 4],
    scale: Vector3f,
}

impl Placement {
    /// The placement of `entity`, or the identity when it has no transform.
    fn of(world: &World, entity: Entity) -> Placement {
        let transform = world
            .get_component::<TransformComponent>(entity)
            .copied()
            .unwrap_or_default();
        Placement {
            translation: Vector3f::from_array(transform.translation),
            rotation: transform.rotation,
            scale: Vector3f::from_array(transform.scale),
        }
    }

    /// A place in the spline's own space, in world space.
    fn to_world(&self, point: Vector3f) -> [f32; 3] {
        (rotate(self.rotation, point * self.scale) + self.translation).to_array()
    }

    /// A direction in the spline's own space, in world space.
    ///
    /// Only the rotation applies: a direction has no place, and scaling it
    /// would turn a tangent's length into a measurement of the entity.
    fn to_world_direction(&self, direction: Vector3f) -> Vector3f {
        rotate(self.rotation, direction)
    }

    /// A place in world space, back in the spline's own space.
    fn to_local(&self, point: [f32; 3]) -> Vector3f {
        let offset = Vector3f::from_array(point) - self.translation;
        let unrotated = rotate(inverse_rotation(self.rotation), offset);
        Vector3f::new(
            if self.scale.x.abs() > f32::EPSILON {
                unrotated.x / self.scale.x
            } else {
                unrotated.x
            },
            if self.scale.y.abs() > f32::EPSILON {
                unrotated.y / self.scale.y
            } else {
                unrotated.y
            },
            if self.scale.z.abs() > f32::EPSILON {
                unrotated.z / self.scale.z
            } else {
                unrotated.z
            },
        )
    }
}

// =============================================================================
// Rotation maths
// =============================================================================

/// Turns `vector` by the unit quaternion `rotation`, `x y z w` order.
fn rotate(rotation: [f32; 4], vector: Vector3f) -> Vector3f {
    let [x, y, z, w] = rotation;
    // The standard quaternion sandwich, with the vector's `w` held at zero.
    let axis = Vector3f::new(x, y, z);
    let twice_cross = axis.cross(vector) * 2.0;
    vector + twice_cross * w + axis.cross(twice_cross)
}

/// The inverse of a unit quaternion: its rotation the other way round.
fn inverse_rotation(rotation: [f32; 4]) -> [f32; 4] {
    [-rotation[0], -rotation[1], -rotation[2], rotation[3]]
}

/// The rotation that turns the world's `+Y` axis onto `direction`.
///
/// A direction along `-Y` is the one case the axis-angle form cannot express,
/// and a zero direction means "no frame", which the identity answers.
fn rotation_following(direction: Vector3f) -> [f32; 4] {
    let forward = direction.normalize_or_zero();
    if forward.length_squared() < 0.5 {
        return ElementTransform::IDENTITY_ROTATION;
    }
    let up = Vector3f::Y;
    let dot = up.dot(forward).clamp(-1.0, 1.0);
    if dot < -0.999 {
        // A half turn about any axis across `Y`; X is as good as any.
        return [1.0, 0.0, 0.0, 0.0];
    }
    let axis = up.cross(forward);
    let angle = dot.acos();
    let half = angle * 0.5;
    let sine = half.sin() / axis.length();
    [axis.x * sine, axis.y * sine, axis.z * sine, half.cos()]
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_editor_core::tools::ToolRegistry;

    /// Tolerance for comparing places, generous enough for the accumulated
    /// error of the rotations in the placement maths.
    const EPSILON: f32 = 1e-4;

    /// A world with one entity carrying a spline and a transform.
    fn world_with_spline(points: &[Vector3f], translation: [f32; 3]) -> (World, Entity) {
        let mut world = World::default();
        // A world built by hand registers nothing, so the two components the
        // entity carries are registered here first, the way a host does on
        // load.
        pill_engine::register_transform_component(&mut world);
        pill_spline::register_component(&mut world);
        let entity = world
            .create_entity()
            .with(Spline::from_points(points))
            .with(TransformComponent {
                translation,
                ..TransformComponent::default()
            })
            .build()
            .expect("the spline entity builds");
        (world, entity)
    }

    /// A registry with the spline tool registered and active.
    fn active_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::default();
        register_into(&mut registry);
        registry
            .set_active(Some(TOOL_NAME))
            .expect("the spline tool is registered");
        registry
    }

    /// The tool reports one element per point and one per tangent, of two
    /// kinds, at the places the entity's transform gives them.
    #[test]
    fn the_tool_reports_points_and_tangents() {
        let (world, entity) = world_with_spline(
            &[
                Vector3f::new(0.0, 0.0, 0.0),
                Vector3f::new(10.0, 0.0, 0.0),
                Vector3f::new(20.0, 0.0, 0.0),
            ],
            [100.0, 0.0, 0.0],
        );
        let registry = active_registry();

        let kinds = registry.active_kinds(entity, &world);
        assert_eq!(
            kinds
                .iter()
                .map(|kind| kind.name.as_str())
                .collect::<Vec<&str>>(),
            [POINT_KIND, TANGENT_KIND]
        );
        let elements = registry.active_elements(entity, &world);
        assert_eq!(elements.len(), 6, "three points and three tangents");
        let points: Vec<&ToolElement> = elements
            .iter()
            .filter(|element| element.kind == POINT_KIND)
            .collect();
        assert_eq!(
            points
                .iter()
                .map(|element| element.id)
                .collect::<Vec<u64>>(),
            [0, 1, 2]
        );
        assert!(
            points[1]
                .transform
                .translation
                .iter()
                .copied()
                .zip([110.0, 0.0, 0.0])
                .all(|(got, want)| (got - want).abs() < EPSILON),
            "the entity's translation was not applied: {:?}",
            points[1].transform.translation
        );
        assert!(
            elements
                .iter()
                .filter(|element| element.kind == TANGENT_KIND)
                .all(|element| element.id >= TANGENT_ID_BASE),
            "a tangent took a control point's id"
        );
    }

    /// The frame follows the tangent of the selected point, so a drag along its
    /// X axis follows the curve.
    #[test]
    fn the_spline_frame_follows_the_tangent() {
        let points = vec![
            Vector3f::new(0.0, 0.0, 0.0),
            Vector3f::new(10.0, 10.0, 0.0),
            Vector3f::new(20.0, 20.0, 0.0),
        ];
        let (mut world, entity) = world_with_spline(&points, [0.0; 3]);
        // A stored tangent pointing up-and-right gives the frame something the
        // world axes do not already have.
        let spline = world
            .get_component_mut::<Spline>(entity)
            .expect("the entity carries a spline");
        spline.out_tangents[0] = Vector3f::new(1.0, 1.0, 0.0);
        let registry = active_registry();

        let frames = registry.active_frames(entity, &[0], &world);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].name, FRAME_NAME);
        let turned_up = rotate(frames[0].rotation, Vector3f::Y);
        assert!(
            turned_up.abs_diff_eq(Vector3f::new(1.0, 1.0, 0.0).normalize(), EPSILON),
            "the frame's Y axis is not the point's tangent: {turned_up:?}"
        );
    }

    /// A dragged point lands in the spline's own space, not in world space, and
    /// an auto spline refits its tangents afterwards.
    #[test]
    fn a_dragged_point_is_written_back_in_the_splines_space() {
        let (mut world, entity) = world_with_spline(
            &[
                Vector3f::new(0.0, 0.0, 0.0),
                Vector3f::new(10.0, 0.0, 0.0),
                Vector3f::new(20.0, 0.0, 0.0),
            ],
            [100.0, 5.0, 0.0],
        );
        let registry = active_registry();
        let mut moved = vec![ElementMove {
            element: 1,
            transform: ElementTransform::at([115.0, 9.0, 0.0]),
        }];
        assert!(
            registry.active_set_transforms(entity, &moved, &mut world),
            "the tool writes transforms back"
        );

        let spline = world.get_component::<Spline>(entity).expect("the spline");
        assert!(
            spline.control_points[1].abs_diff_eq(Vector3f::new(15.0, 4.0, 0.0), EPSILON),
            "the point landed at {:?} rather than in the spline's own space",
            spline.control_points[1]
        );
        assert!(
            spline.out_tangents[1].length_squared() > 0.0,
            "an auto spline did not refit its tangents after the drag"
        );

        // The same drag on a tangent handle sets the vector from its point to
        // where it was dragged, and an aligned spline mirrors it.
        let dragged_tangent = [115.0, 8.0, 0.0];
        moved = vec![ElementMove {
            element: TANGENT_ID_BASE,
            transform: ElementTransform::at(dragged_tangent),
        }];
        let mut aligned_world = world;
        let spline = aligned_world
            .get_component_mut::<Spline>(entity)
            .expect("the spline");
        spline.set_tangent_mode(TangentMode::Aligned);
        registry.active_set_transforms(entity, &moved, &mut aligned_world);
        let spline = aligned_world
            .get_component::<Spline>(entity)
            .expect("the spline");
        assert!(
            spline.out_tangents[0].abs_diff_eq(Vector3f::new(15.0, 3.0, 0.0), EPSILON),
            "the tangent handle wrote {:?}",
            spline.out_tangents[0]
        );
        assert_eq!(
            spline.in_tangents[0], spline.out_tangents[0],
            "an aligned spline left the two sides of a point apart"
        );
    }

    /// The menu offers an insert row and, once a point is selected, a delete
    /// row; both answer the element edits the editor records.
    #[test]
    fn the_menu_rows_answer_recorded_element_edits() {
        let (world, entity) = world_with_spline(
            &[Vector3f::new(0.0, 0.0, 0.0), Vector3f::new(10.0, 0.0, 0.0)],
            [0.0; 3],
        );
        let registry = active_registry();

        let empty_menu = registry.active_menu(entity, POINT_KIND, &[], &world);
        assert_eq!(
            empty_menu
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<&str>>(),
            [MENU_INSERT],
            "an empty selection offers only the insert row"
        );
        let selected_menu = registry.active_menu(entity, POINT_KIND, &[1], &world);
        assert_eq!(
            selected_menu
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<&str>>(),
            [MENU_INSERT, MENU_DELETE]
        );

        let insert = registry
            .active_menu_run(entity, POINT_KIND, &[1], MENU_INSERT, &world)
            .expect("the insert row runs");
        assert!(
            matches!(
                insert.as_slice(),
                [EditorCommand::InsertElements { kind, elements, .. }]
                    if kind == POINT_KIND && elements.len() == 1 && elements[0].element == 2
            ),
            "the insert row answered {insert:?}"
        );
        let delete = registry
            .active_menu_run(entity, POINT_KIND, &[1], MENU_DELETE, &world)
            .expect("the delete row runs");
        assert!(
            matches!(
                delete.as_slice(),
                [EditorCommand::RemoveElements { elements, .. }] if elements == &[1]
            ),
            "the delete row answered {delete:?}"
        );

        // The key means the same insert, and a full spline leaves the key to
        // the editor rather than consuming it.
        let key = registry.active_key(entity, POINT_KIND, KEY_INSERT, &[1], &world);
        assert!(
            matches!(key, KeyResponse::Commands(ref commands) if matches!(
                commands.as_slice(),
                [EditorCommand::InsertElements { elements, .. }] if elements[0].element == 2
            )),
            "the insert key answered {key:?}"
        );
        let mut full = world;
        let spline = full
            .get_component_mut::<Spline>(entity)
            .expect("the spline");
        for index in 0..pill_spline::MAX_CONTROL_POINTS {
            spline.control_points[index] = Vector3f::new(index as f32, 0.0, 0.0);
        }
        spline.control_point_count = pill_spline::MAX_CONTROL_POINTS as u32;
        assert_eq!(
            registry.active_key(entity, POINT_KIND, KEY_INSERT, &[1], &full),
            KeyResponse::Default,
            "a full spline consumed the insert key"
        );
    }

    /// The insert and remove hooks put a point where the editor asks and take
    /// one away again, which is what an undo of either step needs.
    #[test]
    fn inserting_and_removing_points_reaches_the_component() {
        let (mut world, entity) = world_with_spline(
            &[
                Vector3f::new(0.0, 0.0, 0.0),
                Vector3f::new(10.0, 0.0, 0.0),
                Vector3f::new(20.0, 0.0, 0.0),
            ],
            [0.0; 3],
        );
        let registry = active_registry();

        let inserted = registry.active_insert(
            entity,
            POINT_KIND,
            Some(1),
            ElementTransform::at([5.0, 1.0, 0.0]),
            &mut world,
        );
        assert_eq!(inserted, Some(1), "the point did not take the asked id");
        let spline = world.get_component::<Spline>(entity).expect("the spline");
        assert_eq!(spline.control_points().len(), 4);
        assert!(spline.control_points[1].abs_diff_eq(Vector3f::new(5.0, 1.0, 0.0), EPSILON));

        let removed = registry
            .active_remove(entity, POINT_KIND, &[1], &mut world)
            .expect("the tool removes points");
        assert_eq!(removed, 1);
        let spline = world.get_component::<Spline>(entity).expect("the spline");
        assert_eq!(spline.control_points().len(), 3);
        assert_eq!(spline.control_points[1], Vector3f::new(10.0, 0.0, 0.0));
    }

    /// Registering twice keeps one record, which is what a reload relies on.
    #[test]
    fn registering_twice_keeps_one_record() {
        let mut registry = ToolRegistry::default();
        register_into(&mut registry);
        register_into(&mut registry);
        assert_eq!(
            registry
                .tools()
                .iter()
                .filter(|tool| tool.name == TOOL_NAME)
                .count(),
            1
        );
        let record = registry
            .tools()
            .iter()
            .find(|tool| tool.name == TOOL_NAME)
            .expect("the spline tool is registered");
        assert!(!record.global, "a component tool is not offered everywhere");
        assert_eq!(record.label, TOOL_LABEL);
    }
}
