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
//! transform each frame, so a test that moves the entity moves its handles
//! with it.

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
pub(super) fn register(world: &mut pill_engine_core::World) {
    tools::tool_registry_mut(world).register_plugin(
        &FixtureTool,
        tools::ToolElementHooks {
            kinds: Some(fixture_kinds),
            elements: Some(fixture_elements),
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

/// Fills `out` with the fixture's elements for `entity`.
///
/// An entity with no readable transform reports nothing, which is also the
/// state a suite sees when it has no entity selected.
fn fixture_elements(
    entity: Entity,
    world: &pill_engine_core::World,
    out: &mut Vec<tools::ToolElement>,
) {
    let Some(transform) = read_transform(world, entity) else {
        return;
    };
    let origin = transform.translation;
    let centre = (FIXTURE_POINT_COUNT as f32 - 1.0) * 0.5;
    for index in 0..FIXTURE_POINT_COUNT {
        out.push(tools::ToolElement {
            id: index as u64,
            kind: FIXTURE_POINT_KIND.to_string(),
            position: [
                origin[0] + (index as f32 - centre) * FIXTURE_ELEMENT_SPACING,
                origin[1],
                origin[2],
            ],
        });
    }
    for index in 0..FIXTURE_TANGENT_COUNT {
        out.push(tools::ToolElement {
            id: FIXTURE_TANGENT_ID_BASE + index as u64,
            kind: FIXTURE_TANGENT_KIND.to_string(),
            position: [
                origin[0]
                    + (index as f32 - (FIXTURE_TANGENT_COUNT as f32 - 1.0) * 0.5)
                        * FIXTURE_ELEMENT_SPACING,
                origin[1] + FIXTURE_TANGENT_LIFT,
                origin[2],
            ],
        });
    }
}
