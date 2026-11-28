//! A scene: the entities of a world, the components that can be saved and
//! their references, written as one standalone asset in `res`.
//!
//! # Responsibilities
//!
//! - [`SceneDocument`]: what a `.scene` file holds - one entry per entity, its
//!   stable [`EntityId`], and its components by registered type name, each a
//!   map of field name to the value the field layer reads.
//! - [`save_scene`]: a world's entities as that document, with references in
//!   their saved form.
//! - [`load_scene`]: the document back into a world - entities created under
//!   the ids it carries, components re-attached by name, asset references
//!   loaded on demand and entity references resolved once every entity exists.
//! - [`Scene`]: the asset type itself, so a scene can be named by a handle and
//!   loaded by its `.scene` extension.
//!
//! # Design
//!
//! The field layer - the engine code that reads and writes a component's
//! fields by name - writes the document, not each component's own serde. A
//! handle field is saved as its guid and an entity field as its stable id. A
//! component that serializes a handle as a slot index and a generation would
//! write numbers that mean nothing in a later run, so the field layer, which
//! knows what a reference is, is the writer.
//!
//! A field the field layer can describe but not read out (a `vec:` or
//! `dynbuf:` container, or a `string`) is not written, and
//! [`SceneSave::skipped_fields`] names it, so a save that loses data says so
//! instead of looking complete. A field it can find but not interpret (a
//! `struct:` with no known layout) is saved as its raw bytes and written back
//! unchanged. A whole component it cannot reach - one it cannot resolve, one
//! with no layout, or a row it cannot read - is left out and named in
//! [`SceneSave::skipped_components`], which the caller has to report.
//!
//! Loading runs four passes in this order: create every entity, attach every
//! component, write every field, then resolve the entity references. A
//! component and its fields must exist before a reference is written into
//! them. An entity reference may also name an entity that comes later in the
//! file, so the last pass waits until every entity exists.

// Standard library
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

// External crates
use serde::{Deserialize, Serialize};
use trait_type_map::impl_trait_accessible;

// Current crate
use crate::asset::{Asset, AssetLoadResult, AssetManager};
use crate::asset_database::AssetDatabase;
use crate::asset_import_registry::ImportRegistry;
use crate::asset_standalone::StandaloneAsset;
use crate::commands::{ByteComponentAdder, CommandError};
use crate::component_field::{is_handle_tag, parse_enum_tag, EnumFieldTag, FieldValue};
use crate::engine::Engine;
use crate::entity::{Entity, EntityId};
use crate::saved_references::{
    restore_asset_reference, saved_reference, SavedAssetReference, SavedEntityLinks, SavedReference,
};
use crate::world::{ChangeOrigin, World};

// =============================================================================
// The document
// =============================================================================

/// One entity in a scene document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneEntity {
    /// The entity's stable id. A reference to it in the same document names
    /// this id.
    pub id: EntityId,
    /// Its components by registered type name, each a map of field name to the
    /// value the field layer reads.
    ///
    /// A name written before type names dropped their module path is read in
    /// its current form (`crate::type_names::crate_qualified_name`), so an
    /// older scene still finds its components.
    #[serde(default, deserialize_with = "deserialize_component_map")]
    pub components: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
}

/// One entity's components as a scene document stores them: field values by
/// field name, by component name.
type ComponentMap = BTreeMap<String, BTreeMap<String, serde_json::Value>>;

/// Reads an entity's component map with every component name in its current
/// form. When an older and a current spelling of one component both appear,
/// the first in name order is kept.
fn deserialize_component_map<'de, D>(deserializer: D) -> Result<ComponentMap, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let stored = ComponentMap::deserialize(deserializer)?;
    let mut components = ComponentMap::new();
    for (name, fields) in stored {
        let current_name = crate::type_names::crate_qualified_name(&name).into_owned();
        components.entry(current_name).or_insert(fields);
    }
    Ok(components)
}

/// The body of a `.scene` file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneDocument {
    /// Every entity the scene holds.
    pub entities: Vec<SceneEntity>,
}

/// A scene asset. It holds the document it was loaded from, and components
/// can name it with a `Handle<Scene>`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// The document the file held.
    document: SceneDocument,
}

impl Scene {
    /// The document this scene was loaded from.
    #[must_use]
    pub fn document(&self) -> &SceneDocument {
        &self.document
    }
}

/// The shared name of [`Scene`], written as a literal: the key the import
/// registry files the type under, and the name a `Handle<Scene>` field records
/// as its asset type. It follows the `<crate>::<Type>` rule under the public
/// crate name, and a file written with the older `pill_engine::scene::Scene`
/// still reads as this type.
const SCENE_SHARED_NAME: &str = "pill_engine::Scene";

// Shared identity: each binary that compiles this type gets its own `TypeId`
// for it, and the shared name is what gives the copies one asset column
// instead of one each. Keep the name verbatim when moving the type.
impl Asset for Scene {
    fn shared_name() -> Option<&'static str> {
        Some(SCENE_SHARED_NAME)
    }
}

impl_trait_accessible!(dyn Asset; Scene);

impl StandaloneAsset for Scene {
    type Document = SceneDocument;

    const FILE_EXTENSION: &'static str = "scene";

    fn metadata_type_name() -> &'static str {
        SCENE_SHARED_NAME
    }

    fn from_document(
        _name: &str,
        document: SceneDocument,
        _assets: &AssetManager,
    ) -> AssetLoadResult<Self> {
        Ok(Self { document })
    }
}

// =============================================================================
// Saving
// =============================================================================

/// What [`save_scene`] wrote, and what it could not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SceneSave {
    /// One entry per field the field layer could not write, by
    /// `<entity>: <component>.<field>`.
    pub skipped_fields: Vec<String>,
    /// One entry per component the save left out, naming the entity, the
    /// component and the reason. The save writes components through the field
    /// layer, so a component with no field layout has nothing to write. The
    /// entity is then written without that component, which is a loss the
    /// caller has to see.
    pub skipped_components: Vec<String>,
    /// How many component entries the document holds.
    pub components: usize,
}

/// Writes a world's entities into a scene document.
///
/// `entities` names the entities to write, and `None` writes every live
/// entity. A component is written when the field layer can read it, which is
/// also what a load needs to write it back. Persistability is not required:
/// it is the serde registration that migrates a type's rows across a reload.
/// A component holding an `Entity` - the very thing a scene exists to carry -
/// cannot be serde-serialized at all.
pub fn save_scene(
    world: &World,
    assets: &AssetManager,
    registry: &ImportRegistry,
    entities: Option<&[Entity]>,
) -> (SceneDocument, SceneSave) {
    // Step 1: take the chosen entities, or every live entity.
    let entities: Vec<Entity> = match entities {
        Some(chosen) => chosen.to_vec(),
        None => world
            .entity_rows()
            .into_iter()
            .map(|row| row.entity)
            .collect(),
    };
    // Step 2: write each entity's components, field by field, and record what
    // the field layer could not write.
    let mut document = SceneDocument::default();
    let mut report = SceneSave::default();
    for entity in entities {
        let Some(id) = world.entity_id(entity) else {
            continue;
        };
        let mut components = BTreeMap::new();
        for component_name in world.entity_component_names(entity).unwrap_or_default() {
            let Some(component_id) = world.resolve_entity_component_id(entity, &component_name)
            else {
                report.skipped_components.push(format!(
                    "{id}: {component_name} is not registered on this entity"
                ));
                continue;
            };
            let Some(layout) = world.component_field_layout(component_id) else {
                report
                    .skipped_components
                    .push(format!("{id}: {component_name} has no field layout"));
                continue;
            };
            let field_values = match world.read_component_fields(entity, &component_name) {
                Ok(field_values) => field_values,
                Err(_) => {
                    report
                        .skipped_components
                        .push(format!("{id}: {component_name} cannot be read"));
                    continue;
                }
            };
            let mut fields = BTreeMap::new();
            for (descriptor, field_value) in layout.iter().zip(field_values.iter()) {
                match encode_field_value(world, assets, registry, field_value) {
                    Some(encoded_value) => {
                        fields.insert(descriptor.name.to_string(), encoded_value);
                    }
                    None => report
                        .skipped_fields
                        .push(format!("{id}: {component_name}.{}", descriptor.name)),
                }
            }
            report.components += 1;
            components.insert(component_name, fields);
        }
        document.entities.push(SceneEntity { id, components });
    }
    (document, report)
}

/// Returns one field value the way a scene document stores it, or `None` when
/// it has no saved form. That is a heap container, which the field layer
/// cannot read out, and a float JSON cannot hold (NaN or an infinity).
///
/// `Text` and `List` both return `None` for the same reason: the field layer
/// cannot write a heap-backed field, so a value saved here would never reach
/// a component on load.
fn encode_field_value(
    world: &World,
    assets: &AssetManager,
    registry: &ImportRegistry,
    field_value: &FieldValue,
) -> Option<serde_json::Value> {
    /// Returns `value` as a JSON number, or `None` for NaN and the infinities.
    fn json_number(value: f64) -> Option<serde_json::Value> {
        serde_json::Number::from_f64(value).map(serde_json::Value::Number)
    }
    match field_value {
        FieldValue::F32(value) => json_number(f64::from(*value)),
        FieldValue::F64(value) => json_number(*value),
        FieldValue::I8(value) => Some(serde_json::json!(value)),
        FieldValue::I16(value) => Some(serde_json::json!(value)),
        FieldValue::I32(value) => Some(serde_json::json!(value)),
        FieldValue::I64(value) => Some(serde_json::json!(value)),
        FieldValue::U8(value) => Some(serde_json::json!(value)),
        FieldValue::U16(value) => Some(serde_json::json!(value)),
        FieldValue::U32(value) => Some(serde_json::json!(value)),
        FieldValue::U64(value) => Some(serde_json::json!(value)),
        FieldValue::Usize(value) => Some(serde_json::json!(value)),
        FieldValue::Isize(value) => Some(serde_json::json!(value)),
        FieldValue::Bool(value) => Some(serde_json::json!(value)),
        FieldValue::Array(elements) => {
            let mut encoded_elements = Vec::with_capacity(elements.len());
            for element in elements {
                encoded_elements.push(encode_field_value(world, assets, registry, element)?);
            }
            Some(serde_json::Value::Array(encoded_elements))
        }
        // An enum is saved as its variant's name, so a document survives a
        // reordering of the declaration. A discriminant no variant declares
        // is kept as its number rather than lost.
        FieldValue::Enum { discriminant, .. } => Some(match field_value.enum_variant_name() {
            Some(variant_name) => serde_json::json!(variant_name),
            None => serde_json::json!(discriminant),
        }),
        FieldValue::Opaque { type_tag, bytes } => Some(serde_json::json!({
            "opaque": hex_of(bytes),
            "tag": type_tag,
        })),
        FieldValue::Text(_) | FieldValue::List { .. } => None,
        FieldValue::Asset { .. } | FieldValue::Entity { .. } => {
            match saved_reference(world, assets, registry, field_value) {
                Some(SavedReference::Asset(reference)) => serde_json::to_value(reference).ok(),
                Some(SavedReference::Entity(id)) => Some(serde_json::json!(id.to_string())),
                // An unset reference is written as `null`, and a load reads
                // that back as the cleared value. A set reference the writer
                // cannot name (an asset with no guid) is left out, and the
                // caller reports the loss.
                None if !is_set_reference(field_value) => Some(serde_json::Value::Null),
                None => None,
            }
        }
    }
}

/// Returns whether `field_value` names an asset or an entity. Only the two
/// reference variants can.
fn is_set_reference(field_value: &FieldValue) -> bool {
    match field_value {
        FieldValue::Asset { .. } | FieldValue::Entity { .. } => !field_value.references_nothing(),
        _ => false,
    }
}

/// Returns `bytes` as lowercase hex, the form an opaque payload is written in.
fn hex_of(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing into a `String` cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// Returns the bytes `text` spells as hex, or `None` when it is not hex.
fn bytes_of(text: &str) -> Option<Vec<u8>> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.as_chunks::<2>().0 {
        let pair = std::str::from_utf8(pair).ok()?;
        bytes.push(u8::from_str_radix(pair, 16).ok()?);
    }
    Some(bytes)
}

// =============================================================================
// Loading
// =============================================================================

/// What [`load_scene`] made of a document.
///
/// Each list below records one kind of loss:
///
/// - [`unknown_components`](Self::unknown_components): the world knows no
///   component of that name, so nothing could be attached.
/// - [`failed_components`](Self::failed_components): the world knows the type
///   but could not attach it - the reason is the field layer's or a command's
///   own message.
/// - [`skipped_entities`](Self::skipped_entities): a document entry was dropped
///   because its id repeats or is already taken.
/// - [`skipped_fields`](Self::skipped_fields): a field could not be written,
///   by `<entity>: <component>.<field>`.
/// - [`unresolved_references`](Self::unresolved_references): an entity
///   reference was not written into its field, because the id names no entity
///   of the document or the field refused the write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SceneLoad {
    /// Entities created.
    pub entities: usize,
    /// Components attached, or already present on the entity.
    pub components: usize,
    /// Component type names the world does not know.
    pub unknown_components: Vec<String>,
    /// Components that failed to attach although the type is registered, each
    /// entry naming the entity, the component and the reason.
    pub failed_components: Vec<String>,
    /// Entity references that were not written, because the id names no
    /// entity of the document or the field refused the write.
    pub unresolved_references: usize,
    /// Document entries whose entity was not created, with the reason.
    pub skipped_entities: Vec<String>,
    /// Fields that could not be written, by `<entity>: <component>.<field>`.
    pub skipped_fields: Vec<String>,
}

/// Instantiates a scene document into a world.
///
/// The entities are created under the ids the document carries, so a reference
/// between them resolves. A component the world does not know is reported and
/// skipped. A field the layout does not have, or a value the field layer
/// refuses, is reported the same way.
pub fn load_scene(
    engine: &mut Engine,
    database: &AssetDatabase,
    document: &SceneDocument,
) -> SceneLoad {
    // Every attachment a load makes reports the load as its cause, whichever
    // caller asked for it.
    engine.with_change_origin(ChangeOrigin::SceneLoad, |engine| {
        load_scene_in_current_origin(engine, database, document)
    })
}

/// [`load_scene`], with the caller's change origin already in place.
fn load_scene_in_current_origin(
    engine: &mut Engine,
    database: &AssetDatabase,
    document: &SceneDocument,
) -> SceneLoad {
    let mut report = SceneLoad::default();
    let registry = engine
        .world()
        .get_resource::<ImportRegistry>()
        .cloned()
        .unwrap_or_default();
    // Step 1: create every entity under the id the document carries, so a
    // reference can resolve to it later.
    let mut created_entities: Vec<(Entity, &SceneEntity)> =
        Vec::with_capacity(document.entities.len());
    let mut seen_ids = HashSet::with_capacity(document.entities.len());
    for scene_entity in &document.entities {
        if !seen_ids.insert(scene_entity.id) {
            report
                .skipped_entities
                .push(format!("{}: the id repeats in this scene", scene_entity.id));
            continue;
        }
        match engine
            .world_mut()
            .create_entity()
            .with_id(scene_entity.id)
            .build()
        {
            Ok(entity) => {
                report.entities += 1;
                created_entities.push((entity, scene_entity));
            }
            Err(_) => report
                .skipped_entities
                .push(format!("{}: the id is already taken", scene_entity.id)),
        }
    }

    // Step 2: attach every component, so its fields can be written into it.
    let mut freshly_attached: HashSet<(Entity, String)> = HashSet::new();
    for (entity, scene_entity) in &created_entities {
        for component_name in scene_entity.components.keys() {
            match attach_component(engine, *entity, component_name) {
                Ok(attached) => {
                    report.components += 1;
                    if attached {
                        freshly_attached.insert((*entity, component_name.clone()));
                    }
                }
                Err(AttachError::UnknownType) => {
                    report.unknown_components.push(component_name.clone());
                }
                Err(failure) => report.failed_components.push(format!(
                    "{entity}: {component_name} could not attach: {failure}"
                )),
            }
        }
    }

    // Step 3: write every field. Entity references wait for their entity.
    let mut links = SavedEntityLinks::default();
    for (entity, scene_entity) in &created_entities {
        for (component_name, fields) in &scene_entity.components {
            // The tags are copied out first, because the field loop below
            // writes through `engine.world_mut()`, which cannot run while the
            // layout borrows the world.
            let tags: BTreeMap<String, &'static str> = {
                let world = engine.world();
                let Some(component_id) = world.resolve_entity_component_id(*entity, component_name)
                else {
                    continue;
                };
                let Some(layout) = world.component_field_layout(component_id) else {
                    continue;
                };
                layout
                    .iter()
                    .map(|row| (row.name.to_string(), row.type_tag))
                    .collect()
            };
            // A freshly attached component is a zero-filled image, and a zeroed
            // reference points at slot 0, which can hold a real entity or
            // asset. So every reference the document does not name is cleared
            // explicitly.
            if freshly_attached.contains(&(*entity, component_name.clone())) {
                for (field_name, tag) in &tags {
                    if fields.contains_key(field_name.as_str()) {
                        continue;
                    }
                    if !is_handle_tag(tag) && tag != &"entity" {
                        continue;
                    }
                    if let Err(error) = engine.world_mut().write_component_field(
                        *entity,
                        component_name,
                        field_name,
                        cleared_reference_value(tag),
                    ) {
                        report.skipped_fields.push(format!(
                            "{entity}: {component_name}.{field_name} was refused: {error}"
                        ));
                    }
                }
            }
            for (field_name, encoded_value) in fields {
                let Some(tag) = tags.get(field_name.as_str()).copied() else {
                    report.skipped_fields.push(format!(
                        "{entity}: {component_name}.{field_name} is not in the layout"
                    ));
                    continue;
                };
                let field_value = match decoded_field(tag, encoded_value) {
                    Ok(DecodedField::Plain(field_value)) => field_value,
                    Ok(DecodedField::Asset(saved_asset_reference)) => {
                        let assets = engine
                            .world_mut()
                            .get_resource_mut::<AssetManager>()
                            .expect("a world with an asset manager");
                        restore_asset_reference(assets, database, &registry, &saved_asset_reference)
                    }
                    Ok(DecodedField::Entity(id)) => {
                        links.push(*entity, component_name, field_name, id);
                        continue;
                    }
                    Ok(DecodedField::Cleared) => cleared_reference_value(tag),
                    Err(reason) => {
                        report
                            .skipped_fields
                            .push(format!("{entity}: {component_name}.{field_name} {reason}"));
                        continue;
                    }
                };
                if let Err(error) = engine.world_mut().write_component_field(
                    *entity,
                    component_name,
                    field_name,
                    field_value,
                ) {
                    report.skipped_fields.push(format!(
                        "{entity}: {component_name}.{field_name} was refused: {error}"
                    ));
                }
            }
        }
    }

    // Step 4: write the entity references, once every entity of the document
    // exists.
    let (_, unresolved_links) = links.resolve(engine.world_mut());
    report.unresolved_references = unresolved_links.len();
    report
}

/// Why a scene document's component could not be attached.
#[derive(Debug)]
enum AttachError {
    /// The world knows no component of that name.
    UnknownType,
    /// The world knows the type but could not attach it. The message is the
    /// field layer's or a command's own reason.
    Failed(String),
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownType => formatter.write_str("the world knows no component of this name"),
            Self::Failed(reason) => formatter.write_str(reason),
        }
    }
}

/// Attaches `component_name` to `entity` with a zero-filled image, and reports
/// whether the component was newly attached.
///
/// # Errors
///
/// [`AttachError::UnknownType`] when the world has no component of that name,
/// or when two registrations claim it and there is no single one to attach.
/// [`AttachError::Failed`] when the world knows the type but the image cannot
/// be built or the add command is refused, carrying that layer's own reason.
fn attach_component(
    engine: &mut Engine,
    entity: Entity,
    component_name: &str,
) -> Result<bool, AttachError> {
    // Step 1: a component the entity already carries needs no add. Its fields
    // are written all the same, but its references are not the zero-filled
    // ones, so they are left alone.
    if engine
        .world()
        .resolve_entity_component_id(entity, component_name)
        .is_some()
    {
        return Ok(false);
    }
    // Step 2: resolve the type before building the image, so "unknown" and
    // "known but unbuildable" stay separate answers.
    let component_id = engine
        .world()
        .resolve_component_id_by_name_any(component_name)
        .map_err(|_| AttachError::UnknownType)?
        .ok_or(AttachError::UnknownType)?;
    let component_image = engine
        .world()
        .build_component_image(component_name, &[])
        .map_err(|error| AttachError::Failed(error.to_string()))?;
    // Step 3: queue the add and apply it now, so the field writes that follow
    // find the new row.
    engine.queue_deferred_commands(move |_world, queue| {
        if component_id.is_native_storage() {
            queue.add_component_adder_to_entity(
                entity,
                Box::new(ByteComponentAdder::new(component_id, component_image)),
            );
        } else {
            queue.add_descriptor_component_to_entity(entity, component_id, component_image);
        }
    });
    engine
        .flush_deferred_commands()
        .map_err(|errors| AttachError::Failed(join_command_errors(errors)))?;
    Ok(true)
}

/// Renders a failed command batch as one reason string.
fn join_command_errors(errors: Vec<CommandError>) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// What one document value means for the field it is written into.
enum DecodedField {
    /// A plain value: written into the field as it is.
    Plain(FieldValue),
    /// An asset reference to resolve against the world's assets.
    Asset(SavedAssetReference),
    /// An entity reference to resolve after every entity exists.
    Entity(EntityId),
    /// A `null` in a reference field, meaning the cleared handle.
    Cleared,
}

/// Decodes one document value for the field tagged `tag`.
///
/// `Err` carries the reason the value cannot be written, and the load report
/// prints it next to the field. The reason is a shape the field cannot read,
/// or a number that does not fit the field's type.
fn decoded_field(
    tag: &'static str,
    document_value: &serde_json::Value,
) -> Result<DecodedField, String> {
    if document_value.is_null() {
        return if is_handle_tag(tag) || tag == "entity" {
            Ok(DecodedField::Cleared)
        } else {
            Err("does not read".to_owned())
        };
    }
    if is_handle_tag(tag) {
        let saved_asset_reference: SavedAssetReference =
            serde_json::from_value(document_value.clone())
                .map_err(|_| "does not read".to_owned())?;
        return Ok(DecodedField::Asset(saved_asset_reference));
    }
    if tag == "entity" {
        return document_value
            .as_str()
            .and_then(EntityId::parse)
            .map(DecodedField::Entity)
            .ok_or_else(|| "does not read".to_owned());
    }
    if let Some(enum_tag) = parse_enum_tag(tag) {
        return decoded_enum_field(tag, &enum_tag, document_value).map(DecodedField::Plain);
    }
    if let Some(element_tag) = tag.strip_prefix("array:") {
        // The element tag is the tail of the field's own `'static` tag, so the
        // same decoder works for every element. This branch builds a plain
        // array, so an element that decodes to an asset reference or an entity
        // id is refused.
        let elements = document_value
            .as_array()
            .ok_or_else(|| "does not read".to_owned())?;
        let mut field_values = Vec::with_capacity(elements.len());
        for element in elements {
            match decoded_field(element_tag, element)? {
                DecodedField::Plain(element_value) => field_values.push(element_value),
                _ => return Err("does not read".to_owned()),
            }
        }
        return Ok(DecodedField::Plain(FieldValue::Array(field_values)));
    }
    let field_value = match tag {
        "f32" => FieldValue::F32(narrow_to_f32(tag, document_value)?),
        "f64" => FieldValue::F64(
            document_value
                .as_f64()
                .ok_or_else(|| "does not read".to_owned())?,
        ),
        "i8" => FieldValue::I8(narrow_to_signed(tag, document_value)?),
        "i16" => FieldValue::I16(narrow_to_signed(tag, document_value)?),
        "i32" => FieldValue::I32(narrow_to_signed(tag, document_value)?),
        "i64" => FieldValue::I64(
            document_value
                .as_i64()
                .ok_or_else(|| "does not read".to_owned())?,
        ),
        "u8" => FieldValue::U8(narrow_to_unsigned(tag, document_value)?),
        "u16" => FieldValue::U16(narrow_to_unsigned(tag, document_value)?),
        "u32" => FieldValue::U32(narrow_to_unsigned(tag, document_value)?),
        "u64" => FieldValue::U64(
            document_value
                .as_u64()
                .ok_or_else(|| "does not read".to_owned())?,
        ),
        "usize" => FieldValue::Usize(narrow_to_unsigned(tag, document_value)?),
        "isize" => FieldValue::Isize(narrow_to_signed(tag, document_value)?),
        "bool" => FieldValue::Bool(
            document_value
                .as_bool()
                .ok_or_else(|| "does not read".to_owned())?,
        ),
        "string" => FieldValue::Text(
            document_value
                .as_str()
                .ok_or_else(|| "does not read".to_owned())?
                .to_owned(),
        ),
        _ => {
            return Ok(DecodedField::Plain(
                decode_opaque_field(tag, document_value)
                    .ok_or_else(|| "does not read".to_owned())?,
            ));
        }
    };
    Ok(DecodedField::Plain(field_value))
}

/// A whole number narrowed to a signed field's own type.
///
/// A value that does not fit is refused, not cast. A cast would wrap 300 into
/// 44 with `as i8`, and leave the world holding a number the document never
/// said. The report names the refused value.
fn narrow_to_signed<T: TryFrom<i64>>(
    tag: &'static str,
    document_value: &serde_json::Value,
) -> Result<T, String> {
    let number = document_value
        .as_i64()
        .ok_or_else(|| "does not read".to_owned())?;
    T::try_from(number).map_err(|_| format!("{document_value} does not fit a `{tag}`"))
}

/// A whole number narrowed to an unsigned field's own type, refusing a
/// negative one and one that does not fit.
fn narrow_to_unsigned<T: TryFrom<u64>>(
    tag: &'static str,
    document_value: &serde_json::Value,
) -> Result<T, String> {
    let number = document_value
        .as_u64()
        .ok_or_else(|| format!("{document_value} does not fit a `{tag}`"))?;
    T::try_from(number).map_err(|_| format!("{document_value} does not fit a `{tag}`"))
}

/// A number narrowed to an `f32`, refusing the one cast that changes its kind:
/// a finite value whose magnitude exceeds `f32::MAX` becomes `inf`. Rounding
/// within range is what the type is for, so it is not a refusal.
fn narrow_to_f32(tag: &'static str, document_value: &serde_json::Value) -> Result<f32, String> {
    let number = document_value
        .as_f64()
        .ok_or_else(|| "does not read".to_owned())?;
    let narrowed = number as f32;
    if number.is_finite() && !narrowed.is_finite() {
        return Err(format!("{document_value} does not fit a `{tag}`"));
    }
    Ok(narrowed)
}

/// Returns the raw bytes an opaque field was written from, tagged with the
/// layout's own `'static` tag. A document can therefore never introduce a tag
/// the layout does not have.
fn decode_opaque_field(
    tag: &'static str,
    document_value: &serde_json::Value,
) -> Option<FieldValue> {
    let encoded_object = document_value.as_object()?;
    if encoded_object.get("tag")?.as_str()? != tag {
        return None;
    }
    Some(FieldValue::Opaque {
        type_tag: tag,
        bytes: bytes_of(encoded_object.get("opaque")?.as_str()?)?,
    })
}

/// Decodes an enum field's document value: a variant name, a discriminant, or
/// the opaque payload a document saved before the enum was declared, whose
/// bytes are the representation's integer.
///
/// # Errors
///
/// Returns the reason when the value has none of those shapes, or names a
/// variant or a discriminant the enum does not declare.
fn decoded_enum_field(
    tag: &'static str,
    enum_tag: &EnumFieldTag,
    document_value: &serde_json::Value,
) -> Result<FieldValue, String> {
    let discriminant = if let Some(variant_name) = document_value.as_str() {
        enum_tag.discriminant_of(variant_name).ok_or_else(|| {
            format!(
                "`{variant_name}` is not a variant of `{}`",
                enum_tag.type_name
            )
        })?
    } else if let Some(number) = document_value.as_i64() {
        number
    } else {
        legacy_opaque_discriminant(enum_tag, document_value)
            .ok_or_else(|| "does not read".to_owned())?
    };
    if enum_tag.variant_name(discriminant).is_none() {
        return Err(format!(
            "{discriminant} is not a variant of `{}`",
            enum_tag.type_name
        ));
    }
    Ok(FieldValue::Enum {
        type_tag: tag,
        discriminant,
    })
}

/// The discriminant an opaque `{ "opaque": <hex>, "tag": ... }` payload holds
/// when its length is the enum representation's size. Bytes are native-endian,
/// as every row is.
fn legacy_opaque_discriminant(
    enum_tag: &EnumFieldTag,
    document_value: &serde_json::Value,
) -> Option<i64> {
    let bytes = bytes_of(document_value.as_object()?.get("opaque")?.as_str()?)?;
    let signed = enum_tag.representation.starts_with('i');
    Some(match (bytes.len(), signed) {
        (1, true) => i64::from(i8::from_ne_bytes(bytes.try_into().ok()?)),
        (1, false) => i64::from(u8::from_ne_bytes(bytes.try_into().ok()?)),
        (2, true) => i64::from(i16::from_ne_bytes(bytes.try_into().ok()?)),
        (2, false) => i64::from(u16::from_ne_bytes(bytes.try_into().ok()?)),
        (4, true) => i64::from(i32::from_ne_bytes(bytes.try_into().ok()?)),
        (4, false) => i64::from(u32::from_ne_bytes(bytes.try_into().ok()?)),
        (8, true) => i64::from_ne_bytes(bytes.try_into().ok()?),
        (8, false) => i64::try_from(u64::from_ne_bytes(bytes.try_into().ok()?)).ok()?,
        _ => return None,
    })
}

/// The value that restores a reference field the document wrote as `null`.
fn cleared_reference_value(tag: &str) -> FieldValue {
    if tag == "entity" {
        FieldValue::Entity {
            id: Entity::NULL.id(),
            generation: Entity::NULL.generation(),
        }
    } else {
        FieldValue::unset_asset()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{AssetGuid, AssetLoadResult, Handle};
    use crate::asset_metadata::{ImportedAsset, MetadataPolicy};
    use crate::asset_store::{mount_directory, mounted_directory_test_lock};
    use crate::component::Component;
    use crate::component::ComponentRegistry;
    use crate::component_registry::ComponentFieldDescriptor;
    use std::borrow::Cow;
    use std::path::{Path, PathBuf};

    /// The test asset: a note's text.
    #[derive(Debug)]
    struct Note(String);
    impl Asset for Note {}
    impl_trait_accessible!(dyn Asset; Note);

    impl ImportedAsset for Note {
        type ImportSettings = ();
        const SOURCE_EXTENSIONS: &'static [&'static str] = &["note"];

        fn metadata_type_name() -> &'static str {
            "pill_engine::scene::tests::Note"
        }

        fn import(_name: &str, bytes: &[u8], _settings: &()) -> AssetLoadResult<Self> {
            Ok(Self(String::from_utf8_lossy(bytes).into_owned()))
        }
    }

    /// A component with a scalar, an asset reference and an entity reference,
    /// which is the shape a scene has to carry.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct Actor {
        speed: f32,
        note: Handle<Note>,
        target: Entity,
    }
    impl Component for Actor {}

    /// Its layout, tagged the way the derive tags the three fields.
    static ACTOR_FIELDS: &[ComponentFieldDescriptor] = &[
        ComponentFieldDescriptor {
            name: "speed",
            type_tag: "f32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "note",
            type_tag: "struct:Handle<Note>",
            offset: 4,
            size: std::mem::size_of::<Handle<Note>>(),
            align: 4,
            element_count: 0,
            decorators: Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "target",
            type_tag: "entity",
            offset: 16,
            size: std::mem::size_of::<Entity>(),
            align: 8,
            element_count: 0,
            decorators: Cow::Borrowed(&[]),
        },
    ];

    const NOTE_GUID: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f0";

    /// A scratch `res` directory, mounted while the test holds the lock, and an
    /// engine with the note type and the actor component registered.
    struct Scratch {
        root_directory: PathBuf,
        engine: Engine,
        _mount_guard: std::sync::MutexGuard<'static, ()>,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            let mount_guard = mounted_directory_test_lock();
            let root_directory =
                std::env::temp_dir().join(format!("pill-scene-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root_directory);
            std::fs::create_dir_all(&root_directory).unwrap();
            mount_directory(&root_directory);
            let mut engine = Engine::new();
            let world = engine.world_mut();
            world.register_imported_asset::<Note>();
            world.register_standalone_asset::<Scene>();
            world.register_component_with_layout::<Actor>(ACTOR_FIELDS);
            world.record_asset_field_type(
                crate::component::ComponentId::of::<Actor>(),
                "note",
                Note::metadata_type_name(),
            );
            Self {
                root_directory,
                engine,
                _mount_guard: mount_guard,
            }
        }

        fn write(&self, relative_path: &str, text: &str) {
            let path = self.root_directory.join(relative_path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        /// A note at `relative_path` whose sidecar carries `guid`.
        fn note(&self, relative_path: &str, guid: &str) {
            self.write(relative_path, "a note");
            self.write(
                &format!("{relative_path}.meta"),
                &format!(
                    r#"{{"format_version": 1, "asset_type": "pill_engine::scene::tests::Note", "guid": "{guid}", "settings": null}}"#
                ),
            );
        }

        fn registry(&self) -> ImportRegistry {
            self.engine
                .world()
                .get_resource::<ImportRegistry>()
                .unwrap()
                .clone()
        }

        fn database(&self) -> AssetDatabase {
            AssetDatabase::from_directory(&self.root_directory, &self.registry())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root_directory);
        }
    }

    fn actor_component_name() -> String {
        ComponentRegistry::registered_name::<Actor>()
    }

    /// A world saves to a `.scene` file with a fresh guid, and loading that file
    /// into an empty engine brings back the same components and references. The
    /// asset resolves by guid and the entity by stable id.
    #[test]
    fn a_scene_round_trips_through_a_file() {
        // Step 1: the world that saves.
        let (reference_guid, scene_file_bytes, target_id) = {
            let mut scratch = Scratch::new("round-trip");
            scratch.note("notes/a.note", NOTE_GUID);
            let registry = scratch.registry();
            let database = scratch.database();
            let world = scratch.engine.world_mut();
            let note_import = {
                let assets = world.get_resource_mut::<AssetManager>().unwrap();
                registry
                    .load_by_guid(
                        assets,
                        &database,
                        AssetGuid::parse(NOTE_GUID).unwrap(),
                        MetadataPolicy::ReadIfPresent,
                    )
                    .unwrap()
            };
            let target_id = EntityId::from_name("hero-target");
            let target = world.create_entity().with_id(target_id).build().unwrap();
            world
                .create_entity()
                .with_id(EntityId::from_name("hero"))
                .with(Actor {
                    speed: 2.5,
                    note: Handle::from_raw(note_import.index, note_import.generation),
                    target,
                })
                .build()
                .unwrap();

            let assets = world.get_resource::<AssetManager>().unwrap();
            let (document, report) = save_scene(world, assets, &registry, None);
            assert!(
                report.skipped_fields.is_empty(),
                "{:?}",
                report.skipped_fields
            );
            assert_eq!(report.components, 1, "only the actor's component");
            assert_eq!(document.entities.len(), 2);

            // The document itself: a scalar, an asset reference by guid and an
            // entity reference by stable id.
            let actor = document
                .entities
                .iter()
                .find(|entity| entity.components.contains_key(&actor_component_name()))
                .expect("the actor is in the document");
            let fields = &actor.components[&actor_component_name()];
            assert_eq!(fields["speed"], serde_json::json!(2.5));
            assert_eq!(
                fields["note"],
                serde_json::json!({ "guid": NOTE_GUID, "path": "notes/a.note" })
            );
            assert_eq!(fields["target"], serde_json::json!(target_id.to_string()));

            // Step 2: the file, written where a project keeps its scenes.
            let guid = AssetGuid::random().expect("a fresh guid");
            let scene_file_bytes =
                crate::asset_standalone::render_standalone::<Scene>(guid, &document)
                    .expect("the scene renders");
            assert!(!document.entities.is_empty());
            (guid, scene_file_bytes, target_id)
        };

        // Step 3: an empty engine with the same project reads the file back as
        // the scene type the project registered.
        let mut scratch = Scratch::new("load");
        scratch.note("notes/a.note", NOTE_GUID);
        scratch.write(
            "scenes/a.scene",
            &String::from_utf8(scene_file_bytes).unwrap(),
        );
        let database = scratch.database();
        // Relative, because an asset's name is its path below the project's
        // asset directory.
        let path = Path::new("scenes/a.scene");
        let scene = {
            let assets = scratch
                .engine
                .world_mut()
                .get_resource_mut::<AssetManager>()
                .unwrap();
            let scene_import = assets
                .import_standalone::<Scene>(path)
                .expect("the scene file loads");
            assert_eq!(
                scene_import.guid,
                Some(reference_guid),
                "the file keeps the guid it was saved with"
            );
            assets
                .get(scene_import.handle)
                .expect("the scene asset is live")
                .clone()
        };
        let report = load_scene(&mut scratch.engine, &database, scene.document());
        assert_eq!(report.entities, 2);
        assert_eq!(report.components, 1);
        assert!(
            report.unknown_components.is_empty(),
            "{:?}",
            report.unknown_components
        );
        assert!(
            report.skipped_fields.is_empty(),
            "{:?}",
            report.skipped_fields
        );
        assert_eq!(report.unresolved_references, 0);

        let world = scratch.engine.world_mut();
        let hero = world
            .entity_by_id(EntityId::from_name("hero"))
            .expect("the hero exists");
        let actor = *world
            .get_component::<Actor>(hero)
            .expect("the actor component");
        assert_eq!(actor.speed, 2.5);
        let assets = world.get_resource::<AssetManager>().unwrap();
        assert_eq!(
            assets.guid_of(actor.note),
            Some(AssetGuid::parse(NOTE_GUID).unwrap()),
            "the asset reference resolves to the same file"
        );
        assert_eq!(
            assets.get(actor.note).map(|note| note.0.as_str()),
            Some("a note"),
            "and the asset it names is the one that was loaded"
        );
        assert_eq!(world.entity_id(actor.target), Some(target_id));
    }

    /// Whatever `save_scene` writes, `load_scene` restores.
    ///
    /// A heap-backed field is read-only through the field layer: its buffer is
    /// reached through the type's own accessors, not through the raw row bytes.
    /// So the saver leaves one out and reports it, because writing a `string`
    /// would put a value in the file that no load could restore.
    #[test]
    fn a_saved_field_is_one_the_loader_accepts() {
        static LABELED_FIELDS: &[ComponentFieldDescriptor] = &[
            ComponentFieldDescriptor {
                name: "label",
                type_tag: "string",
                offset: 0,
                size: std::mem::size_of::<String>(),
                align: std::mem::align_of::<String>(),
                element_count: 0,
                decorators: Cow::Borrowed(&[]),
            },
            ComponentFieldDescriptor {
                name: "points",
                type_tag: "vec:u32",
                offset: std::mem::size_of::<String>(),
                size: std::mem::size_of::<Vec<u32>>(),
                align: std::mem::align_of::<Vec<u32>>(),
                element_count: 0,
                decorators: Cow::Borrowed(&[]),
            },
        ];

        #[repr(C)]
        #[derive(Debug)]
        struct Labeled {
            label: String,
            points: Vec<u32>,
        }
        impl Component for Labeled {}

        // Step 1: a world holding both heap kinds saves.
        let (document, save_report) = {
            let mut scratch = Scratch::new("heap-save");
            scratch
                .engine
                .world_mut()
                .register_component_with_layout::<Labeled>(LABELED_FIELDS);
            let registry = scratch.registry();
            let world = scratch.engine.world_mut();
            world
                .create_entity()
                .with(Labeled {
                    label: "a note's title".to_string(),
                    points: vec![1, 2, 3],
                })
                .build()
                .expect("entity builds");
            let assets = world.get_resource::<AssetManager>().unwrap();
            save_scene(world, assets, &registry, None)
        };

        // Step 2: the document carries no field a load would have to refuse,
        // and both omissions are reported rather than silent.
        let component_fields =
            &document.entities[0].components[&ComponentRegistry::registered_name::<Labeled>()];
        assert!(
            component_fields.is_empty(),
            "a heap-backed field is never written: {component_fields:?}"
        );
        for field in ["label", "points"] {
            assert!(
                save_report
                    .skipped_fields
                    .iter()
                    .any(|skipped| skipped.ends_with(&format!(".{field}"))),
                "{field} is reported as skipped: {:?}",
                save_report.skipped_fields
            );
        }
    }

    /// A component the world knows but has no field layout for is not written,
    /// and the report says so instead of dropping it.
    ///
    /// The field layer writes the scene, so a component without a layout has no
    /// fields to write. Keeping quiet about it would silently leave gameplay
    /// state out of the file.
    #[test]
    fn a_component_without_a_layout_is_reported_as_not_written() {
        #[repr(C)]
        #[derive(Debug, Clone, Copy)]
        struct Plain {
            number: u32,
        }
        impl Component for Plain {}

        let mut scratch = Scratch::new("no-layout");
        let world = scratch.engine.world_mut();
        world.register_component::<Plain>();
        let entity = world
            .create_entity()
            .with(Actor {
                speed: 1.0,
                note: Handle::default(),
                target: Entity::NULL,
            })
            .with(Plain { number: 7 })
            .build()
            .expect("entity builds");
        assert_eq!(
            world.entity_component_names(entity).unwrap().len(),
            2,
            "the entity carries both components"
        );
        let id = world.entity_id(entity).expect("the entity has a stable id");
        let registry = scratch.registry();
        let (document, report) = {
            let world = scratch.engine.world();
            let assets = world.get_resource::<AssetManager>().unwrap();
            save_scene(world, assets, &registry, None)
        };

        // The document holds the component the field layer can write, and only
        // that one.
        let written_components = &document.entities[0].components;
        assert_eq!(report.components, 1, "one of the two is written");
        assert!(
            written_components.contains_key(&actor_component_name()),
            "{written_components:?}"
        );

        // The other one is reported, by entity and name, with the reason.
        let plain_component_name = ComponentRegistry::registered_name::<Plain>();
        assert_eq!(
            report.skipped_components,
            vec![format!("{id}: {plain_component_name} has no field layout")],
            "the component the save left out is named"
        );
    }

    /// A component the world does not know is reported, and the rest of the
    /// entity still loads.
    #[test]
    fn an_unknown_component_is_reported_and_the_rest_loads() {
        let mut scratch = Scratch::new("unknown");
        let id = EntityId::from_name("hero");
        let mut fields = BTreeMap::new();
        fields.insert("speed".to_string(), serde_json::json!(1.25));
        let mut components = BTreeMap::new();
        components.insert(actor_component_name(), fields);
        components.insert(
            "demo::Missing".to_string(),
            BTreeMap::from([("x".to_string(), serde_json::json!(1.0))]),
        );
        let document = SceneDocument {
            entities: vec![SceneEntity { id, components }],
        };

        let report = load_scene(&mut scratch.engine, &AssetDatabase::new(), &document);
        assert_eq!(report.entities, 1);
        assert_eq!(report.components, 1, "the known component attaches");
        assert_eq!(report.unknown_components, vec!["demo::Missing".to_string()]);

        let world = scratch.engine.world_mut();
        let hero = world.entity_by_id(id).expect("the entity exists");
        let actor = world
            .get_component::<Actor>(hero)
            .expect("the actor exists");
        assert_eq!(actor.speed, 1.25);
        assert_eq!(actor.target, Entity::NULL, "an absent reference is unset");
        assert_eq!(actor.note, Handle::default(), "and so is an absent handle");
    }

    /// A scene written while component names kept their module path still
    /// finds its components: the key is read in its current form.
    #[test]
    fn an_old_module_path_component_key_still_attaches() {
        let mut scratch = Scratch::new("old-key");
        let id = EntityId::from_name("hero");
        let current_name = actor_component_name();
        let type_name = current_name.rsplit("::").next().unwrap_or(&current_name);
        let old_name = format!("pill_engine::old_module::{type_name}");
        let mut components = BTreeMap::new();
        components.insert(
            old_name,
            BTreeMap::from([("speed".to_string(), serde_json::json!(1.5))]),
        );
        let document = SceneDocument {
            entities: vec![SceneEntity { id, components }],
        };

        // Through JSON, so the document field's own reader is what normalizes
        // the key - the path a file takes.
        let document: SceneDocument =
            serde_json::from_str(&serde_json::to_string(&document).unwrap()).unwrap();
        let stored_name = document.entities[0]
            .components
            .keys()
            .next()
            .expect("the entity carries one component");
        assert_eq!(
            stored_name, &current_name,
            "the key reads in its current form"
        );

        let report = load_scene(&mut scratch.engine, &AssetDatabase::new(), &document);
        assert_eq!(report.components, 1, "the old key still attaches");
        assert!(
            report.unknown_components.is_empty(),
            "{:?}",
            report.unknown_components
        );
        let world = scratch.engine.world_mut();
        let hero = world.entity_by_id(id).expect("the entity exists");
        let actor = world
            .get_component::<Actor>(hero)
            .expect("the actor exists");
        assert_eq!(actor.speed, 1.5);
    }

    /// A scene written before a component was renamed names it by the old
    /// name. The load attaches it through the former name the type declares,
    /// and the next save writes the current one.
    #[test]
    fn a_component_key_written_under_a_former_name_attaches_and_saves_current() {
        static RENAMED_FIELDS: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
            name: "speed",
            type_tag: "f32",
            offset: 0,
            size: std::mem::size_of::<f32>(),
            align: std::mem::align_of::<f32>(),
            element_count: 0,
            decorators: Cow::Borrowed(&[]),
        }];

        #[repr(C)]
        #[derive(Debug, Clone, Copy)]
        struct RenamedActor {
            speed: f32,
        }
        impl Component for RenamedActor {
            fn shared_name() -> Option<&'static str> {
                Some("demo::RenamedActor")
            }

            fn former_shared_names() -> &'static [&'static str] {
                &["demo_legacy::RenamedActor"]
            }
        }

        let mut scratch = Scratch::new("former-key");
        scratch
            .engine
            .world_mut()
            .register_component_with_layout::<RenamedActor>(RENAMED_FIELDS);
        let id = EntityId::from_name("hero");
        let document = SceneDocument {
            entities: vec![SceneEntity {
                id,
                components: BTreeMap::from([(
                    "demo_legacy::RenamedActor".to_string(),
                    BTreeMap::from([("speed".to_string(), serde_json::json!(2.5))]),
                )]),
            }],
        };

        let report = load_scene(&mut scratch.engine, &AssetDatabase::new(), &document);
        assert_eq!(report.components, 1, "the old name still attaches");
        assert!(
            report.unknown_components.is_empty(),
            "{:?}",
            report.unknown_components
        );
        let hero = scratch
            .engine
            .world_mut()
            .entity_by_id(id)
            .expect("the entity exists");
        assert_eq!(
            scratch
                .engine
                .world()
                .get_component::<RenamedActor>(hero)
                .map(|component| component.speed),
            Some(2.5)
        );

        let registry = scratch.registry();
        let (saved, _) = {
            let world = scratch.engine.world();
            let assets = world.get_resource::<AssetManager>().unwrap();
            save_scene(world, assets, &registry, None)
        };
        assert!(
            saved.entities[0]
                .components
                .contains_key(&ComponentRegistry::registered_name::<RenamedActor>()),
            "the save writes the name the type registers under now: {:?}",
            saved.entities[0].components
        );
    }

    /// A component the world knows but cannot build an image for is reported
    /// in its own list with the reason, not as an unknown type.
    ///
    /// The two failures mean different things to whoever reads the report. An
    /// unknown name is a missing registration. An image that cannot be built is
    /// a registered type the field layer cannot build, which in practice means
    /// a component with no field layout.
    #[test]
    fn a_registered_component_that_cannot_be_attached_is_reported_with_its_reason() {
        #[repr(C)]
        #[derive(Debug, Clone, Copy)]
        struct Layoutless {
            value: u32,
        }
        impl Component for Layoutless {}

        let mut scratch = Scratch::new("unbuildable");
        scratch
            .engine
            .world_mut()
            .register_component::<Layoutless>();
        let id = EntityId::from_name("hero");
        let document = SceneDocument {
            entities: vec![SceneEntity {
                id,
                components: BTreeMap::from([(
                    ComponentRegistry::registered_name::<Layoutless>(),
                    BTreeMap::from([("value".to_string(), serde_json::json!(3))]),
                )]),
            }],
        };

        let report = load_scene(&mut scratch.engine, &AssetDatabase::new(), &document);

        assert!(
            report.unknown_components.is_empty(),
            "the type is registered, so it is not unknown: {:?}",
            report.unknown_components
        );
        assert_eq!(
            report.failed_components.len(),
            1,
            "one component failed to attach: {:?}",
            report.failed_components
        );
        let failure_message = &report.failed_components[0];
        assert!(
            failure_message.contains("could not attach")
                && failure_message.contains("no registered field layout"),
            "the entry carries the field layer's reason: {failure_message}"
        );
        assert_eq!(report.components, 0);
    }

    /// A document that repeats an entity id reports the dropped entry as an
    /// entity, not as a field.
    #[test]
    fn a_repeated_entity_id_is_reported_as_a_skipped_entity() {
        let mut scratch = Scratch::new("repeated-id");
        let id = EntityId::from_name("hero");
        let components = BTreeMap::from([(
            actor_component_name(),
            BTreeMap::from([("speed".to_string(), serde_json::json!(1.0))]),
        )]);
        let document = SceneDocument {
            entities: vec![
                SceneEntity {
                    id,
                    components: components.clone(),
                },
                SceneEntity { id, components },
            ],
        };

        let report = load_scene(&mut scratch.engine, &AssetDatabase::new(), &document);

        assert_eq!(report.entities, 1, "only the first entry creates an entity");
        assert_eq!(
            report.skipped_entities,
            vec![format!("{id}: the id repeats in this scene")]
        );
        assert!(
            report.skipped_fields.is_empty(),
            "nothing was lost at field level: {:?}",
            report.skipped_fields
        );
    }

    /// A document value that does not fit its field's type is refused and
    /// reported, instead of being narrowed into a different number.
    ///
    /// Casting would wrap `"amount": 300` to 44 and turn `1e40` into `inf` for
    /// an `f32`, leaving the world holding numbers the document never said.
    #[test]
    fn a_value_that_does_not_fit_its_field_is_reported() {
        #[repr(C)]
        #[derive(Debug, Clone, Copy, Default)]
        struct Damage {
            amount: u8,
            scale: f32,
        }
        impl Component for Damage {}

        static DAMAGE_FIELDS: &[ComponentFieldDescriptor] = &[
            ComponentFieldDescriptor {
                name: "amount",
                type_tag: "u8",
                offset: 0,
                size: 1,
                align: 1,
                element_count: 0,
                decorators: Cow::Borrowed(&[]),
            },
            ComponentFieldDescriptor {
                name: "scale",
                type_tag: "f32",
                offset: 4,
                size: 4,
                align: 4,
                element_count: 0,
                decorators: Cow::Borrowed(&[]),
            },
        ];

        let mut scratch = Scratch::new("narrow");
        scratch
            .engine
            .world_mut()
            .register_component_with_layout::<Damage>(DAMAGE_FIELDS);
        let id = EntityId::from_name("hero");
        let name = ComponentRegistry::registered_name::<Damage>();
        let mut fields = BTreeMap::new();
        fields.insert("amount".to_string(), serde_json::json!(300));
        fields.insert("scale".to_string(), serde_json::json!(1.0e40));
        let document = SceneDocument {
            entities: vec![SceneEntity {
                id,
                components: BTreeMap::from([(name.clone(), fields)]),
            }],
        };

        let report = load_scene(&mut scratch.engine, &AssetDatabase::new(), &document);
        assert_eq!(report.components, 1, "the component still attaches");
        assert_eq!(
            report.skipped_fields.len(),
            2,
            "both out-of-range values are reported: {:?}",
            report.skipped_fields
        );
        for (field, said) in [("amount", "300"), ("scale", "1e+40")] {
            let entry = report
                .skipped_fields
                .iter()
                .find(|skipped| skipped.contains(&format!("{name}.{field} ")))
                .unwrap_or_else(|| panic!("{field} is reported: {:?}", report.skipped_fields));
            assert!(
                entry.contains(said) && entry.contains("does not fit"),
                "{field} is reported with the value and the reason: {entry}"
            );
        }

        // Refused, not narrowed: the component keeps the zero its image was
        // built with, so nothing invents a number the document never said.
        let world = scratch.engine.world_mut();
        let hero = world.entity_by_id(id).expect("the entity exists");
        let damage = world.get_component::<Damage>(hero).expect("it attached");
        assert_eq!(damage.amount, 0, "300 did not wrap to 44");
        assert_eq!(damage.scale, 0.0, "1e40 did not become inf");
    }

    /// An enum field reads a variant name, a declared discriminant, and the
    /// opaque payload a document saved before the enum was declared; a name or
    /// number the enum does not declare is refused with a reason.
    #[test]
    fn an_enum_field_decodes_names_numbers_and_old_opaque_payloads() {
        const TAG: &str = "enum:u8:Shape{Ball=0,Cuboid=1}";
        let decoded = |document_value: serde_json::Value| match decoded_field(TAG, &document_value)
        {
            Ok(DecodedField::Plain(FieldValue::Enum { discriminant, .. })) => Ok(discriminant),
            Ok(_) => Err("decoded to another kind".to_owned()),
            Err(reason) => Err(reason),
        };
        assert_eq!(decoded(serde_json::json!("Cuboid")), Ok(1));
        assert_eq!(decoded(serde_json::json!(0)), Ok(0));
        assert_eq!(
            decoded(serde_json::json!({ "opaque": "01", "tag": "struct:Shape" })),
            Ok(1)
        );
        assert!(decoded(serde_json::json!("Pyramid")).is_err());
        assert!(decoded(serde_json::json!(7)).is_err());
    }
}
