//! The physics extension's editor part: the tool that edits a collider's shape.
//!
//! # Responsibilities
//!
//! - Declares the `collider` tool: one kind per handle family (a cuboid's six
//!   faces, a ball's radius, a capsule's or cylinder's radius and half height)
//!   and the elements each shape offers.
//! - Writes a drag back into the collider's `dimensions`, so the simulation
//!   moves with the shape the reader dragged.
//! - Registers itself into the tool registry on every generation.
//!
//! # Design
//!
//! In the style of Unity's "Edit Collider", the shape is edited by dragging the
//! handles drawn on it rather than by typing numbers: six face handles for a
//! cuboid, one radius handle for a ball, and a radius plus a half-height handle
//! for the two upright shapes. Every handle sits on the collider's own surface,
//! so what is dragged is what changes.
//!
//! A drag arrives as the world transform the editor's gizmo produced. The tool
//! measures that place back into the collider's axes and writes the one number
//! the handle stands for: a face handle writes the half extent along its axis, a
//! radius handle writes a radius, and a height handle writes a half height. The
//! write goes into the component, and the simulation's own field-by-field sync
//! picks it up on the next step - no extra code, and no second copy of the
//! shape to keep in step.
//!
//! The editor records the drag as one command, so one undo puts the collider
//! back the way it was.
//!
//! Only the handles the current shape actually has are reported: a ball offers
//! its radius and nothing else, which is what stops a reader dragging a face
//! that does not exist.

// External crates
use pill_core::math::Vector3f;
use pill_editor_core::editor_state::SelectionClick;
use pill_editor_core::tools::{
    self, EditorTool, ElementKind, ElementMove, ElementTransform, PressResponse, ToolElement,
    ToolElementHooks, ToolRegistry,
};
use pill_engine::{pill_module, Engine, Entity, TransformComponent, World};
use pill_physics::{ColliderComponent, ColliderShape};

// The build script scans this crate and emits one address entry per function
// into `function_inventory.rs`.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Constants
// =============================================================================

/// The name `tool collider` activates and the toolbox row shows.
pub const TOOL_NAME: &str = "collider";

/// The label the toolbox row carries.
pub const TOOL_LABEL: &str = "Collider";

/// The kind a cuboid's face handles are reported under.
pub const FACE_KIND: &str = "collider face";

/// The kind a radius handle is reported under.
pub const RADIUS_KIND: &str = "collider radius";

/// The kind a half-height handle is reported under.
pub const HEIGHT_KIND: &str = "collider height";

/// The first element id a face handle uses: `0` to `5`, one per axis side.
pub const FACE_ID_BASE: u64 = 0;

/// The element id a radius handle uses.
pub const RADIUS_ID: u64 = 10;

/// The element id a half-height handle uses.
pub const HEIGHT_ID: u64 = 11;

/// The smallest measurement a drag may leave a shape with.
///
/// A shape with a zero or negative extent is not a shape the simulation can
/// build, so a handle dragged past its collider's centre stops here instead of
/// inverting the collider.
const MINIMUM_EXTENT: f32 = 0.01;

/// How wide a face handle draws, in pixels.
const FACE_SIZE_PIXELS: f32 = 9.0;

/// How wide a radius or height handle draws, in pixels.
const DIMENSION_SIZE_PIXELS: f32 = 8.0;

/// The colour a face handle draws in.
const FACE_COLOR: [f32; 4] = [0.35, 0.95, 0.55, 1.0];

/// The colour a radius or height handle draws in.
const DIMENSION_COLOR: [f32; 4] = [0.95, 0.75, 0.2, 1.0];

/// The axes the shape maths reads, in the order the three slots of
/// [`ColliderComponent::dimensions`] use them.
const AXES: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

// =============================================================================
// The Tool
// =============================================================================

/// The collider tool itself: a component tool over one entity's collider.
pub struct ColliderTool;

impl EditorTool for ColliderTool {
    fn name(&self) -> &'static str {
        TOOL_NAME
    }

    fn label(&self) -> &'static str {
        TOOL_LABEL
    }

    /// A component tool: it edits the selected entity's collider, so it is not
    /// offered whatever the selection holds.
    fn is_global(&self) -> bool {
        false
    }

    /// Offered for an entity that carries a [`ColliderComponent`].
    fn requires_component(&self) -> Option<&'static str> {
        Some("ColliderComponent")
    }
}

/// Registers the tool and its element hooks into `registry`.
pub fn register_into(registry: &mut ToolRegistry) {
    registry.register_plugin(
        &ColliderTool,
        ToolElementHooks {
            kinds: Some(collider_kinds),
            elements: Some(collider_elements),
            set_transforms: Some(collider_set_transforms),
            press: Some(collider_press),
            ..ToolElementHooks::default()
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

/// The artifact's one-time entry point, which defers to [`register`].
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

// =============================================================================
// Kinds
// =============================================================================

/// The three handle families the tool exposes.
///
/// All three are reported whatever the shape is; which of them the selection
/// actually has is what [`collider_elements`] decides.
fn collider_kinds(out: &mut Vec<ElementKind>) {
    let mut face = ElementKind::new(FACE_KIND);
    face.size_pixels = FACE_SIZE_PIXELS;
    face.color = FACE_COLOR;
    // A box over face handles would select whichever ones it covered rather
    // than the faces the reader meant to bring in.
    face.box_select = false;
    out.push(face);

    let mut radius = ElementKind::new(RADIUS_KIND);
    radius.size_pixels = DIMENSION_SIZE_PIXELS;
    radius.color = DIMENSION_COLOR;
    radius.box_select = false;
    out.push(radius);

    let mut height = ElementKind::new(HEIGHT_KIND);
    height.size_pixels = DIMENSION_SIZE_PIXELS;
    height.color = DIMENSION_COLOR;
    height.box_select = false;
    out.push(height);
}

// =============================================================================
// Elements
// =============================================================================

/// The handles the entity's collider offers, at the places the shape puts them.
///
/// Each handle sits on the surface it edits: a face handle at the face's centre,
/// a radius handle one radius out along `+X`, a height handle one half height up
/// along `+Y`. The shape's own offset and the entity's rotation place all of
/// them, so a turned collider's handles turn with it.
fn collider_elements(entity: Entity, world: &World, out: &mut Vec<ToolElement>) {
    let (Some(collider), Some(transform)) = (
        world.get_component::<ColliderComponent>(entity),
        world.get_component::<TransformComponent>(entity),
    ) else {
        return;
    };
    let centre = pill_physics::manager::collider_center(transform, collider);
    let [first, second, third] = collider.dimensions;
    match collider.shape {
        ColliderShape::Cuboid => {
            for (axis, extent) in [first, second, third].into_iter().enumerate() {
                for (side, sign) in [-1.0_f32, 1.0].into_iter().enumerate() {
                    let local = Vector3f::new(
                        AXES[axis][0] * extent * sign,
                        AXES[axis][1] * extent * sign,
                        AXES[axis][2] * extent * sign,
                    );
                    out.push(ToolElement {
                        id: FACE_ID_BASE + (axis * 2 + side) as u64,
                        kind: FACE_KIND.to_string(),
                        transform: ElementTransform::at(place(transform, centre, local)),
                    });
                }
            }
        }
        ColliderShape::Ball => {
            out.push(radius_handle(transform, centre, first));
        }
        ColliderShape::Capsule | ColliderShape::Cylinder => {
            out.push(radius_handle(transform, centre, second));
            out.push(ToolElement {
                id: HEIGHT_ID,
                kind: HEIGHT_KIND.to_string(),
                transform: ElementTransform::at(place(
                    transform,
                    centre,
                    Vector3f::new(0.0, first, 0.0),
                )),
            });
        }
    }
}

/// The one radius handle a round shape offers, `radius` out along `+X`.
fn radius_handle(transform: &TransformComponent, centre: [f32; 3], radius: f32) -> ToolElement {
    ToolElement {
        id: RADIUS_ID,
        kind: RADIUS_KIND.to_string(),
        transform: ElementTransform::at(place(transform, centre, Vector3f::new(radius, 0.0, 0.0))),
    }
}

/// A point `local` away from the collider's centre, turned with the entity.
fn place(transform: &TransformComponent, centre: [f32; 3], local: Vector3f) -> [f32; 3] {
    let turned = rotate(transform.rotation, local);
    [
        centre[0] + turned.x,
        centre[1] + turned.y,
        centre[2] + turned.z,
    ]
}

// =============================================================================
// Writing edits back
// =============================================================================

/// Writes a drag's world places into the collider's measurements.
///
/// Each handle writes exactly one number: a face handle's distance from the
/// centre along its own axis, a radius handle's distance from the centre, a
/// height handle's height above the centre. A drag past the centre stops at
/// [`MINIMUM_EXTENT`], so the shape always stays a shape.
fn collider_set_transforms(entity: Entity, moves: &[ElementMove], world: &mut World) {
    let Some(transform) = world.get_component::<TransformComponent>(entity).copied() else {
        return;
    };
    let Some(collider) = world.get_component::<ColliderComponent>(entity).copied() else {
        return;
    };
    let centre = Vector3f::from_array(pill_physics::manager::collider_center(
        &transform, &collider,
    ));
    let inverse = [
        -transform.rotation[0],
        -transform.rotation[1],
        -transform.rotation[2],
        transform.rotation[3],
    ];
    let Some(live) = world.get_component_mut::<ColliderComponent>(entity) else {
        return;
    };
    for moved in moves {
        let offset = rotate(
            inverse,
            Vector3f::from_array(moved.transform.translation) - centre,
        );
        match moved.element {
            RADIUS_ID => {
                let axis = match live.shape {
                    ColliderShape::Ball => 0,
                    ColliderShape::Capsule | ColliderShape::Cylinder => 1,
                    // A cuboid has no radius handle; a stray id changes nothing.
                    ColliderShape::Cuboid => continue,
                };
                let radius = offset.length().max(MINIMUM_EXTENT);
                live.dimensions[axis] = radius;
            }
            HEIGHT_ID => {
                live.dimensions[0] = offset.y.abs().max(MINIMUM_EXTENT);
            }
            face => {
                let axis = (face as usize) / 2;
                if axis >= AXES.len() {
                    continue;
                }
                // The even id of a pair is the minus face, the odd one the plus
                // face, so each measures its own side of the centre: a plus face
                // dragged past the centre stops at the minimum instead of
                // flipping to the mirrored extent.
                let side = if face % 2 == 0 { -1.0 } else { 1.0 };
                let half_extent =
                    (offset.dot(Vector3f::from_array(AXES[axis])) * side).max(MINIMUM_EXTENT);
                live.dimensions[axis] = half_extent;
            }
        }
    }
}

/// Answers a press on one of the tool's handles.
///
/// The editor's own rule is what a collider wants: a press selects the handle
/// and starts a drag.
fn collider_press(
    _entity: Entity,
    _element: u64,
    _click: SelectionClick,
    _selected: &[u64],
    _world: &World,
) -> PressResponse {
    PressResponse::Default
}

// =============================================================================
// Rotation maths
// =============================================================================

/// Turns `vector` by the unit quaternion `rotation`, `x y z w` order.
fn rotate(rotation: [f32; 4], vector: Vector3f) -> Vector3f {
    let [x, y, z, w] = rotation;
    let axis = Vector3f::new(x, y, z);
    let twice_cross = axis.cross(vector) * 2.0;
    vector + twice_cross * w + axis.cross(twice_cross)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for comparing places and measurements.
    const EPSILON: f32 = 1e-4;

    /// A world with one entity carrying a collider and a transform.
    fn world_with_collider(shape: ColliderShape, dimensions: [f32; 3]) -> (World, Entity) {
        let mut world = World::default();
        pill_engine::register_transform_component(&mut world);
        pill_physics::components::register(&mut world);
        let entity = world
            .create_entity()
            .with(ColliderComponent {
                shape,
                dimensions,
                ..ColliderComponent::default()
            })
            .with(TransformComponent::default())
            .build()
            .expect("the collider entity builds");
        (world, entity)
    }

    /// A registry with the collider tool registered and active.
    fn active_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::default();
        register_into(&mut registry);
        registry
            .set_active(Some(TOOL_NAME))
            .expect("the collider tool is registered");
        registry
    }

    /// A cuboid offers six face handles, each on the face it edits.
    #[test]
    fn a_cuboid_reports_six_face_handles() {
        let (world, entity) = world_with_collider(ColliderShape::Cuboid, [2.0, 3.0, 4.0]);
        let registry = active_registry();

        let elements = registry.active_elements(entity, &world);
        assert_eq!(elements.len(), 6, "a cuboid has six faces");
        assert!(elements.iter().all(|element| element.kind == FACE_KIND));
        // The `+X` face sits two units out along x; the `-Z` face four units
        // back along z. An even id is the minus face of its pair.
        let plus_x = elements
            .iter()
            .find(|element| element.id == FACE_ID_BASE + 1)
            .expect("the +X face");
        assert!(
            plus_x
                .transform
                .translation
                .iter()
                .copied()
                .zip([2.0, 0.0, 0.0])
                .all(|(got, want)| (got - want).abs() < EPSILON),
            "the +X face sits at {:?}",
            plus_x.transform.translation
        );
        let minus_z = elements
            .iter()
            .find(|element| element.id == FACE_ID_BASE + 4)
            .expect("the -Z face");
        assert!(
            minus_z
                .transform
                .translation
                .iter()
                .copied()
                .zip([0.0, 0.0, -4.0])
                .all(|(got, want)| (got - want).abs() < EPSILON),
            "the -Z face sits at {:?}",
            minus_z.transform.translation
        );
    }

    /// A ball offers its radius handle and nothing else.
    #[test]
    fn a_ball_reports_only_its_radius_handle() {
        let (world, entity) = world_with_collider(ColliderShape::Ball, [1.5, 0.0, 0.0]);
        let registry = active_registry();

        let elements = registry.active_elements(entity, &world);
        assert_eq!(elements.len(), 1, "a ball has one handle");
        assert_eq!(elements[0].kind, RADIUS_KIND);
        assert_eq!(elements[0].id, RADIUS_ID);
        assert!((elements[0].transform.translation[0] - 1.5).abs() < EPSILON);
    }

    /// The two upright shapes offer a radius and a half-height handle.
    #[test]
    fn an_upright_shape_reports_radius_and_height() {
        for shape in [ColliderShape::Capsule, ColliderShape::Cylinder] {
            let (world, entity) = world_with_collider(shape, [2.0, 0.75, 0.0]);
            let registry = active_registry();
            let elements = registry.active_elements(entity, &world);
            let kinds: Vec<&str> = elements
                .iter()
                .map(|element| element.kind.as_str())
                .collect();
            assert_eq!(kinds, [RADIUS_KIND, HEIGHT_KIND], "shape {shape:?}");
            let height = elements
                .iter()
                .find(|element| element.id == HEIGHT_ID)
                .expect("the height handle");
            assert!(
                (height.transform.translation[1] - 2.0).abs() < EPSILON,
                "the height handle sits at {:?}",
                height.transform.translation
            );
        }
    }

    /// Dragging a face handle writes the half extent along its axis, and only
    /// that axis.
    #[test]
    fn a_face_drag_writes_one_half_extent() {
        let (mut world, entity) = world_with_collider(ColliderShape::Cuboid, [2.0, 3.0, 4.0]);
        let registry = active_registry();

        let moved = vec![ElementMove {
            element: FACE_ID_BASE + 1,
            transform: ElementTransform::at([5.0, 0.0, 0.0]),
        }];
        assert!(
            registry.active_set_transforms(entity, &moved, &mut world),
            "the tool writes transforms back"
        );

        let collider = world
            .get_component::<ColliderComponent>(entity)
            .expect("the collider");
        assert!((collider.dimensions[0] - 5.0).abs() < EPSILON);
        assert!((collider.dimensions[1] - 3.0).abs() < EPSILON);
        assert!((collider.dimensions[2] - 4.0).abs() < EPSILON);
    }

    /// A drag past the centre keeps the shape valid instead of inverting it.
    #[test]
    fn a_face_drag_past_the_centre_stops_at_the_minimum() {
        let (mut world, entity) = world_with_collider(ColliderShape::Cuboid, [2.0, 3.0, 4.0]);
        let registry = active_registry();
        // The `+X` face dragged to the other side of the centre: its own side
        // of the shape is what it measures, so it stops at the minimum.
        let moved = vec![ElementMove {
            element: FACE_ID_BASE + 1,
            transform: ElementTransform::at([-0.5, 0.0, 0.0]),
        }];
        registry.active_set_transforms(entity, &moved, &mut world);
        let collider = world
            .get_component::<ColliderComponent>(entity)
            .expect("the collider");
        assert_eq!(collider.dimensions[0], MINIMUM_EXTENT);
    }

    /// A radius drag writes the radius, on the slot the shape reads it from.
    #[test]
    fn a_radius_drag_writes_the_radius() {
        let (mut world, entity) = world_with_collider(ColliderShape::Ball, [1.0, 0.0, 0.0]);
        let registry = active_registry();
        let moved = vec![ElementMove {
            element: RADIUS_ID,
            transform: ElementTransform::at([0.0, 3.0, 0.0]),
        }];
        registry.active_set_transforms(entity, &moved, &mut world);
        let collider = world
            .get_component::<ColliderComponent>(entity)
            .expect("the collider");
        assert!((collider.dimensions[0] - 3.0).abs() < EPSILON);

        // A capsule reads its radius from the second slot, so the same handle
        // leaves the first one - its half height - alone.
        let (mut capsule_world, capsule) =
            world_with_collider(ColliderShape::Capsule, [2.0, 0.5, 0.0]);
        let moved = vec![ElementMove {
            element: RADIUS_ID,
            transform: ElementTransform::at([0.0, 0.0, 1.25]),
        }];
        registry.active_set_transforms(capsule, &moved, &mut capsule_world);
        let collider = capsule_world
            .get_component::<ColliderComponent>(capsule)
            .expect("the collider");
        assert!((collider.dimensions[1] - 1.25).abs() < EPSILON);
        assert!((collider.dimensions[0] - 2.0).abs() < EPSILON);
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
    }
}
