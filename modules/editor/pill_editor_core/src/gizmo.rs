//! Transform gizmos: handle geometry, drag math, picking and snapping.
//!
//! # Responsibilities
//!
//! - Name the tool's modes ([`GizmoMode`]: translate, rotate, scale) and
//!   spaces ([`GizmoSpace`]: world, local), and the settings a toolbar edits
//!   ([`GizmoSettings`]).
//! - Build the handle geometry a renderer draws, one handle id per axis of the
//!   active mode ([`build_geometry`]).
//! - Answer which handle a cursor ray is over ([`hit_test`]) and build the
//!   guides a drag draws ([`axis_guide`], [`dotted_segments`]).
//! - Do the drag math: project a cursor ray onto the dragged axis or rotation
//!   plane, and snap the result to the configured step.
//! - Carry the editable-field table ([`RESOURCE_FIELDS`]) that the Inspector's
//!   rows and `SetResourceField` both use.
//!
//! # Design
//!
//! Everything here is a pure function over plain arrays, so a drag can be
//! tested without a renderer or a window. The editor executable owns the
//! camera and the world; this module only does the arithmetic.
//!
//! Handle ids are stable numbers: `1..=3` for translate axes, `4..=6` for
//! translation planes, `11..=13` for rotate rings and `21..=23` for scale
//! axes. A pick's `Handle(id)` decodes through [`handle_kind`] into the mode
//! and the axis it belongs to (or the axis pair a plane spans), so the
//! renderer never needs to know what a handle means.

// External crates
use pill_engine::{Entity, FieldValue};
use serde::{Deserialize, Serialize};

// =============================================================================
// State
// =============================================================================

/// The gizmo tool's state: the settings the editor draws and drags with.
///
/// This is a world resource, not shell state. The executable owns the drag
/// (it holds the camera and receives the pick answers) and writes the
/// preferences file; the shell only queues the commands. Both sides meet here,
/// so a shell reload keeps the tool as it is, and the draw, the pick and the
/// drag always agree about which handles are on screen.
#[derive(Clone, Copy, Debug, Default)]
pub struct GizmoState {
    /// What the tool edits, in which space, and how it snaps.
    pub settings: GizmoSettings,
}

impl pill_engine::Resource for GizmoState {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some(RESOURCE_NAME)
    }
}

// =============================================================================
// Resource fields
// =============================================================================

/// The name [`GizmoState`] registers under, which a `SetResourceField`
/// command must also carry. Keeping them the same lets a command written by
/// hand find the resource.
pub const RESOURCE_NAME: &str = "pill_editor_core::gizmo::GizmoState";

/// One editable field of the gizmo settings.
///
/// It carries the name a command addresses the field by, the label the
/// Inspector's row shows, and the functions that read and write it.
#[derive(Clone, Copy, Debug)]
pub struct ResourceField {
    /// The field's name in a `SetResourceField` command.
    pub name: &'static str,
    /// The label the Inspector's row shows.
    pub label: &'static str,
    /// Read the field out of the settings.
    pub read: fn(&GizmoSettings) -> FieldValue,
    /// Write the field and return its previous value, which the command's
    /// inverse restores.
    pub write: fn(&mut GizmoSettings, &FieldValue) -> Result<FieldValue, String>,
}

/// Every editable field, in the order the Inspector lists them.
///
/// One table serves three readers: the write path ([`set_resource_field`]),
/// the read path ([`resource_field_value`]) and the Inspector's rows, through
/// [`crate::resources::EDITABLE_RESOURCES`]. A field therefore cannot be
/// offered by the tab and refused by the write, or be readable and not
/// writable.
///
/// An enum field travels as its `U32` discriminant, in declaration order. A
/// step travels as an `F32`.
pub const RESOURCE_FIELDS: &[ResourceField] = &[
    ResourceField {
        name: "settings.mode",
        label: "Mode",
        read: |settings| FieldValue::U32(mode_discriminant(settings.mode)),
        write: |settings, field_value| {
            let previous = settings.mode;
            settings.mode = mode_from_discriminant(field_u32("settings.mode", field_value)?)?;
            Ok(FieldValue::U32(mode_discriminant(previous)))
        },
    },
    ResourceField {
        name: "settings.space",
        label: "Space",
        read: |settings| FieldValue::U32(space_discriminant(settings.space)),
        write: |settings, field_value| {
            let previous = settings.space;
            settings.space = space_from_discriminant(field_u32("settings.space", field_value)?)?;
            Ok(FieldValue::U32(space_discriminant(previous)))
        },
    },
    ResourceField {
        name: "settings.snap",
        label: "Snapping",
        read: |settings| FieldValue::Bool(settings.snap),
        write: |settings, field_value| {
            let previous = settings.snap;
            settings.snap = match field_value {
                FieldValue::Bool(value) => *value,
                other => return Err(format!("`settings.snap` expects a Bool, got {other:?}")),
            };
            Ok(FieldValue::Bool(previous))
        },
    },
    ResourceField {
        name: "settings.translate_step",
        label: "Move step",
        read: |settings| FieldValue::F32(settings.translate_step),
        write: |settings, field_value| {
            swap_f32(
                &mut settings.translate_step,
                "settings.translate_step",
                field_value,
            )
        },
    },
    ResourceField {
        name: "settings.rotate_step_degrees",
        label: "Rotate step",
        read: |settings| FieldValue::F32(settings.rotate_step_degrees),
        write: |settings, field_value| {
            swap_f32(
                &mut settings.rotate_step_degrees,
                "settings.rotate_step_degrees",
                field_value,
            )
        },
    },
    ResourceField {
        name: "settings.scale_step",
        label: "Scale step",
        read: |settings| FieldValue::F32(settings.scale_step),
        write: |settings, field_value| {
            swap_f32(&mut settings.scale_step, "settings.scale_step", field_value)
        },
    },
];

/// The table's row for `field_name`, when it has one.
pub fn resource_field(field_name: &str) -> Option<&'static ResourceField> {
    RESOURCE_FIELDS.iter().find(|row| row.name == field_name)
}

/// Sets one field of the gizmo settings from a field value and returns the
/// previous value, for the command's inverse.
///
/// The engine cannot read or write a Rust resource's fields generically, so
/// [`RESOURCE_FIELDS`] lists the ones that may be written. This function is a
/// lookup in that table, then the row's own write.
///
/// # Errors
///
/// Returns the message to show the user when `resource_name` is not
/// [`RESOURCE_NAME`], when `field_name` is not in [`RESOURCE_FIELDS`], or when
/// `field_value` has a kind the field cannot hold.
pub fn set_resource_field(
    settings: &mut GizmoSettings,
    resource_name: &str,
    field_name: &str,
    field_value: &FieldValue,
) -> Result<FieldValue, String> {
    check_resource(resource_name)?;
    let row = resource_field(field_name).ok_or_else(|| {
        let field_names: Vec<&str> = RESOURCE_FIELDS.iter().map(|row| row.name).collect();
        format!(
            "`{field_name}` is not an editable field of `{RESOURCE_NAME}` ({})",
            field_names.join(", ")
        )
    })?;
    (row.write)(settings, field_value)
}

/// Reads one editable field of the gizmo settings: the reading half of the
/// table [`set_resource_field`] writes through.
///
/// # Errors
///
/// Returns the message to show the user when `resource_name` is not
/// [`RESOURCE_NAME`] or when `field_name` is not in [`RESOURCE_FIELDS`].
pub fn resource_field_value(
    settings: &GizmoSettings,
    resource_name: &str,
    field_name: &str,
) -> Result<FieldValue, String> {
    check_resource(resource_name)?;
    let row = resource_field(field_name)
        .ok_or_else(|| format!("`{field_name}` is not a readable field of `{RESOURCE_NAME}`"))?;
    Ok((row.read)(settings))
}

/// Refuse any resource but the gizmo state, naming it in the message.
fn check_resource(resource_name: &str) -> Result<(), String> {
    if resource_name == RESOURCE_NAME {
        Ok(())
    } else {
        Err(format!(
            "`{resource_name}` is not an editable resource (`{RESOURCE_NAME}` is)"
        ))
    }
}

/// A `U32` field value, accepting the wider unsigned forms too.
fn field_u32(field_name: &str, field_value: &FieldValue) -> Result<u32, String> {
    match field_value {
        FieldValue::U32(value) => Ok(*value),
        FieldValue::U64(value) => {
            u32::try_from(*value).map_err(|_| format!("`{field_name}` does not fit a u32"))
        }
        other => Err(format!("`{field_name}` expects a U32, got {other:?}")),
    }
}

/// Swap one `f32` field and hand back its previous value.
fn swap_f32(
    field_slot: &mut f32,
    field_name: &str,
    field_value: &FieldValue,
) -> Result<FieldValue, String> {
    let next = match field_value {
        FieldValue::F32(value) => *value,
        FieldValue::F64(value) => *value as f32,
        other => return Err(format!("`{field_name}` expects an F32, got {other:?}")),
    };
    let previous = *field_slot;
    *field_slot = next;
    Ok(FieldValue::F32(previous))
}

/// The `U32` discriminant of a mode, in declaration order.
fn mode_discriminant(mode: GizmoMode) -> u32 {
    match mode {
        GizmoMode::Translate => 0,
        GizmoMode::Rotate => 1,
        GizmoMode::Scale => 2,
    }
}

/// The mode a discriminant names.
fn mode_from_discriminant(discriminant: u32) -> Result<GizmoMode, String> {
    match discriminant {
        0 => Ok(GizmoMode::Translate),
        1 => Ok(GizmoMode::Rotate),
        2 => Ok(GizmoMode::Scale),
        _ => Err("`settings.mode` is 0 (translate), 1 (rotate) or 2 (scale)".to_owned()),
    }
}

/// The `U32` discriminant of a space, in declaration order.
fn space_discriminant(space: GizmoSpace) -> u32 {
    match space {
        GizmoSpace::World => 0,
        GizmoSpace::Local => 1,
    }
}

/// The space a discriminant names.
fn space_from_discriminant(discriminant: u32) -> Result<GizmoSpace, String> {
    match discriminant {
        0 => Ok(GizmoSpace::World),
        1 => Ok(GizmoSpace::Local),
        _ => Err("`settings.space` is 0 (world) or 1 (local)".to_owned()),
    }
}

// =============================================================================
// Settings
// =============================================================================

/// Which transform a gizmo edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GizmoMode {
    /// Move the entity along one axis.
    Translate,
    /// Turn the entity around one axis.
    Rotate,
    /// Scale the entity along one axis.
    Scale,
}

impl From<GizmoMode> for crate::tools::BuiltinTool {
    /// The built-in tool a gizmo mode belongs to: the three modes and the three
    /// built-in tools are the same thing under two names.
    fn from(mode: GizmoMode) -> crate::tools::BuiltinTool {
        match mode {
            GizmoMode::Translate => crate::tools::BuiltinTool::Translate,
            GizmoMode::Rotate => crate::tools::BuiltinTool::Rotate,
            GizmoMode::Scale => crate::tools::BuiltinTool::Scale,
        }
    }
}

/// What "the axes" mean: the world's, or the entity's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GizmoSpace {
    /// The world's X/Y/Z.
    World,
    /// The entity's rotation applied to those axes.
    Local,
}

/// The tool's settings: what it edits, in which space, and how it snaps.
///
/// The preferences file stores them, snapping steps included, so a step can be
/// edited by hand and the panel and the file always show the same values.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GizmoSettings {
    /// The active mode.
    #[serde(default = "default_mode")]
    pub mode: GizmoMode,
    /// Whether the handles align with the entity's own orientation.
    #[serde(default = "default_space")]
    pub space: GizmoSpace,
    /// Whether dragging snaps.
    #[serde(default)]
    pub snap: bool,
    /// Translate snapping step, in world units.
    #[serde(default = "default_translate_step")]
    pub translate_step: f32,
    /// Rotate snapping step, in degrees.
    #[serde(default = "default_rotate_step")]
    pub rotate_step_degrees: f32,
    /// Scale snapping step, as a factor increment.
    #[serde(default = "default_scale_step")]
    pub scale_step: f32,
}

fn default_mode() -> GizmoMode {
    GizmoMode::Translate
}

fn default_space() -> GizmoSpace {
    GizmoSpace::World
}

fn default_translate_step() -> f32 {
    0.5
}

fn default_rotate_step() -> f32 {
    15.0
}

fn default_scale_step() -> f32 {
    0.1
}

impl Default for GizmoSettings {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            space: default_space(),
            snap: false,
            translate_step: default_translate_step(),
            rotate_step_degrees: default_rotate_step(),
            scale_step: default_scale_step(),
        }
    }
}

impl GizmoSettings {
    /// The snapping step that belongs to `mode`.
    pub fn step_for(&self, mode: GizmoMode) -> f32 {
        match mode {
            GizmoMode::Translate => self.translate_step,
            GizmoMode::Rotate => self.rotate_step_degrees,
            GizmoMode::Scale => self.scale_step,
        }
    }
}

// =============================================================================
// Handle ids
// =============================================================================

/// The handle one axis of a mode is pickable by.
pub const fn axis_handle(mode: GizmoMode, axis_index: usize) -> u32 {
    match mode {
        GizmoMode::Translate => 1 + axis_index as u32,
        GizmoMode::Rotate => 11 + axis_index as u32,
        GizmoMode::Scale => 21 + axis_index as u32,
    }
}

/// The three translation planes, as the axis pairs they span.
pub const PLANE_PAIRS: [(usize, usize); 3] = [(0, 1), (0, 2), (1, 2)];

/// The handle one translation plane is pickable by: the square between its
/// two axes. Planes only exist on the translation tool.
pub const fn plane_handle(first_axis: usize, second_axis: usize) -> u32 {
    let plane_index = match (first_axis, second_axis) {
        (0, 1) => 0,
        (0, 2) => 1,
        (1, 2) => 2,
        _ => 0,
    };
    4 + plane_index as u32
}

/// What a handle id addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleKind {
    /// One axis of one mode.
    Axis {
        /// The mode the axis belongs to.
        mode: GizmoMode,
        /// The axis index, `0..=2` for X, Y and Z.
        axis: usize,
    },
    /// A translation plane, spanned by two axes.
    Plane {
        /// The first axis of the plane.
        first: usize,
        /// The second axis of the plane.
        second: usize,
    },
}

impl HandleKind {
    /// The axis a single-axis handle drags along; `None` for a plane.
    pub fn axis(self) -> Option<(GizmoMode, usize)> {
        match self {
            HandleKind::Axis { mode, axis } => Some((mode, axis)),
            HandleKind::Plane { .. } => None,
        }
    }

    /// The axis pair a plane handle spans; `None` for a single axis.
    pub fn plane(self) -> Option<(usize, usize)> {
        match self {
            HandleKind::Plane { first, second } => Some((first, second)),
            HandleKind::Axis { .. } => None,
        }
    }
}

/// What a handle id addresses; `None` for an id this module never issues.
///
/// Ids are stable numbers: `1..=3` translate axes, `4..=6` translation
/// planes, `11..=13` rotate rings, `21..=23` scale axes.
pub fn handle_kind(handle: u32) -> Option<HandleKind> {
    match handle {
        1..=3 => Some(HandleKind::Axis {
            mode: GizmoMode::Translate,
            axis: (handle - 1) as usize,
        }),
        4..=6 => {
            let (first_axis, second_axis) = PLANE_PAIRS[(handle - 4) as usize];
            Some(HandleKind::Plane {
                first: first_axis,
                second: second_axis,
            })
        }
        11..=13 => Some(HandleKind::Axis {
            mode: GizmoMode::Rotate,
            axis: (handle - 11) as usize,
        }),
        21..=23 => Some(HandleKind::Axis {
            mode: GizmoMode::Scale,
            axis: (handle - 21) as usize,
        }),
        _ => None,
    }
}

/// Which mode and axis a handle belongs to; `None` for an id this module never
/// issues, or for a plane handle.
pub fn handle_axis(handle: u32) -> Option<(GizmoMode, usize)> {
    handle_kind(handle).and_then(HandleKind::axis)
}

// =============================================================================
// Element handles
// =============================================================================

/// The first handle id an element handle may occupy.
///
/// The transform gizmo owns `1..=23`, and a pick answers with only the id it
/// was drawn with, so the element band starts well above them: no element can
/// ever be read as a gizmo handle, and no gizmo handle as an element.
pub const ELEMENT_HANDLE_BASE: u32 = 1000;

/// How many element handles one frame may address.
pub const ELEMENT_HANDLE_LIMIT: u32 = 4096;

/// The handle id the element in slot `index` of a frame's map is pickable by.
pub const fn element_handle(index: u32) -> u32 {
    ELEMENT_HANDLE_BASE + index
}

/// The slot a handle id addresses, or `None` when the id is not an element
/// handle.
pub fn element_handle_slot(handle: u32) -> Option<usize> {
    if !(ELEMENT_HANDLE_BASE..ELEMENT_HANDLE_BASE + ELEMENT_HANDLE_LIMIT).contains(&handle) {
        return None;
    }
    Some((handle - ELEMENT_HANDLE_BASE) as usize)
}

/// What one element handle of a frame points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementHandleTarget {
    /// The entity the element belongs to.
    pub entity: Entity,
    /// The tool's own id for the element.
    pub element: u64,
    /// The kind's name, as the tool reported it.
    pub kind: String,
}

/// One frame's element handles: the slot in this list a handle's id was built
/// from answers what the handle points at.
///
/// The editor rebuilds it every frame from the active tool's report, so a
/// handle can never outlive the element it was drawn for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElementHandleMap {
    targets: Vec<ElementHandleTarget>,
}

impl ElementHandleMap {
    /// An empty map, ready for a frame's handles.
    pub fn new() -> ElementHandleMap {
        ElementHandleMap::default()
    }

    /// Records `target` as the next handle and answers with its id.
    ///
    /// # Panics
    ///
    /// Panics when a frame reports more than [`ELEMENT_HANDLE_LIMIT`]
    /// elements; the limit is far above what a tool on one entity reports.
    pub fn push(&mut self, target: ElementHandleTarget) -> u32 {
        assert!(
            (self.targets.len() as u32) < ELEMENT_HANDLE_LIMIT,
            "a frame cannot address more than {ELEMENT_HANDLE_LIMIT} element handles"
        );
        let handle = element_handle(self.targets.len() as u32);
        self.targets.push(target);
        handle
    }

    /// What `handle` points at, or `None` when it is not one of this frame's
    /// element handles.
    pub fn target(&self, handle: u32) -> Option<&ElementHandleTarget> {
        self.targets.get(element_handle_slot(handle)?)
    }

    /// The recorded handles in slot order.
    pub fn targets(&self) -> &[ElementHandleTarget] {
        &self.targets
    }

    /// Whether the frame reported no elements.
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// How many elements the frame reported.
    pub fn len(&self) -> usize {
        self.targets.len()
    }
}

// =============================================================================
// Appearance
// =============================================================================

/// The world axes, in handle order.
pub const AXES: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// The colour one axis draws in: X red, Y green, Z blue.
pub const AXIS_COLORS: [[f32; 4]; 3] = [
    [0.90, 0.25, 0.20, 1.0],
    [0.35, 0.80, 0.30, 1.0],
    [0.30, 0.50, 0.95, 1.0],
];

/// The factor a hovered handle's colour is multiplied by.
pub const HOVER_BRIGHTEN: f32 = 1.35;
/// The factor the active handle's colour is multiplied by.
pub const ACTIVE_DIM: f32 = 0.70;

/// The plane squares' alpha: translucent, so the axis arrows behind them stay
/// visible.
pub const PLANE_ALPHA: f32 = 0.55;

/// Where a plane square's centre sits along both of its axes, as a fraction
/// of the gizmo's length.
pub const PLANE_OFFSET: f32 = 0.32;
/// Half of a plane square's side, as a fraction of the gizmo's length.
pub const PLANE_HALF: f32 = 0.09;

/// The largest factor a scale handle may grow to during a drag. Past it the
/// handle would reach out of the view.
pub const SCALE_HANDLE_MAX_RATIO: f32 = 3.0;
/// The smallest factor a scale handle may shrink to during a drag. Below it
/// the handle would vanish into the origin.
pub const SCALE_HANDLE_MIN_RATIO: f32 = 0.2;

/// The handle the pointer is over and the handle a drag holds.
///
/// The geometry is built in this state, so a hover or a drag shows on the
/// handles the frame draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HandleStates {
    /// The handle under the pointer, if one is.
    pub hovered: Option<u32>,
    /// The handle a drag holds, if one does.
    pub active: Option<u32>,
}

impl HandleStates {
    /// The state one handle draws in: active wins over hovered.
    pub fn of(self, handle: u32) -> HandleState {
        if self.active == Some(handle) {
            HandleState::Active
        } else if self.hovered == Some(handle) {
            HandleState::Hovered
        } else {
            HandleState::Idle
        }
    }
}

/// The visual state of one handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleState {
    /// Normal appearance.
    Idle,
    /// Under the pointer: brighter.
    Hovered,
    /// Held by a drag: dimmer than the hover state.
    Active,
}

/// `base_color` adjusted for the state `handle` is in.
pub fn styled_color(base_color: [f32; 4], handle: u32, states: HandleStates) -> [f32; 4] {
    match states.of(handle) {
        HandleState::Idle => base_color,
        HandleState::Hovered => [
            (base_color[0] * HOVER_BRIGHTEN).min(1.0),
            (base_color[1] * HOVER_BRIGHTEN).min(1.0),
            (base_color[2] * HOVER_BRIGHTEN).min(1.0),
            base_color[3],
        ],
        HandleState::Active => [
            base_color[0] * ACTIVE_DIM,
            base_color[1] * ACTIVE_DIM,
            base_color[2] * ACTIVE_DIM,
            base_color[3],
        ],
    }
}

/// The colour a plane square draws in: its two axes' colours averaged, at
/// [`PLANE_ALPHA`].
pub fn plane_color(first_axis: usize, second_axis: usize) -> [f32; 4] {
    let first_color = AXIS_COLORS[first_axis];
    let second_color = AXIS_COLORS[second_axis];
    [
        (first_color[0] + second_color[0]) * 0.5,
        (first_color[1] + second_color[1]) * 0.5,
        (first_color[2] + second_color[2]) * 0.5,
        PLANE_ALPHA,
    ]
}

// =============================================================================
// Geometry
// =============================================================================

/// One gizmo line: its two endpoints, its colour, and its handle id.
pub type GizmoLine = ([f32; 3], [f32; 3], [f32; 4], u32);

/// One gizmo triangle: its three points, its colour, and its handle id.
pub type GizmoTriangle = ([[f32; 3]; 3], [f32; 4], u32);

/// The handle geometry one gizmo draws.
///
/// The executable adds every line and triangle to the engine's `DebugDraw`,
/// and each one carries the id of the handle it belongs to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GizmoGeometry {
    /// Handle lines.
    pub lines: Vec<GizmoLine>,
    /// Handle triangles (arrow and cube heads).
    pub triangles: Vec<GizmoTriangle>,
}

/// Builds the handles of `mode` around `origin`, one per basis vector, each
/// `length` long, with no hover or drag.
///
/// This is the plain form of [`build_geometry_styled`].
///
/// `basis` is the frame the axes draw in: the world axes, or those axes with
/// the entity's rotation applied for local space. The caller picks the space,
/// so the geometry itself needs no quaternions.
pub fn build_geometry(
    mode: GizmoMode,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    length: f32,
) -> GizmoGeometry {
    build_geometry_styled(mode, origin, basis, [length; 3], HandleStates::default())
}

/// Builds the handles of `mode` around `origin`, one per basis vector, each as
/// long as `lengths` gives for its axis.
///
/// `states` decides which handle is drawn hovered or active.
///
/// A scale drag passes the ratio it is writing as the dragged axis's length,
/// so that handle grows and shrinks with the object's scale. The translation
/// tool also builds its three plane squares here, one between each pair of
/// axes.
pub fn build_geometry_styled(
    mode: GizmoMode,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    lengths: [f32; 3],
    states: HandleStates,
) -> GizmoGeometry {
    let mut geometry = GizmoGeometry::default();
    // Step 1: one handle per basis vector - a line, a ring or a cube head,
    // depending on the mode.
    for (axis_index, axis) in basis.iter().enumerate() {
        let handle = axis_handle(mode, axis_index);
        let color = styled_color(AXIS_COLORS[axis_index], handle, states);
        let length = lengths[axis_index].max(1e-4);
        let tip = add(origin, scale(*axis, length));
        match mode {
            GizmoMode::Translate => {
                geometry.lines.push((origin, tip, color, handle));
                // The arrow head: a square base part-way along the axis,
                // with one triangle from each of its edges to the tip.
                head_triangles(&mut geometry.triangles, origin, *axis, tip, color, handle);
            }
            GizmoMode::Scale => {
                geometry.lines.push((origin, tip, color, handle));
                cube_triangles(
                    &mut geometry.triangles,
                    tip,
                    *axis,
                    length * 0.08,
                    color,
                    handle,
                );
            }
            GizmoMode::Rotate => {
                // A full ring around the axis, cut into segments. Every
                // segment carries the ring's handle, so any part of the ring
                // can be grabbed.
                let (u, v) = plane_axes(*axis);
                let ring_segment_count = 48;
                let mut previous = ring_point(origin, u, v, length, 0.0);
                for segment_index in 1..=ring_segment_count {
                    let angle =
                        segment_index as f32 / ring_segment_count as f32 * std::f32::consts::TAU;
                    let point = ring_point(origin, u, v, length, angle);
                    geometry.lines.push((previous, point, color, handle));
                    previous = point;
                }
            }
        }
    }
    // Step 2: the three plane squares, which only the translation tool has.
    if mode == GizmoMode::Translate {
        for (first_axis, second_axis) in PLANE_PAIRS {
            let handle = plane_handle(first_axis, second_axis);
            let color = styled_color(plane_color(first_axis, second_axis), handle, states);
            let length = (lengths[first_axis] + lengths[second_axis]) * 0.5;
            plane_square(
                &mut geometry.triangles,
                origin,
                basis[first_axis],
                basis[second_axis],
                length,
                color,
                handle,
            );
        }
    }
    geometry
}

/// Builds one translation plane's square as two triangles.
///
/// The square is centred [`PLANE_OFFSET`] times `length` along both of the
/// plane's axes, and its half-size is [`PLANE_HALF`] times `length`.
fn plane_square(
    triangles: &mut Vec<GizmoTriangle>,
    origin: [f32; 3],
    first_axis_direction: [f32; 3],
    second_axis_direction: [f32; 3],
    length: f32,
    color: [f32; 4],
    handle: u32,
) {
    let center_offset = length * PLANE_OFFSET;
    let half_size = length * PLANE_HALF;
    let center = add(
        origin,
        add(
            scale(first_axis_direction, center_offset),
            scale(second_axis_direction, center_offset),
        ),
    );
    let corner = |along_first: f32, along_second: f32| {
        add(
            center,
            add(
                scale(first_axis_direction, along_first * half_size),
                scale(second_axis_direction, along_second * half_size),
            ),
        )
    };
    let corners = [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ];
    triangles.push(([corners[0], corners[1], corners[2]], color, handle));
    triangles.push(([corners[0], corners[2], corners[3]], color, handle));
}

/// One point of a rotation ring.
fn ring_point(origin: [f32; 3], u: [f32; 3], v: [f32; 3], radius: f32, angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    add(origin, add(scale(u, radius * cos), scale(v, radius * sin)))
}

/// Builds an arrow head at `tip`: a square base 80% of the way along `axis`,
/// with one triangle from each of its edges to the tip.
///
/// The head is small next to the axis length, so it reads as an arrow from any
/// side.
fn head_triangles(
    triangles: &mut Vec<GizmoTriangle>,
    origin: [f32; 3],
    axis: [f32; 3],
    tip: [f32; 3],
    color: [f32; 4],
    handle: u32,
) {
    let (u, v) = plane_axes(axis);
    let square_center = add(origin, scale(sub(tip, origin), 0.8));
    let half_size = distance(origin, tip) * 0.06;
    let corners = [
        add(add(square_center, scale(u, half_size)), scale(v, half_size)),
        add(
            add(square_center, scale(u, -half_size)),
            scale(v, half_size),
        ),
        add(
            add(square_center, scale(u, -half_size)),
            scale(v, -half_size),
        ),
        add(
            add(square_center, scale(u, half_size)),
            scale(v, -half_size),
        ),
    ];
    for corner_index in 0..4 {
        let next_index = (corner_index + 1) % 4;
        triangles.push((
            [corners[corner_index], corners[next_index], tip],
            color,
            handle,
        ));
    }
}

/// Twelve triangles forming a small cube around `center`.
fn cube_triangles(
    triangles: &mut Vec<GizmoTriangle>,
    center: [f32; 3],
    axis: [f32; 3],
    half_size: f32,
    color: [f32; 4],
    handle: u32,
) {
    let (u, v) = plane_axes(axis);
    let corner = |along_u: f32, along_v: f32, along_axis: f32| {
        add(
            center,
            add(
                add(scale(u, along_u * half_size), scale(v, along_v * half_size)),
                scale(axis, along_axis * half_size),
            ),
        )
    };
    let corners = [
        corner(-1.0, -1.0, -1.0),
        corner(1.0, -1.0, -1.0),
        corner(1.0, 1.0, -1.0),
        corner(-1.0, 1.0, -1.0),
        corner(-1.0, -1.0, 1.0),
        corner(1.0, -1.0, 1.0),
        corner(1.0, 1.0, 1.0),
        corner(-1.0, 1.0, 1.0),
    ];
    // Six faces, two triangles each.
    for (first_corner, second_corner, third_corner, fourth_corner) in [
        (0, 1, 2, 3),
        (4, 5, 6, 7),
        (0, 1, 5, 4),
        (1, 2, 6, 5),
        (2, 3, 7, 6),
        (3, 0, 4, 7),
    ] {
        triangles.push((
            [
                corners[first_corner],
                corners[second_corner],
                corners[third_corner],
            ],
            color,
            handle,
        ));
        triangles.push((
            [
                corners[first_corner],
                corners[third_corner],
                corners[fourth_corner],
            ],
            color,
            handle,
        ));
    }
}

// =============================================================================
// Drag math
// =============================================================================

/// Returns how far along `axis` the point closest to the cursor ray lies,
/// measured from `origin`. `None` when the ray runs parallel to the axis.
///
/// This is where two lines come closest: the axis through `origin` and the
/// cursor ray. A translation drag applies the difference between the value
/// recorded at the press and the value now, so the entity follows the cursor
/// without jumping to it.
pub fn closest_axis_point(
    origin: [f32; 3],
    axis: [f32; 3],
    ray_origin: [f32; 3],
    ray_direction: [f32; 3],
) -> Option<f32> {
    let axis = normalize(axis)?;
    let ray_direction = normalize(ray_direction)?;
    let origin_offset = sub(origin, ray_origin);
    let axis_length_squared = dot(axis, axis);
    let axis_dot_ray_direction = dot(axis, ray_direction);
    let ray_direction_length_squared = dot(ray_direction, ray_direction);
    let axis_dot_origin_offset = dot(axis, origin_offset);
    let ray_direction_dot_origin_offset = dot(ray_direction, origin_offset);
    let denominator = axis_length_squared * ray_direction_length_squared
        - axis_dot_ray_direction * axis_dot_ray_direction;
    if denominator.abs() < 1e-6 {
        return None;
    }
    Some(
        (axis_dot_ray_direction * ray_direction_dot_origin_offset
            - ray_direction_length_squared * axis_dot_origin_offset)
            / denominator,
    )
}

/// Where the cursor ray meets the plane through `origin` whose normal is
/// `normal`, or `None` when the ray runs parallel to it or the hit lies behind
/// the ray's origin.
pub fn ray_plane_point(
    origin: [f32; 3],
    normal: [f32; 3],
    ray_origin: [f32; 3],
    ray_direction: [f32; 3],
) -> Option<[f32; 3]> {
    let normal = normalize(normal)?;
    let denominator = dot(normal, ray_direction);
    if denominator.abs() < 1e-6 {
        return None;
    }
    let ray_distance = dot(normal, sub(origin, ray_origin)) / denominator;
    if ray_distance < 0.0 {
        return None;
    }
    Some(add(ray_origin, scale(ray_direction, ray_distance)))
}

/// Returns the angle of `point` around `axis`, in radians, measured in the
/// basis of the axis's rotation ring.
///
/// A rotation drag records this angle at the press and reads it again on every
/// move. The difference is the rotation, so the entity does not jump on the
/// first frame.
pub fn plane_angle(origin: [f32; 3], axis: [f32; 3], point: [f32; 3]) -> f32 {
    let (u, v) = plane_axes(axis);
    let relative = sub(point, origin);
    dot(relative, v).atan2(dot(relative, u))
}

/// Wraps an angle difference into `(-PI, PI]`.
///
/// Without the wrap, a drag past the seam - where the angle jumps from `PI` to
/// `-PI` - would turn the entity the long way round.
pub fn wrap_angle(angle: f32) -> f32 {
    let mut wrapped = angle % std::f32::consts::TAU;
    if wrapped > std::f32::consts::PI {
        wrapped -= std::f32::consts::TAU;
    } else if wrapped <= -std::f32::consts::PI {
        wrapped += std::f32::consts::TAU;
    }
    wrapped
}

/// Round `value` to the nearest multiple of `step`; a non-positive step
/// leaves it alone.
pub fn snap_value(value: f32, step: f32) -> f32 {
    if step <= 0.0 {
        return value;
    }
    (value / step).round() * step
}

/// Returns the `[x, y, z, w]` quaternion that turns by `radians` around
/// `axis`.
///
/// A zero-length axis gives the identity quaternion. A drag with no axis
/// cannot happen, but a caller must never get a NaN rotation from one.
pub fn axis_angle_quaternion(axis: [f32; 3], radians: f32) -> [f32; 4] {
    let Some(axis) = normalize(axis) else {
        return [0.0, 0.0, 0.0, 1.0];
    };
    let half_angle = radians * 0.5;
    let (sin, cos) = half_angle.sin_cos();
    [axis[0] * sin, axis[1] * sin, axis[2] * sin, cos]
}

/// Rotate `vector` by the `[x, y, z, w]` quaternion.
pub fn rotate_vector(quaternion: [f32; 4], vector: [f32; 3]) -> [f32; 3] {
    let [x, y, z, w] = quaternion;
    let quaternion_length = (x * x + y * y + z * z + w * w).sqrt();
    if quaternion_length <= f32::EPSILON {
        return vector;
    }
    let (x, y, z, w) = (
        x / quaternion_length,
        y / quaternion_length,
        z / quaternion_length,
        w / quaternion_length,
    );
    // v' = v + 2 * cross(q.xyz, cross(q.xyz, v) + w * v)
    let axis = [x, y, z];
    let axis_cross_vector = cross(axis, vector);
    let axis_cross_corrected = cross(axis, add(axis_cross_vector, scale(vector, w)));
    add(vector, scale(axis_cross_corrected, 2.0))
}

// =============================================================================
// Plane drags, scale handles and guides
// =============================================================================

/// Returns `point`'s coordinates along a plane's two axes, measured from
/// `origin`.
///
/// A plane drag records both at the press and reads them again on every move,
/// so the object follows the cursor without jumping to it.
/// [`closest_axis_point`] does the same for a single axis. The two axes are the
/// gizmo's basis vectors, so world and local space both work.
pub fn plane_coordinates(
    origin: [f32; 3],
    first_axis_direction: [f32; 3],
    second_axis_direction: [f32; 3],
    point: [f32; 3],
) -> (f32, f32) {
    let relative = sub(point, origin);
    (
        dot(relative, first_axis_direction),
        dot(relative, second_axis_direction),
    )
}

/// Returns the handle length for each axis during a scale drag.
///
/// The dragged axis follows the object's scale against its scale at the press.
/// The other axes keep `reference_length`.
///
/// The ratio is clamped to the handle's range. A zero scale at the press, or a
/// zero scale written by a command in the middle of a drag, gives a ratio that
/// is not usable; it falls back to `reference_length`, so a degenerate object
/// still draws a usable handle instead of NaN geometry.
pub fn scale_handle_lengths(
    reference_length: f32,
    original_scale: [f32; 3],
    current_scale: [f32; 3],
    axis_index: usize,
) -> [f32; 3] {
    let mut lengths = [reference_length; 3];
    if axis_index >= 3 {
        return lengths;
    }
    let scale_ratio = if original_scale[axis_index].abs() > 1e-4 {
        current_scale[axis_index] / original_scale[axis_index]
    } else {
        1.0
    };
    let clamped_ratio = if scale_ratio.is_finite() {
        scale_ratio.clamp(SCALE_HANDLE_MIN_RATIO, SCALE_HANDLE_MAX_RATIO)
    } else {
        1.0
    };
    lengths[axis_index] = reference_length * clamped_ratio;
    lengths
}

/// Returns the two ends of the guide line a single-axis drag draws: `span`
/// long on both sides of `origin`, along `axis`.
pub fn axis_guide(origin: [f32; 3], axis: [f32; 3], span: f32) -> ([f32; 3], [f32; 3]) {
    let axis = normalize(axis).unwrap_or(AXES[0]);
    (
        sub(origin, scale(axis, span)),
        add(origin, scale(axis, span)),
    )
}

/// Builds the dashes of a dotted line from `start` to `end`.
///
/// Each dash is `dash_length` long, the gaps between them are the same size,
/// and the line always ends exactly at `end`.
///
/// The caller derives the dash length from the camera distance, so the dots
/// look the same on screen at any zoom. A very long line is cut to
/// [`DOTTED_SEGMENTS_MAX`] dashes instead of building thousands of them.
pub fn dotted_segments(
    start: [f32; 3],
    end: [f32; 3],
    dash_length: f32,
) -> Vec<([f32; 3], [f32; 3])> {
    let line_length = distance(start, end);
    if line_length <= 1e-5 || dash_length <= 1e-5 {
        return Vec::new();
    }
    let dash_period = dash_length * 2.0;
    let dash_count = ((line_length / dash_period).ceil() as usize).clamp(1, DOTTED_SEGMENTS_MAX);
    let direction = scale(sub(end, start), 1.0 / line_length);
    let mut segments = Vec::with_capacity(dash_count);
    for index in 0..dash_count {
        let dash_start = line_length * index as f32 / dash_count as f32;
        // The last dash reaches the end exactly, so the line always shows
        // where the pivot is now, whatever the dash count rounds to.
        let dash_end = if index + 1 == dash_count {
            line_length
        } else {
            (dash_start + dash_length).min(line_length)
        };
        segments.push((
            add(start, scale(direction, dash_start)),
            add(start, scale(direction, dash_end)),
        ));
    }
    segments
}

/// The most dashes [`dotted_segments`] builds for one line.
pub const DOTTED_SEGMENTS_MAX: usize = 96;

// =============================================================================
// Picking
// =============================================================================

/// Returns the handle under a cursor ray: the nearest one whose line passes
/// within `radius` world units of the ray, or whose triangle the ray enters.
/// `None` when the ray touches no handle.
///
/// The test runs on the geometry the frame draws, the same numbers the
/// renderer turns into pixels, so the highlight cannot disagree with what is
/// on screen.
pub fn hit_test(
    geometry: &GizmoGeometry,
    ray_origin: [f32; 3],
    ray_direction: [f32; 3],
    radius: f32,
) -> Option<u32> {
    let mut nearest_hit: Option<(f32, u32)> = None;
    let mut consider = |distance_along_ray: f32, handle: u32| {
        let better = match nearest_hit {
            None => true,
            Some((current_distance, _)) => distance_along_ray < current_distance,
        };
        if better {
            nearest_hit = Some((distance_along_ray, handle));
        }
    };
    // Step 1: the handle lines, within the pick radius of the ray.
    for (start, end, _, handle) in &geometry.lines {
        if let Some((distance_along_ray, distance_from_ray)) =
            ray_segment_distance(ray_origin, ray_direction, *start, *end)
        {
            if distance_from_ray <= radius {
                consider(distance_along_ray, *handle);
            }
        }
    }
    // Step 2: the handle triangles, which the ray has to enter.
    for (corners, _, handle) in &geometry.triangles {
        if let Some(distance_along_ray) =
            ray_triangle_intersection(ray_origin, ray_direction, *corners)
        {
            consider(distance_along_ray, *handle);
        }
    }
    nearest_hit.map(|(_, handle)| handle)
}

/// Returns how far along the ray, and how far away from it, the ray comes
/// closest to the segment `start..end`.
///
/// A closest point behind the ray's origin is clamped to the origin. `None`
/// for a zero-length ray direction, or for a zero-length segment that lies
/// behind the origin.
fn ray_segment_distance(
    ray_origin: [f32; 3],
    ray_direction: [f32; 3],
    start: [f32; 3],
    end: [f32; 3],
) -> Option<(f32, f32)> {
    let ray_direction = normalize(ray_direction)?;
    let segment = sub(end, start);
    let segment_length = dot(segment, segment).sqrt();
    if segment_length <= 1e-6 {
        let distance_along_ray = dot(sub(start, ray_origin), ray_direction);
        if distance_along_ray < 0.0 {
            return None;
        }
        let point = add(ray_origin, scale(ray_direction, distance_along_ray));
        return Some((distance_along_ray, distance(point, start)));
    }
    let segment_direction = scale(segment, 1.0 / segment_length);
    let ray_origin_offset = sub(ray_origin, start);
    let ray_direction_length_squared = dot(ray_direction, ray_direction);
    let ray_direction_dot_segment_direction = dot(ray_direction, segment_direction);
    let segment_direction_length_squared = dot(segment_direction, segment_direction);
    let ray_direction_dot_origin_offset = dot(ray_direction, ray_origin_offset);
    let segment_direction_dot_origin_offset = dot(segment_direction, ray_origin_offset);
    let denominator = ray_direction_length_squared * segment_direction_length_squared
        - ray_direction_dot_segment_direction * ray_direction_dot_segment_direction;
    let (distance_along_ray, segment_position) = if denominator.abs() < 1e-6 {
        // Parallel: measure from the ray's own origin.
        (
            0.0f32,
            (segment_direction_dot_origin_offset / segment_direction_length_squared)
                .clamp(0.0, segment_length),
        )
    } else {
        let distance_along_ray = (ray_direction_dot_segment_direction
            * segment_direction_dot_origin_offset
            - segment_direction_length_squared * ray_direction_dot_origin_offset)
            / denominator;
        let segment_position = (ray_direction_length_squared * segment_direction_dot_origin_offset
            - ray_direction_dot_segment_direction * ray_direction_dot_origin_offset)
            / denominator;
        (
            distance_along_ray.max(0.0),
            segment_position.clamp(0.0, segment_length),
        )
    };
    let ray_point = add(ray_origin, scale(ray_direction, distance_along_ray));
    let segment_point = add(start, scale(segment_direction, segment_position));
    Some((distance_along_ray, distance(ray_point, segment_point)))
}

/// Returns how far along the ray it hits the triangle `corners`, using the
/// Möller-Trumbore test. `None` for a miss, or for a triangle behind the ray's
/// origin.
pub fn ray_triangle_intersection(
    ray_origin: [f32; 3],
    ray_direction: [f32; 3],
    corners: [[f32; 3]; 3],
) -> Option<f32> {
    let ray_direction = normalize(ray_direction)?;
    let edge_one = sub(corners[1], corners[0]);
    let edge_two = sub(corners[2], corners[0]);
    let pvec = cross(ray_direction, edge_two);
    let determinant = dot(edge_one, pvec);
    if determinant.abs() < 1e-7 {
        return None;
    }
    let inverse_determinant = 1.0 / determinant;
    let tvec = sub(ray_origin, corners[0]);
    let u = dot(tvec, pvec) * inverse_determinant;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qvec = cross(tvec, edge_one);
    let v = dot(ray_direction, qvec) * inverse_determinant;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance_along_ray = dot(edge_two, qvec) * inverse_determinant;
    (distance_along_ray > 1e-4).then_some(distance_along_ray)
}

// =============================================================================
// Small vector helpers
// =============================================================================

/// Two unit vectors spanning the plane whose normal is `normal`.
pub fn plane_axes(normal: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let normal = normalize(normal).unwrap_or([0.0, 1.0, 0.0]);
    let helper_axis = if normal[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = normalize(cross(helper_axis, normal)).unwrap_or([1.0, 0.0, 0.0]);
    let v = cross(normal, u);
    (u, v)
}

/// `left + right`, per component.
pub fn add(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

/// `left - right`, per component.
pub fn sub(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

/// `vector * factor`.
pub fn scale(vector: [f32; 3], factor: f32) -> [f32; 3] {
    [vector[0] * factor, vector[1] * factor, vector[2] * factor]
}

/// The dot product.
pub fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

/// The cross product.
pub fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

/// `vector` scaled to unit length; `None` for a zero-length vector.
pub fn normalize(vector: [f32; 3]) -> Option<[f32; 3]> {
    let length = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
    if length <= f32::EPSILON {
        return None;
    }
    Some([vector[0] / length, vector[1] / length, vector[2] / length])
}

/// The distance between two points.
pub fn distance(left: [f32; 3], right: [f32; 3]) -> f32 {
    let delta = sub(left, right);
    dot(delta, delta).sqrt()
}

// =============================================================================
// Released drags
// =============================================================================

/// Returns the edits a released drag records, as `(index, value)` pairs in
/// array order.
///
/// `pressed_values` is the field's value at the press and `released_values` is
/// the value the drag wrote. `None` when the two are equal, because a click
/// that moved nothing is not an undo step.
///
/// Any length works, so the transform's arrays are covered too: `translation`
/// and `scale` hold 3 values and `rotation` holds 4.
pub fn release_edits(pressed_values: &[f32], released_values: &[f32]) -> Option<Vec<(usize, f32)>> {
    if pressed_values == released_values {
        return None;
    }
    Some(
        released_values
            .iter()
            .enumerate()
            .map(|(index, value)| (index, *value))
            .collect(),
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The handles of each mode are distinguishable and land on their axis.
    #[test]
    fn every_mode_builds_its_handles() {
        let origin = [1.0, 2.0, 3.0];
        let geometry = build_geometry(GizmoMode::Translate, origin, AXES, 2.0);
        assert_eq!(geometry.lines.len(), 3);
        assert!(!geometry.triangles.is_empty());
        for (axis_index, (start, end, _, handle)) in geometry.lines.iter().enumerate() {
            assert_eq!(*start, origin);
            assert_eq!(*handle, axis_handle(GizmoMode::Translate, axis_index));
            assert!((distance(*end, add(origin, scale(AXES[axis_index], 2.0)))).abs() < 1e-5);
        }
        let rings = build_geometry(GizmoMode::Rotate, origin, AXES, 1.0);
        assert_eq!(
            rings
                .lines
                .iter()
                .filter(|(_, _, _, handle_id)| *handle_id == 11)
                .count(),
            48,
            "the X ring's segments all carry its handle"
        );
        assert!(rings.triangles.is_empty(), "rings carry no triangles");
        let cubes = build_geometry(GizmoMode::Scale, origin, AXES, 1.0);
        assert!(!cubes.triangles.is_empty(), "scale has cube heads");

        // A pick's handle names its mode and axis.
        assert_eq!(handle_axis(2), Some((GizmoMode::Translate, 1)));
        assert_eq!(handle_axis(13), Some((GizmoMode::Rotate, 2)));
        assert_eq!(handle_axis(21), Some((GizmoMode::Scale, 0)));
        assert_eq!(handle_axis(0), None);
        assert_eq!(handle_axis(99), None);
    }

    /// Snapping rounds to the step and a missing step leaves the value.
    #[test]
    fn snapping_rounds_to_the_step() {
        assert_eq!(snap_value(0.74, 0.5), 0.5);
        assert_eq!(snap_value(0.9, 0.5), 1.0);
        assert_eq!(snap_value(-0.74, 0.5), -0.5);
        assert_eq!(snap_value(7.3, 0.0), 7.3);
        assert_eq!(snap_value(16.0, 15.0), 15.0);
    }

    /// The closest point on an axis to a ray is where the drag reads its
    /// position from.
    #[test]
    fn the_axis_follows_the_cursor_ray() {
        // Straight down through x = 3: the closest point on X is 3.
        let distance_along_axis = closest_axis_point(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [3.0, 10.0, 0.0],
            [0.0, -1.0, 0.0],
        )
        .expect("not parallel");
        assert!(
            (distance_along_axis - 3.0).abs() < 1e-4,
            "t = {distance_along_axis}"
        );
        // A ray parallel to the axis has no closest point.
        assert!(closest_axis_point(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 5.0, 0.0],
            [1.0, 0.0, 0.0],
        )
        .is_none());
    }

    /// The rotation plane answers with the angle around the ring's axis.
    #[test]
    fn the_ring_angle_reads_the_plane() {
        // `plane_axes` gives the Y ring a basis that starts at +Z, so the +Z
        // point is angle zero and +X is a quarter turn later. The basis decides
        // the sign of that quarter turn; only its size is checked here.
        let axis = [0.0, 1.0, 0.0];
        let start_angle = plane_angle([0.0, 0.0, 0.0], axis, [0.0, 0.0, 1.0]);
        assert!(start_angle.abs() < 1e-4, "start = {start_angle}");
        let quarter_angle = plane_angle([0.0, 0.0, 0.0], axis, [1.0, 0.0, 0.0]);
        assert!(
            (quarter_angle.abs() - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "quarter = {quarter_angle}"
        );
        // Crossing the seam keeps turning the short way.
        assert!((wrap_angle(350.0_f32.to_radians()) + 10.0_f32.to_radians()).abs() < 1e-4);
        // A ray straight down hits the horizontal plane; a parallel one does
        // not.
        assert!(ray_plane_point(
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [4.0, 9.0, 5.0],
            [0.0, -1.0, 0.0]
        )
        .is_some());
        assert!(ray_plane_point(
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [4.0, 9.0, 5.0],
            [1.0, 0.0, 0.0]
        )
        .is_none());
    }

    /// A local gizmo's axes are the entity's own, rotated.
    #[test]
    fn local_axes_follow_the_entities_rotation() {
        // 180 degrees around Y: the entity's X points along -X.
        let rotated = rotate_vector([0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert!((rotated[0] + 1.0).abs() < 1e-4, "rotated = {rotated:?}");
        // The identity leaves the axes alone.
        assert_eq!(
            rotate_vector([0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            [0.0, 1.0, 0.0]
        );
        // A quarter turn around Y, built from the angle: X goes to -Z.
        let quarter_turn = axis_angle_quaternion([0.0, 1.0, 0.0], std::f32::consts::FRAC_PI_2);
        let turned_vector = rotate_vector(quarter_turn, [1.0, 0.0, 0.0]);
        assert!(
            turned_vector[2] + 1.0 < 1e-4 && turned_vector[2] + 1.0 > -1e-4,
            "turned = {turned_vector:?}"
        );
        // A zero axis is the identity, not a NaN.
        let identity_quaternion = axis_angle_quaternion([0.0; 3], 1.0);
        assert_eq!(identity_quaternion, [0.0, 0.0, 0.0, 1.0]);
    }

    /// The translation tool carries one square per axis pair, and its handle
    /// ids name the pair.
    #[test]
    fn the_translation_tool_builds_three_plane_squares() {
        let origin = [0.0, 0.0, 0.0];
        let length = 2.0;
        let geometry = build_geometry(GizmoMode::Translate, origin, AXES, length);
        // Three axes plus three planes, two triangles per plane.
        let plane_handles: Vec<u32> = (4..=6).collect();
        for handle in plane_handles {
            let triangle_count = geometry
                .triangles
                .iter()
                .filter(|(_, _, id)| *id == handle)
                .count();
            assert_eq!(triangle_count, 2, "plane {handle} is one square");
        }
        // The XY square sits at the offset along +X and +Y.
        let expected_offset = length * PLANE_OFFSET;
        let xy_triangle = geometry
            .triangles
            .iter()
            .find(|(_, _, id)| *id == plane_handle(0, 1))
            .expect("the XY square exists");
        // The first triangle's corners 0 and 2 are the square's diagonal.
        let center = [
            (xy_triangle.0[0][0] + xy_triangle.0[2][0]) * 0.5,
            (xy_triangle.0[0][1] + xy_triangle.0[2][1]) * 0.5,
            (xy_triangle.0[0][2] + xy_triangle.0[2][2]) * 0.5,
        ];
        assert!(center[0] > 0.0 && center[1] > 0.0, "center {center:?}");
        assert!(
            (center[0] - expected_offset).abs() < 1e-4
                && (center[1] - expected_offset).abs() < 1e-4,
            "center {center:?}"
        );
        assert!(center[2].abs() < 1e-5, "the XY square stays at z = 0");
        // The plane handles resolve back to their axis pairs.
        assert_eq!(
            handle_kind(plane_handle(1, 2)),
            Some(HandleKind::Plane {
                first: 1,
                second: 2
            })
        );
        assert_eq!(handle_kind(4).and_then(HandleKind::axis), None);
        assert_eq!(
            handle_kind(2).and_then(HandleKind::axis),
            Some((GizmoMode::Translate, 1))
        );
    }

    /// The element band and the gizmo's own ids never overlap, in either
    /// direction: a picked id can only mean one thing.
    #[test]
    fn element_handles_and_gizmo_handles_never_overlap() {
        for handle in [1_u32, 2, 3, 4, 5, 6, 11, 12, 13, 21, 22, 23] {
            assert!(handle_kind(handle).is_some(), "{handle} is a gizmo handle");
            assert_eq!(
                element_handle_slot(handle),
                None,
                "{handle} must not read as an element handle"
            );
        }
        for index in [0_u32, 1, 41, ELEMENT_HANDLE_LIMIT - 1] {
            let handle = element_handle(index);
            assert_eq!(element_handle_slot(handle), Some(index as usize));
            assert_eq!(
                handle_kind(handle),
                None,
                "{handle} must not read as a gizmo handle"
            );
        }
        assert_eq!(element_handle_slot(ELEMENT_HANDLE_BASE - 1), None);
        assert_eq!(
            element_handle_slot(ELEMENT_HANDLE_BASE + ELEMENT_HANDLE_LIMIT),
            None
        );
    }

    /// The frame's map answers the handles it handed out and nothing else.
    #[test]
    fn the_element_map_answers_its_own_handles() {
        let mut world = pill_engine::World::default();
        let first = world.create_entity().build().expect("entity builds");
        let second = world.create_entity().build().expect("entity builds");
        let mut map = ElementHandleMap::new();
        assert!(map.is_empty());
        let first_handle = map.push(ElementHandleTarget {
            entity: first,
            element: 4,
            kind: "point".to_string(),
        });
        let second_handle = map.push(ElementHandleTarget {
            entity: second,
            element: 9,
            kind: "roll".to_string(),
        });
        assert_eq!(first_handle, ELEMENT_HANDLE_BASE);
        assert_eq!(second_handle, ELEMENT_HANDLE_BASE + 1);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.target(first_handle)
                .map(|target| (target.entity, target.element)),
            Some((first, 4))
        );
        assert_eq!(
            map.target(second_handle).map(|target| target.kind.as_str()),
            Some("roll")
        );
        assert_eq!(map.target(7), None, "a gizmo id is not an element handle");
        assert_eq!(
            map.target(second_handle + 1),
            None,
            "a slot past the frame's report is empty"
        );
    }

    /// A hovered handle brightens, and a handle being dragged dims below its
    /// normal colour.
    #[test]
    fn handle_states_brighten_then_dim() {
        let base_color = AXIS_COLORS[0];
        let states = HandleStates {
            hovered: Some(1),
            active: None,
        };
        let hovered_color = styled_color(base_color, 1, states);
        assert!(
            hovered_color[0] > base_color[0]
                || hovered_color[1] > base_color[1]
                || hovered_color[2] > base_color[2]
        );
        let active_states = HandleStates {
            hovered: Some(1),
            active: Some(1),
        };
        let dragging_color = styled_color(base_color, 1, active_states);
        assert!(
            dragging_color[0] < base_color[0],
            "the active handle dims: {dragging_color:?}"
        );
        assert!(
            dragging_color[0] < hovered_color[0],
            "active is dimmer than hovered: {dragging_color:?} vs {hovered_color:?}"
        );
        // Other handles keep their normal look.
        assert_eq!(styled_color(base_color, 2, states), base_color);
        // A plane handle's colour is its axes' average, translucent.
        let plane_square_color = plane_color(0, 2);
        assert!(plane_square_color[3] < 1.0, "plane squares are translucent");
        assert!(
            (plane_square_color[0] - (AXIS_COLORS[0][0] + AXIS_COLORS[2][0]) * 0.5).abs() < 1e-5
        );
    }

    /// A scale handle follows the scale factor, clamped at both ends.
    #[test]
    fn scale_handles_follow_the_scale_factor() {
        let reference_length = 2.0;
        let original_scale = [1.0, 1.0, 1.0];
        let grown = scale_handle_lengths(reference_length, original_scale, [2.0, 1.0, 1.0], 0);
        assert!((grown[0] - 4.0).abs() < 1e-5, "grown {grown:?}");
        assert!((grown[1] - reference_length).abs() < 1e-5, "others stay");
        let shrunk = scale_handle_lengths(reference_length, original_scale, [0.5, 1.0, 1.0], 0);
        assert!((shrunk[0] - 1.0).abs() < 1e-5, "shrunk {shrunk:?}");
        // The clamps hold against extreme and degenerate scales.
        let huge = scale_handle_lengths(reference_length, original_scale, [1000.0, 1.0, 1.0], 0);
        assert!((huge[0] - reference_length * SCALE_HANDLE_MAX_RATIO).abs() < 1e-5);
        let tiny = scale_handle_lengths(reference_length, original_scale, [0.0001, 1.0, 1.0], 0);
        assert!((tiny[0] - reference_length * SCALE_HANDLE_MIN_RATIO).abs() < 1e-5);
        let zero_original =
            scale_handle_lengths(reference_length, [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], 0);
        assert!((zero_original[0] - reference_length).abs() < 1e-5);
        let not_finite =
            scale_handle_lengths(reference_length, original_scale, [f32::NAN, 1.0, 1.0], 0);
        assert!((not_finite[0] - reference_length).abs() < 1e-5);
    }

    /// The guides: a symmetric axis line and evenly spaced dashes.
    #[test]
    fn guides_span_both_directions_and_dash_evenly() {
        let (start, end) = axis_guide([1.0, 0.0, 0.0], AXES[1], 5.0);
        assert_eq!(start, [1.0, -5.0, 0.0]);
        assert_eq!(end, [1.0, 5.0, 0.0]);
        let dashes = dotted_segments([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], 1.0);
        assert_eq!(dashes.len(), 5, "dash and gap are equal: {dashes:?}");
        assert_eq!(dashes[0].0, [0.0, 0.0, 0.0]);
        assert_eq!(dashes.last().expect("a dash").1, [10.0, 0.0, 0.0]);
        for (from, to) in &dashes[..dashes.len() - 1] {
            assert!((distance(*from, *to) - 1.0).abs() < 1e-4, "dash length");
        }
        // The last dash reaches the end, so it may run up to twice as long.
        let last_dash = dashes.last().expect("a dash");
        let last_length = distance(last_dash.0, last_dash.1);
        assert!(
            (1.0..=2.0 + 1e-4).contains(&last_length),
            "last dash {last_length}"
        );
        // A very long line is capped rather than building thousands of dashes.
        let capped_dashes = dotted_segments([0.0, 0.0, 0.0], [10_000.0, 0.0, 0.0], 0.01);
        assert_eq!(capped_dashes.len(), DOTTED_SEGMENTS_MAX);
        assert!(dotted_segments([0.0; 3], [0.0; 3], 1.0).is_empty());
    }

    /// The hover test agrees with the geometry: lines within the radius and
    /// triangles on the ray both answer their handle.
    #[test]
    fn hit_testing_finds_lines_planes_and_nothing() {
        let origin = [0.0, 0.0, 0.0];
        let geometry = build_geometry(GizmoMode::Translate, origin, AXES, 2.0);
        // A ray from above, aimed at the X axis a little past the origin.
        let hit = hit_test(&geometry, [1.0, 5.0, 0.0], [0.0, -1.0, 0.0], 0.05);
        assert_eq!(hit, Some(axis_handle(GizmoMode::Translate, 0)));
        // Straight down through the XY square, away from any line.
        let plane_offset = 2.0 * PLANE_OFFSET;
        let plane_hit = hit_test(
            &geometry,
            [plane_offset, plane_offset, 5.0],
            [0.0, 0.0, -1.0],
            0.02,
        );
        assert_eq!(plane_hit, Some(plane_handle(0, 1)), "the XY square answers");
        // A ray into empty space finds nothing.
        let missed_hit = hit_test(&geometry, [10.0, 10.0, 10.0], [0.0, 1.0, 0.0], 0.05);
        assert_eq!(missed_hit, None);
    }

    /// A released drag records one step, and only when it moved.
    ///
    /// The press's value on its own is not an undo entry.
    #[test]
    fn a_release_records_one_step_only_when_it_moved() {
        let pressed_values = [1.0, 2.0, 3.0];
        assert_eq!(release_edits(&pressed_values, &pressed_values), None);
        let released_values = [1.0, 2.0, 4.5];
        assert_eq!(
            release_edits(&pressed_values, &released_values),
            Some(vec![(0, 1.0), (1, 2.0), (2, 4.5)]),
            "every element of the field is written, in array order"
        );
        // A rotation is four wide; the same rule covers it.
        let released_rotation = [0.0, 0.0, 0.0, 1.0];
        assert_eq!(
            release_edits(&[0.0; 4], &released_rotation).map(|edits| edits.len()),
            Some(4)
        );
    }

    /// Every row of the field table reads, writes and round-trips.
    ///
    /// A value of the wrong kind is refused and leaves the field alone, and the
    /// Inspector's entry uses the same table in the same order.
    #[test]
    fn every_resource_field_reads_writes_and_round_trips() {
        let resource_entry = crate::resources::EDITABLE_RESOURCES
            .iter()
            .find(|entry| entry.name == RESOURCE_NAME)
            .expect("the gizmo settings are an editable resource");
        let inspector_field_names: Vec<&str> =
            resource_entry.fields.iter().map(|row| row.name).collect();
        let table_field_names: Vec<&str> = RESOURCE_FIELDS.iter().map(|row| row.name).collect();
        assert_eq!(
            inspector_field_names, table_field_names,
            "the Inspector offers the table's fields in order"
        );

        for field_row in RESOURCE_FIELDS {
            let mut settings = GizmoSettings::default();
            let before = (field_row.read)(&settings);
            // A value the kind accepts and that differs from the current one.
            let changed_value = match &before {
                FieldValue::Bool(value) => FieldValue::Bool(!*value),
                FieldValue::F32(value) => FieldValue::F32(value + 1.0),
                FieldValue::U32(value) => FieldValue::U32(u32::from(*value == 0)),
                other => panic!("{} reads an unexpected kind {other:?}", field_row.name),
            };
            let previous =
                set_resource_field(&mut settings, RESOURCE_NAME, field_row.name, &changed_value)
                    .unwrap_or_else(|error| {
                        panic!("{} refused its own kind: {error}", field_row.name)
                    });
            assert_eq!(
                previous, before,
                "{} answers the value it replaced, for the inverse",
                field_row.name
            );
            assert_eq!(
                (field_row.read)(&settings),
                changed_value,
                "{}",
                field_row.name
            );
            assert_eq!(
                resource_field_value(&settings, RESOURCE_NAME, field_row.name),
                Ok(changed_value.clone()),
                "{} reads back what the write took",
                field_row.name
            );
            set_resource_field(&mut settings, RESOURCE_NAME, field_row.name, &before)
                .unwrap_or_else(|error| {
                    panic!("{} could not be put back: {error}", field_row.name)
                });
            assert_eq!(
                (field_row.read)(&settings),
                before,
                "{} round-trips",
                field_row.name
            );

            // A kind the field cannot hold is refused, and changes nothing.
            let refused_value = match &before {
                FieldValue::Bool(_) => FieldValue::U32(1),
                _ => FieldValue::Bool(true),
            };
            assert!(
                set_resource_field(&mut settings, RESOURCE_NAME, field_row.name, &refused_value)
                    .is_err(),
                "{} accepted {refused_value:?}",
                field_row.name
            );
            assert_eq!(
                (field_row.read)(&settings),
                before,
                "{} changed on a refused write",
                field_row.name
            );
        }
    }
}
