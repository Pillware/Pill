//! The tool the editor suites drive the element framework with.
//!
//! # Responsibilities
//!
//! - Declare the fixture tool: a component tool that reports a fixed set of
//!   elements for the selected entity, of two kinds, so a suite can hover,
//!   click, extend and box-select them without an extension loaded.
//! - Register it while a scripted channel drives the run, and never in an
//!   editor a person opened.
//!
//! # Design
//!
//! Every stage of the tools framework needs a producer to be tested at
//! runtime, and the first real one - an extension's editor crate - arrives
//! later than the framework parts it exercises. The fixture stands in for it:
//! three `point` elements on a line through the entity's translation and two
//! `tangent` elements above it, at one world unit apart, with stable ids
//! (`0..=2` and `10..=11`).
//!
//! It is registered only when the scripted channel is on, like the channel
//! itself: it exists to be driven by a scripted test, and a person's editor
//! never sees it in the toolbox. The elements are reported from the entity's
//! transform plus an offset this module keeps per element, so a test that
//! moves the entity moves its handles with it, and a drag that writes new
//! world transforms back turns them into offsets again.

// Standard library
use std::collections::HashMap;

// External crates
use pill_engine_core::World;

// Current crate
use super::*;

// =============================================================================
// Constants
// =============================================================================

/// The name `tool fixture` activates, and the toolbox row would show.
pub(super) const FIXTURE_TOOL_NAME: &str = "fixture";

/// How many `point` elements the fixture reports.
pub(super) const FIXTURE_POINT_COUNT: usize = 3;

/// How many `tangent` elements the fixture reports.
pub(super) const FIXTURE_TANGENT_COUNT: usize = 2;

/// The first element id a `tangent` reports under. The points take
/// `0..FIXTURE_POINT_COUNT`, so the two ranges can never collide.
pub(super) const FIXTURE_TANGENT_ID_BASE: u64 = 10;

/// How far apart two neighbouring elements sit, in world units.
pub(super) const FIXTURE_ELEMENT_SPACING: f32 = 1.0;

/// How far above the line of points the tangents sit, in world units.
pub(super) const FIXTURE_TANGENT_LIFT: f32 = 1.0;

/// The kind the points report under.
pub(super) const FIXTURE_POINT_KIND: &str = "point";

/// The kind the tangents report under.
pub(super) const FIXTURE_TANGENT_KIND: &str = "tangent";

/// The name of the one coordinate frame the fixture offers.
///
/// It is a quarter turn around X, so the frame's Y axis is the world's Z: a
/// drag along that handle moves the elements along world Z, while the same
/// handle without the frame moves them along world Y. A suite can tell the two
/// apart by one number, and the frame's own grab points sit clear of every
/// element the fixture reports.
pub(super) const FIXTURE_FRAME_NAME: &str = "Fixture";

// =============================================================================
// Element Storage
// =============================================================================

/// One fixture element, as the fixture stores it.
///
/// The offset is what makes a drag stick: a move arrives as a world transform,
/// and the offset is that transform measured from the entity it belongs to, so
/// moving the entity afterwards still carries its elements.
#[derive(Clone, Copy, Debug)]
struct FixtureElement {
    /// Where the element sits, relative to the entity's translation.
    offset: [f32; 3],
    /// The element's own orientation.
    rotation: [f32; 4],
    /// The element's own scale.
    scale: [f32; 3],
}

/// The fixture's elements, per entity.
///
/// A world resource so the elements outlive a shell reload and ride the play
/// session: the entities they belong to do.
#[derive(Default)]
pub(super) struct FixtureElements {
    by_entity: HashMap<Entity, HashMap<u64, FixtureElement>>,
}

impl pill_engine_core::Resource for FixtureElements {}

impl FixtureElements {
    /// The elements of `entity`, seeded with the fixture's layout the first
    /// time a move writes into it, so an element a drag never touched keeps
    /// the place the layout put it in.
    fn entities_elements(&mut self, entity: Entity) -> &mut HashMap<u64, FixtureElement> {
        self.by_entity
            .entry(entity)
            .or_insert_with(|| fixture_layout().into_iter().collect())
    }
}

/// One element at `offset`, unrotated and unscaled.
fn fixture_element(offset: [f32; 3]) -> FixtureElement {
    FixtureElement {
        offset,
        rotation: tools::ElementTransform::IDENTITY_ROTATION,
        scale: tools::ElementTransform::UNIT_SCALE,
    }
}

/// The fixture's layout: every element at its default offset, in id order.
fn fixture_layout() -> Vec<(u64, FixtureElement)> {
    let mut elements: Vec<(u64, FixtureElement)> = (0..FIXTURE_POINT_COUNT)
        .map(|index| (index as u64, fixture_element(fixture_point_offset(index))))
        .collect();
    elements.extend((0..FIXTURE_TANGENT_COUNT).map(|index| {
        (
            FIXTURE_TANGENT_ID_BASE + index as u64,
            fixture_element(fixture_tangent_offset(index)),
        )
    }));
    elements
}

/// Where point `index` sits, relative to the entity's translation.
fn fixture_point_offset(index: usize) -> [f32; 3] {
    let centre = (FIXTURE_POINT_COUNT as f32 - 1.0) * 0.5;
    [(index as f32 - centre) * FIXTURE_ELEMENT_SPACING, 0.0, 0.0]
}

/// Where tangent `index` sits, relative to the entity's translation.
fn fixture_tangent_offset(index: usize) -> [f32; 3] {
    let centre = (FIXTURE_TANGENT_COUNT as f32 - 1.0) * 0.5;
    [
        (index as f32 - centre) * FIXTURE_ELEMENT_SPACING,
        FIXTURE_TANGENT_LIFT,
        0.0,
    ]
}

/// The rotation of the fixture's frame: a quarter turn around X.
pub(super) fn fixture_frame_rotation() -> [f32; 4] {
    gizmo::axis_angle_quaternion(gizmo::AXES[0], std::f32::consts::FRAC_PI_2)
}

// =============================================================================
// The Tool
// =============================================================================

/// The fixture tool itself: a component tool, offered only while it is
/// chosen, that reports elements for whichever entity is selected.
pub(super) struct FixtureTool;

impl tools::EditorTool for FixtureTool {
    fn name(&self) -> &'static str {
        FIXTURE_TOOL_NAME
    }

    fn label(&self) -> &'static str {
        "Fixture Points"
    }

    /// A component tool: it acts on the selected entity's elements, so it is
    /// not offered whatever the selection holds.
    fn is_global(&self) -> bool {
        false
    }
}

/// Registers the fixture tool, with its reporting hooks, in the world's
/// registry.
///
/// A reload of any shell module re-runs nothing here: the fixture lives in
/// this executable, which is never reloaded, and its hooks are the
/// executable's own functions.
pub(super) fn register(world: &mut World) {
    if world.get_resource::<FixtureElements>().is_none() {
        world.insert_resource(FixtureElements::default());
    }
    tools::tool_registry_mut(world).register_plugin(
        &FixtureTool,
        tools::ToolElementHooks {
            kinds: Some(fixture_kinds),
            elements: Some(fixture_elements),
            set_transforms: Some(fixture_set_transforms),
            frames: Some(fixture_frames),
        },
    );
}

/// The two kinds the fixture reports: selectable points and extendable
/// tangents.
///
/// The tangent kind allows multi-select but not box select, so a suite can
/// prove both that an extend works inside a kind and that a marquee never
/// takes a kind which forbids it.
fn fixture_kinds(out: &mut Vec<tools::ElementKind>) {
    let mut point = tools::ElementKind::new(FIXTURE_POINT_KIND);
    point.color = [0.35, 0.65, 1.0, 1.0];
    let mut tangent = tools::ElementKind::new(FIXTURE_TANGENT_KIND);
    tangent.color = [0.40, 0.90, 0.50, 1.0];
    tangent.box_select = false;
    out.push(point);
    out.push(tangent);
}

/// The fixture's elements for `entity`, each placed at its stored offset from
/// the entity's translation.
///
/// An entity with no readable transform reports nothing, which is also the
/// state a suite sees when it has no entity selected.
fn fixture_elements(entity: Entity, world: &World, out: &mut Vec<tools::ToolElement>) {
    let Some(transform) = read_transform(world, entity) else {
        return;
    };
    // A written layout wins; until the first move, the default one is
    // reported, so a suite can hover and click before it drags anything.
    let stored = world
        .get_resource::<FixtureElements>()
        .and_then(|storage| storage.by_entity.get(&entity));
    let laid_out: Vec<(u64, FixtureElement)> = match stored {
        Some(elements) => {
            let mut ids: Vec<u64> = elements.keys().copied().collect();
            ids.sort_unstable();
            ids.into_iter()
                .filter_map(|id| elements.get(&id).map(|element| (id, *element)))
                .collect()
        }
        None => fixture_layout(),
    };
    for (id, element) in laid_out {
        out.push(tools::ToolElement {
            id,
            kind: fixture_kind_of(id).to_string(),
            transform: tools::ElementTransform {
                translation: gizmo::add(transform.translation, element.offset),
                rotation: element.rotation,
                scale: element.scale,
            },
        });
    }
}

/// The kind the element `id` belongs to.
fn fixture_kind_of(id: u64) -> &'static str {
    if id >= FIXTURE_TANGENT_ID_BASE {
        FIXTURE_TANGENT_KIND
    } else {
        FIXTURE_POINT_KIND
    }
}

/// Writes the world transforms a drag produced back into the fixture's
/// storage, as offsets from the entity they belong to.
///
/// This is the fixture's whole write-back: the editor hands it finished world
/// transforms and it decides what that means for its own component, which is
/// the contract every tool keeps.
fn fixture_set_transforms(entity: Entity, moves: &[tools::ElementMove], world: &mut World) {
    let Some(transform) = read_transform(world, entity) else {
        return;
    };
    if world.get_resource::<FixtureElements>().is_none() {
        world.insert_resource(FixtureElements::default());
    }
    let Some(storage) = world.get_resource_mut::<FixtureElements>() else {
        return;
    };
    let elements = storage.entities_elements(entity);
    for movement in moves {
        let Some(element) = elements.get_mut(&movement.element) else {
            continue;
        };
        element.offset = gizmo::sub(movement.transform.translation, transform.translation);
        element.rotation = movement.transform.rotation;
        element.scale = movement.transform.scale;
    }
}

/// The one frame the fixture offers, so a suite can prove the space row
/// reaches a tool's own frame and that a drag in it produces the world delta
/// the frame implies.
fn fixture_frames(
    _entity: Entity,
    _selected: &[u64],
    _world: &World,
    out: &mut Vec<tools::CoordinateFrame>,
) {
    out.push(tools::CoordinateFrame {
        name: FIXTURE_FRAME_NAME.to_string(),
        rotation: fixture_frame_rotation(),
    });
}
