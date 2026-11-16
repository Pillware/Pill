//! The semi-inheritance proving case: an extension of the spline tool.
//!
//! # Responsibilities
//!
//! - Declares [`UberSplineExtras`], the component an entity needs for the
//!   extension to be attached to it.
//! - Declares the `roll` kind, its handles and the menu row the spline tool
//!   gains while that component is present, without replacing, renaming or
//!   removing anything the spline tool itself offers.
//!
//! # Design
//!
//! This is composition, not inheritance: an entity carries a plain
//! [`Spline`](pill_spline::Spline) plus [`UberSplineExtras`], and a spline
//! that never met this crate keeps working exactly as it did. The extension
//! names the tool it extends by the name a command spells, says which kind its
//! additions apply to and which component attaches it, and states the row
//! order it takes against the tool's other extensions. The registry does the
//! rest: while the entity has no extras the extension is not attached, so
//! neither its kind nor its rows exist for that entity.
//!
//! The crate is an editor-only extension - the folder holds no runtime crate -
//! so an editor host loads it and no game build ever does. Its component is
//! therefore editor-side data, which is exactly what a per-point roll is.

// External crates
use pill_core::math::Vector3f;
use pill_editor_core::editor_state::EditorCommand;
use pill_editor_core::tools::{
    self, ElementKind, ElementMenuEntry, ElementTransform, ToolElement, ToolExtension,
    ToolExtensionHooks, ToolRegistry,
};
use pill_engine::{
    pill_module, Engine, Entity, FieldValue, PillComponent, TransformComponent, World,
};
use pill_spline::{Spline, MAX_CONTROL_POINTS};
use serde::{Deserialize, Serialize};

// The build script scans this crate and emits one address entry per function
// into `function_inventory.rs`.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Constants
// =============================================================================

/// The extension's own name, as the tool dump and the logs spell it.
pub const EXTENSION_NAME: &str = "uber spline rolls";

/// The kind the roll handles are reported under.
pub const ROLL_KIND: &str = "spline roll";

/// The first element id a roll handle uses.
pub const ROLL_ID_BASE: u64 = 200;

/// The menu row that puts every selected point's roll back to zero.
///
/// The id is one word because the scripted channel names a row with one; the
/// label is what a reader sees.
pub const MENU_RESET_ROLL: &str = "reset-roll";

/// How far above its control point a roll handle sits, in spline units.
const ROLL_HANDLE_LIFT: f32 = 2.0;

/// How wide a roll handle draws, in pixels.
const ROLL_SIZE_PIXELS: f32 = 6.0;

/// The colour a roll handle draws in.
const ROLL_COLOR: [f32; 4] = [0.85, 0.45, 0.95, 1.0];

/// The tool this extension extends, by the name a command spells.
const BASE_TOOL: &str = "spline";

/// The kind of the base tool the extension's additions apply to.
const BASE_KIND: &str = "control point";

// =============================================================================
// Component
// =============================================================================

/// What an Uber spline carries beside its spline: one roll per control point.
///
/// The component exists so the extension has something to be attached by. A
/// plain spline entity has no extras, which is what keeps this crate's kind and
/// menu rows out of a plain spline's tool.
#[repr(C)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PillComponent)]
#[pill(persistable, shared)]
pub struct UberSplineExtras {
    /// The roll of each control point, in degrees.
    pub rolls: [f32; MAX_CONTROL_POINTS],
}

impl Default for UberSplineExtras {
    fn default() -> Self {
        Self {
            rolls: [0.0; MAX_CONTROL_POINTS],
        }
    }
}

// =============================================================================
// The Extension
// =============================================================================

/// The extension itself: a roll handle per control point and one menu row.
pub struct UberSplineRollsExtension;

impl ToolExtension for UberSplineRollsExtension {
    fn name() -> &'static str {
        EXTENSION_NAME
    }

    fn extends_tool() -> &'static str {
        BASE_TOOL
    }

    /// Roll handles belong to the points, not to the tangent arms.
    fn for_kinds() -> &'static [&'static str] {
        &[BASE_KIND]
    }

    /// Attached only to an entity that carries the extras, which is what makes
    /// the addition opt-in rather than a change to every spline.
    fn requires_component() -> Option<&'static str> {
        Some("UberSplineExtras")
    }

    /// After anything the base extension does, so its rows read below theirs.
    fn order() -> i32 {
        100
    }

    fn hooks() -> ToolExtensionHooks {
        ToolExtensionHooks {
            kinds: Some(roll_kinds),
            elements: Some(roll_elements),
            menu: Some(roll_menu),
            menu_run: Some(roll_menu_run),
            ..ToolExtensionHooks::default()
        }
    }
}

/// Registers the extension into `registry`.
pub fn register_into(registry: &mut ToolRegistry) {
    registry.register_extension::<UberSplineRollsExtension>();
}

/// Registers the extension against a host engine, on every generation.
///
/// A reload of either crate reaches this again, which is what re-attaches the
/// extension to the tool the base crate owns.
///
/// # Errors
///
/// Returns 0 always: the extension registers no world state that can fail.
pub fn register(engine: &mut Engine) -> u32 {
    register_into(tools::tool_registry_mut(engine.world_mut()));
    0
}

/// The artifact's one-time entry point, which defers to [`register`]: the
/// component is registered by the artifact entry point, and the extension maps
/// to no one-time work.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

/// Registers [`UberSplineExtras`] with a world, field layout included, for a
/// crate that links the type without running the module entry point.
pub fn register_component(world: &mut World) {
    __pill_register_UberSplineExtras(world);
}

// =============================================================================
// Reporting
// =============================================================================

/// Adds the roll kind to the base tool's kinds.
fn roll_kinds(out: &mut Vec<ElementKind>) {
    let mut roll = ElementKind::new(ROLL_KIND);
    roll.size_pixels = ROLL_SIZE_PIXELS;
    roll.color = ROLL_COLOR;
    roll.box_select = false;
    out.push(roll);
}

/// One handle per control point, lifted off the curve so it never sits on the
/// point it belongs to.
fn roll_elements(entity: Entity, world: &World, out: &mut Vec<ToolElement>) {
    let (Some(spline), Some(extras)) = (
        world.get_component::<Spline>(entity),
        world.get_component::<UberSplineExtras>(entity),
    ) else {
        return;
    };
    let translation = world
        .get_component::<TransformComponent>(entity)
        .map_or([0.0; 3], |transform| transform.translation);
    for (index, point) in spline.control_points().iter().enumerate() {
        let lifted = Vector3f::new(point.x, point.y + ROLL_HANDLE_LIFT, point.z);
        out.push(ToolElement {
            id: ROLL_ID_BASE + index as u64,
            kind: ROLL_KIND.to_string(),
            transform: ElementTransform::at([
                translation[0] + lifted.x,
                translation[1] + lifted.y,
                translation[2] + lifted.z,
            ]),
        });
        let _ = extras.rolls[index];
    }
}

/// The one row the extension adds.
fn roll_menu(
    _entity: Entity,
    kind: &str,
    selected: &[u64],
    _world: &World,
    out: &mut Vec<ElementMenuEntry>,
) {
    if kind != BASE_KIND || selected.is_empty() {
        return;
    }
    out.push(ElementMenuEntry {
        id: MENU_RESET_ROLL.to_string(),
        label: "Reset roll".to_string(),
    });
}

/// The row's action: every selected point's roll goes back to zero, as one
/// recorded array write per point.
fn roll_menu_run(
    entity: Entity,
    kind: &str,
    selected: &[u64],
    item: &str,
    world: &World,
) -> Result<Vec<EditorCommand>, String> {
    if kind != BASE_KIND || item != MENU_RESET_ROLL {
        return Err(format!(
            "the uber spline adds no '{item}' row for {kind} elements"
        ));
    }
    let Some(extras) = world.get_component::<UberSplineExtras>(entity) else {
        return Err("the entity carries no uber spline extras".to_string());
    };
    let points: Vec<usize> = selected
        .iter()
        .copied()
        .filter_map(|id| (id < ROLL_ID_BASE).then_some(id as usize))
        .filter(|index| extras.rolls[*index] != 0.0)
        .collect();
    if points.is_empty() {
        return Err("no selected point carries a roll".to_string());
    }
    Ok(points
        .into_iter()
        .map(|index| EditorCommand::SetArrayElement {
            entity,
            component: "UberSplineExtras".to_string(),
            field: "rolls".to_string(),
            index,
            value: FieldValue::F32(0.0),
        })
        .collect())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A world with one spline entity, carrying the extras when asked.
    fn world_with_spline(with_extras: bool) -> (World, Entity) {
        let mut world = World::default();
        pill_engine::register_transform_component(&mut world);
        pill_spline::register_component(&mut world);
        register_component(&mut world);
        let mut builder = world.create_entity().with(Spline::from_points(&[
            Vector3f::new(0.0, 0.0, 0.0),
            Vector3f::new(10.0, 0.0, 0.0),
            Vector3f::new(20.0, 5.0, 0.0),
        ]));
        builder = builder.with(TransformComponent::default());
        if with_extras {
            builder = builder.with(UberSplineExtras::default());
        }
        let entity = builder.build().expect("the spline entity builds");
        (world, entity)
    }

    /// A registry with the real spline tool and this extension, the tool
    /// active: the cross-crate chain is what these tests are about.
    fn active_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::default();
        pill_spline_editor::register_into(&mut registry);
        register_into(&mut registry);
        registry
            .set_active(Some(BASE_TOOL))
            .expect("the base tool is registered");
        registry
    }

    /// The extension's kind and row exist on an entity that carries the extras
    /// and on no other.
    #[test]
    fn the_extension_is_attached_only_by_its_component() {
        let (with_world, with_entity) = world_with_spline(true);
        let (without_world, without_entity) = world_with_spline(false);
        let registry = active_registry();

        let with_kinds: Vec<String> = registry
            .active_kinds(with_entity, &with_world)
            .into_iter()
            .map(|kind| kind.name)
            .collect();
        assert_eq!(
            with_kinds.last().map(String::as_str),
            Some(ROLL_KIND),
            "the extension's kind is added after the base's own: {with_kinds:?}"
        );
        let with_elements = registry.active_elements(with_entity, &with_world);
        assert_eq!(
            with_elements
                .iter()
                .filter(|element| element.kind == ROLL_KIND)
                .count(),
            3,
            "one roll handle per control point"
        );
        let with_menu: Vec<String> = registry
            .active_menu(with_entity, BASE_KIND, &[0], &with_world)
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert_eq!(
            with_menu.last().map(String::as_str),
            Some(MENU_RESET_ROLL),
            "the extension's row reads after the base tool's own: {with_menu:?}"
        );
        assert_eq!(
            with_menu
                .iter()
                .filter(|row| *row == MENU_RESET_ROLL)
                .count(),
            1,
            "the extension's row is added once"
        );

        let without_kinds: Vec<String> = registry
            .active_kinds(without_entity, &without_world)
            .into_iter()
            .map(|kind| kind.name)
            .collect();
        assert!(
            !without_kinds.iter().any(|kind| kind == ROLL_KIND),
            "a plain spline gained the extension's kind: {without_kinds:?}"
        );
        assert!(
            registry
                .active_elements(without_entity, &without_world)
                .iter()
                .all(|element| element.kind != ROLL_KIND),
            "a plain spline gained a roll handle"
        );
        assert!(
            registry
                .active_menu(without_entity, BASE_KIND, &[0], &without_world)
                .iter()
                .all(|row| row.id != MENU_RESET_ROLL),
            "a plain spline gained the extension's row"
        );
    }

    /// The row answers one recorded array write per rolled point.
    #[test]
    fn the_reset_row_answers_recorded_array_writes() {
        let (mut world, entity) = world_with_spline(true);
        let extras = world
            .get_component_mut::<UberSplineExtras>(entity)
            .expect("the extras");
        extras.rolls[0] = 45.0;
        extras.rolls[2] = 90.0;
        let registry = active_registry();

        let commands = registry
            .active_menu_run(entity, BASE_KIND, &[0, 1, 2], MENU_RESET_ROLL, &world)
            .expect("the reset row runs");
        assert_eq!(commands.len(), 2, "only the rolled points are written");
        assert!(
            commands.iter().all(|command| matches!(
                command,
                EditorCommand::SetArrayElement { field, .. } if field == "rolls"
            )),
            "the row answered {commands:?}"
        );
    }
}
