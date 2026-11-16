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

/// The menu row the notes extension adds for `point` elements.
pub(super) const FIXTURE_MENU_NOTE: &str = "note";

/// The menu row the marks extension adds for every kind.
pub(super) const FIXTURE_MENU_MARK: &str = "mark";

/// The chord both fixture extensions bind.
///
/// Two extensions of one base on one chord is the clash a reader is told about
/// once: the lower `order` answers the key and the log names it the winner.
pub(super) const FIXTURE_EXTENSION_CHORD: &str = "n";

/// The component the notes extension is attached by.
///
/// The scene project's rotating models carry it and the camera does not, which
/// is what lets a suite show the same row appearing on one entity and not on
/// the other.
pub(super) const FIXTURE_NOTES_COMPONENT: &str = "TagAlphaComponent";

/// The notes extension's name, as `dump tools` and the conflict line spell it.
pub(super) const FIXTURE_NOTES_EXTENSION: &str = "fixture notes";

/// The marks extension's name.
pub(super) const FIXTURE_MARKS_EXTENSION: &str = "fixture marks";

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
///
/// The two extensions are registered by [`set_extensions`] instead, so every
/// other suite area drives the plain tool: an extension's extra menu rows would
/// otherwise change what those areas read.
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
            menu: Some(fixture_menu),
            menu_run: Some(fixture_menu_run),
            insert: Some(fixture_insert),
            remove: Some(fixture_remove),
            press: Some(fixture_press),
            key: Some(fixture_key),
            selection_changed: Some(fixture_selection_changed),
        },
    );
}

/// Registers or drops the fixture's two extensions.
///
/// The component-tools steps switch them on: until then the fixture tool is
/// exactly the tool every other area drives, with two menu rows and no chords,
/// so an area that reads the element menu elsewhere sees what it always saw.
pub(super) fn set_extensions(world: &mut World, enabled: bool) {
    let registry = tools::tool_registry_mut(world);
    registry.clear_extensions();
    if !enabled {
        return;
    }
    registry.register_extension::<FixtureNotesExtension>();
    registry.register_extension::<FixtureMarksExtension>();
}

// =============================================================================
// Extensions
// =============================================================================

/// The extension that adds a row only for `point` elements, and only on an
/// entity carrying [`FIXTURE_NOTES_COMPONENT`].
pub(super) struct FixtureNotesExtension;

impl tools::ToolExtension for FixtureNotesExtension {
    fn name() -> &'static str {
        FIXTURE_NOTES_EXTENSION
    }

    fn extends_tool() -> &'static str {
        FIXTURE_TOOL_NAME
    }

    /// Its row belongs to the point kind alone, so a tangent selection never
    /// shows it.
    fn for_kinds() -> &'static [&'static str] {
        &[FIXTURE_POINT_KIND]
    }

    fn requires_component() -> Option<&'static str> {
        Some(FIXTURE_NOTES_COMPONENT)
    }

    /// First in the chain, which is what makes it the winner of the chord both
    /// extensions bind.
    fn order() -> i32 {
        10
    }

    fn keys() -> &'static [&'static str] {
        &[FIXTURE_EXTENSION_CHORD]
    }

    fn hooks() -> tools::ToolExtensionHooks {
        tools::ToolExtensionHooks {
            menu: Some(notes_menu),
            menu_run: Some(notes_menu_run),
            key: Some(extension_key),
            ..tools::ToolExtensionHooks::default()
        }
    }
}

/// The extension that adds a row for every kind, on every entity.
pub(super) struct FixtureMarksExtension;

impl tools::ToolExtension for FixtureMarksExtension {
    fn name() -> &'static str {
        FIXTURE_MARKS_EXTENSION
    }

    fn extends_tool() -> &'static str {
        FIXTURE_TOOL_NAME
    }

    /// Second in the chain: it binds the same chord as the notes extension and
    /// loses it, which is the order rule a suite reads off the conflict line.
    fn order() -> i32 {
        30
    }

    fn keys() -> &'static [&'static str] {
        &[FIXTURE_EXTENSION_CHORD]
    }

    fn hooks() -> tools::ToolExtensionHooks {
        tools::ToolExtensionHooks {
            menu: Some(marks_menu),
            menu_run: Some(marks_menu_run),
            key: Some(extension_key),
            ..tools::ToolExtensionHooks::default()
        }
    }
}

/// The notes extension's one row, for the point kind only.
fn notes_menu(
    _entity: Entity,
    kind: &str,
    _selected: &[u64],
    _world: &World,
    out: &mut Vec<tools::ElementMenuEntry>,
) {
    if kind != FIXTURE_POINT_KIND {
        return;
    }
    out.push(tools::ElementMenuEntry {
        id: FIXTURE_MENU_NOTE.to_string(),
        label: "Add note".to_string(),
    });
}

/// Runs the notes row.
///
/// The row changes nothing: the fixture exists to prove an extension's row
/// reaches the menu and runs as its own recorded batch, and an empty batch is
/// the smallest honest one.
fn notes_menu_run(
    _entity: Entity,
    kind: &str,
    _selected: &[u64],
    item: &str,
    _world: &World,
) -> Result<Vec<EditorCommand>, String> {
    if kind == FIXTURE_POINT_KIND && item == FIXTURE_MENU_NOTE {
        return Ok(Vec::new());
    }
    Err(format!("the notes extension adds no '{item}' row"))
}

/// The marks extension's one row, offered for every kind.
fn marks_menu(
    _entity: Entity,
    _kind: &str,
    _selected: &[u64],
    _world: &World,
    out: &mut Vec<tools::ElementMenuEntry>,
) {
    out.push(tools::ElementMenuEntry {
        id: FIXTURE_MENU_MARK.to_string(),
        label: "Mark".to_string(),
    });
}

/// Runs the marks row.
fn marks_menu_run(
    _entity: Entity,
    _kind: &str,
    _selected: &[u64],
    item: &str,
    _world: &World,
) -> Result<Vec<EditorCommand>, String> {
    if item == FIXTURE_MENU_MARK {
        return Ok(Vec::new());
    }
    Err(format!("the marks extension adds no '{item}' row"))
}

/// What the fixture's extensions make of their shared chord: they take it.
///
/// The chain asks them in `order`, so the notes extension's answer is the one
/// that counts while both are attached; either way the editor logs the same
/// consumed-key line, which is what a suite reads.
fn extension_key(
    _entity: Entity,
    key: &str,
    _selected: &[u64],
    _world: &World,
) -> tools::KeyResponse {
    if key.eq_ignore_ascii_case(FIXTURE_EXTENSION_CHORD) {
        return tools::KeyResponse::Consume;
    }
    tools::KeyResponse::Default
}

// =============================================================================
// Menu, structure and overrides
// =============================================================================

/// The id of the menu row that adds a point after the last one.
pub(super) const FIXTURE_MENU_INSERT: &str = "insert";

/// The id of the menu row that puts every point back on its layout.
pub(super) const FIXTURE_MENU_RESET: &str = "reset";

/// The rows the fixture adds to the element context menu.
///
/// A suite reads these from `dump elements` and runs one with `element menu`,
/// which is the same request a click on the row queues.
fn fixture_menu(
    _entity: Entity,
    kind: &str,
    selected: &[u64],
    _world: &World,
    out: &mut Vec<tools::ElementMenuEntry>,
) {
    if kind != FIXTURE_POINT_KIND {
        return;
    }
    out.push(tools::ElementMenuEntry {
        id: FIXTURE_MENU_INSERT.to_string(),
        label: "Insert point".to_string(),
    });
    if !selected.is_empty() {
        out.push(tools::ElementMenuEntry {
            id: FIXTURE_MENU_RESET.to_string(),
            label: "Reset points".to_string(),
        });
    }
}

/// Runs one of the fixture's menu rows, answering the commands it means.
///
/// The insert row adds a point a spacing past the last one; the reset row
/// moves every point back onto the layout. Both are plain edits the editor
/// records, so the row undoes as one step like any other.
fn fixture_menu_run(
    entity: Entity,
    kind: &str,
    selected: &[u64],
    item: &str,
    world: &World,
) -> Result<Vec<EditorCommand>, String> {
    if kind != FIXTURE_POINT_KIND {
        return Err(format!(
            "the fixture adds no '{item}' row for {kind} elements"
        ));
    }
    match item {
        FIXTURE_MENU_INSERT => {
            let Some(transform) = read_transform(world, entity) else {
                return Err("the entity has no transform".to_string());
            };
            let storage = world.get_resource::<FixtureElements>();
            let next_id = storage
                .and_then(|storage| storage.by_entity.get(&entity))
                .map(|elements| elements.keys().copied().max().unwrap_or(0) + 1)
                .unwrap_or(FIXTURE_POINT_COUNT as u64);
            let last = fixture_layout()
                .iter()
                .filter(|(id, _)| *id < FIXTURE_TANGENT_ID_BASE)
                .count() as f32
                - 1.0;
            let offset = [
                (last + 1.0 - (FIXTURE_POINT_COUNT as f32 - 1.0) * 0.5) * FIXTURE_ELEMENT_SPACING,
                0.0,
                0.0,
            ];
            Ok(vec![EditorCommand::InsertElements {
                entity,
                kind: FIXTURE_POINT_KIND.to_string(),
                elements: vec![tools::ElementMove {
                    element: next_id,
                    transform: tools::ElementTransform::at(gizmo::add(
                        transform.translation,
                        offset,
                    )),
                }],
            }])
        }
        FIXTURE_MENU_RESET => {
            let Some(transform) = read_transform(world, entity) else {
                return Err("the entity has no transform".to_string());
            };
            let moves = fixture_layout()
                .into_iter()
                .filter(|(id, _)| selected.contains(id))
                .map(|(id, element)| tools::ElementMove {
                    element: id,
                    transform: tools::ElementTransform {
                        translation: gizmo::add(transform.translation, element.offset),
                        ..tools::ElementTransform::at([0.0; 3])
                    },
                })
                .collect::<Vec<_>>();
            if moves.is_empty() {
                return Err("no point is selected to reset".to_string());
            }
            Ok(vec![EditorCommand::SetElementTransforms {
                entity,
                kind: FIXTURE_POINT_KIND.to_string(),
                moves,
            }])
        }
        other => Err(format!("the fixture has no menu row '{other}'")),
    }
}

/// Inserts one point into the fixture's storage, under `id` when that id is
/// free.
fn fixture_insert(
    entity: Entity,
    kind: &str,
    id: Option<u64>,
    transform: tools::ElementTransform,
    world: &mut World,
) -> Option<u64> {
    if kind != FIXTURE_POINT_KIND {
        return None;
    }
    let entity_transform = read_transform(world, entity)?;
    let storage = world.get_resource_mut::<FixtureElements>()?;
    let elements = storage.entities_elements(entity);
    let assigned = match id {
        Some(id) if !elements.contains_key(&id) => id,
        Some(_) => return None,
        None => elements.keys().copied().max().unwrap_or(0) + 1,
    };
    elements.insert(
        assigned,
        FixtureElement {
            offset: gizmo::sub(transform.translation, entity_transform.translation),
            rotation: transform.rotation,
            scale: transform.scale,
        },
    );
    Some(assigned)
}

/// Removes points from the fixture's storage, answering how many went.
fn fixture_remove(entity: Entity, kind: &str, elements: &[u64], world: &mut World) -> usize {
    if kind != FIXTURE_POINT_KIND {
        return 0;
    }
    let Some(storage) = world.get_resource_mut::<FixtureElements>() else {
        return 0;
    };
    let points = storage.entities_elements(entity);
    elements
        .iter()
        .filter(|id| points.remove(id).is_some())
        .count()
}

/// What a press on a fixture point means.
///
/// The fixture demonstrates both answers a tool can give about the editor's own
/// click rule:
///
/// - a **plain press on a point that is already selected is taken over**: the
///   fixture logs it and answers [`tools::PressResponse::Consume`], so the
///   selection stays exactly as it was;
/// - a **Ctrl press on a point that is already selected removes it**: the
///   fixture answers [`tools::PressResponse::Select`] with the rest of the
///   selection, which is the tool's own toggle rule rather than the editor's
///   add-to-selection one.
///
/// Every other press is the editor's.
fn fixture_press(
    entity: Entity,
    element: u64,
    click: SelectionClick,
    selected: &[u64],
    _world: &World,
) -> tools::PressResponse {
    let _ = entity;
    if !selected.contains(&element) {
        return tools::PressResponse::Default;
    }
    match click {
        SelectionClick::Add => tools::PressResponse::Select(
            selected
                .iter()
                .copied()
                .filter(|id| *id != element)
                .collect(),
        ),
        SelectionClick::Reset => {
            pill_core::info!(
                target: telemetry_target::EDITOR,
                element,
                selected = selected.len(),
                "[editor] element press taken by the fixture"
            );
            tools::PressResponse::Consume
        }
        SelectionClick::Range => tools::PressResponse::Default,
    }
}

/// The keys the fixture owns while its element selection is up.
///
/// `k` is consumed outright, which is how a suite proves the hook is asked
/// before the keybindings; `i` answers with the insert the menu row means, so
/// a key can run the same edit a menu click would.
fn fixture_key(entity: Entity, key: &str, selected: &[u64], world: &World) -> tools::KeyResponse {
    match key {
        "k" => tools::KeyResponse::Consume,
        "i" => match fixture_menu_run(
            entity,
            FIXTURE_POINT_KIND,
            selected,
            FIXTURE_MENU_INSERT,
            world,
        ) {
            Ok(commands) => tools::KeyResponse::Commands(commands),
            Err(_) => tools::KeyResponse::Default,
        },
        _ => tools::KeyResponse::Default,
    }
}

/// Logs every element-selection change the fixture hears.
///
/// The line is the proof that the notification reaches a tool whatever changed
/// the selection: a click, a marquee, an undo and a command all produce it.
fn fixture_selection_changed(entity: Entity, previous: &[u64], current: &[u64], _world: &World) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entity = %entity,
        before = previous.len(),
        after = current.len(),
        "[editor] fixture selection changed"
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
