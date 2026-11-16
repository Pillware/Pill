//! The saved form of references: the shape a document written for a later run
//! stores instead of a [`crate::asset::Handle`] or an [`crate::entity::Entity`].
//!
//! # Responsibilities
//!
//! - [`SavedAssetReference`]: the guid that authorizes a reference, a
//!   non-authoritative path hint beside it, and the sub-asset key it may
//!   carry.
//! - [`saved_reference`]: one field value as the form a saved document holds -
//!   an asset's guid or an entity's stable id.
//! - [`restore_asset_reference`]: the guid back to a field value, loading the
//!   asset on demand and falling back to the path hint when the guid is
//!   unknown.
//! - [`SavedEntityLinks`]: entity references collected while a document is
//!   read, resolved once every entity of it exists.
//!
//! # Design
//!
//! A [`crate::asset::Handle`] is a slot index plus a generation. That pair is
//! exact while one run lasts, and means nothing in the next run, where the
//! same asset may sit in a different slot. Hot-reload persistence depends on
//! that in-run form, so it saves a handle with the handle's own serde and
//! never passes through this module. Data meant for a later run - a scene, a
//! material, a render pass - stores the saved form instead, and turns it back
//! into a handle when the data is read.
//!
//! Resolving a saved reference is a pair of functions over [`FieldValue`], not
//! a serde context. A serialization context cannot do the job: it has no
//! [`AssetManager`] to look a guid up in, and a `Handle` cannot hold a guid
//! waiting to be resolved, because it is `#[repr(C)]` and `Copy`. So an asset
//! reference is resolved on first use, and an entity reference once the whole
//! document has been read.
//!
//! An unresolvable reference means a missing asset, not a broken program. It
//! is reported once per guid and resolves to
//! [`crate::asset::Handle::INVALID`], which the renderer's caches answer with
//! their defaults.

// Standard library
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

// External crates
use pill_core::telemetry::telemetry_target;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// Current crate
use crate::asset::{AssetGuid, AssetManager};
use crate::asset_database::AssetDatabase;
use crate::asset_import_registry::ImportRegistry;
use crate::asset_metadata::MetadataPolicy;
use crate::component_field::FieldValue;
use crate::entity::{Entity, EntityId};
use crate::world::World;

// =============================================================================
// The saved asset reference
// =============================================================================

/// A saved reference to one asset.
///
/// The guid identifies the asset. `path` is a hint stored beside it for a
/// reader whose asset database no longer knows the guid, so a file copied into
/// another project keeps working. `sub_asset` names one part of a file rather
/// than the whole of it, and is written only when it is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedAssetReference {
    /// The guid of the referenced asset.
    pub guid: AssetGuid,
    /// The asset's path when the writer knew it, as a non-authoritative hint.
    pub path: Option<String>,
    /// The sub-asset key inside the file. No loader reads it.
    pub sub_asset: Option<String>,
}

impl SavedAssetReference {
    /// Creates a reference to the asset with `guid`, with no hint and no
    /// sub-asset key.
    pub const fn new(guid: AssetGuid) -> Self {
        Self {
            guid,
            path: None,
            sub_asset: None,
        }
    }

    /// Returns the same reference carrying the path hint `path`.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Returns the same reference naming the sub-asset `key` inside the file.
    #[must_use]
    pub fn with_sub_asset(mut self, key: impl Into<String>) -> Self {
        self.sub_asset = Some(key.into());
        self
    }
}

/// The fields of the object form, as they are written and read.
#[derive(Serialize, Deserialize)]
struct SavedAssetReferenceFields {
    /// The guid, written as 32 hex digits.
    guid: AssetGuid,
    /// The path hint, when the writer knew one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    /// The sub-asset key, when one is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sub_asset: Option<String>,
}

impl Serialize for SavedAssetReference {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        SavedAssetReferenceFields {
            guid: self.guid,
            path: self.path.clone(),
            sub_asset: self.sub_asset.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SavedAssetReference {
    /// Reads the object form, and the bare hex string an older file holds.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// One of the two spellings a document may hold, told apart by the
        /// value's shape.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StoredForm {
            /// The object form this build writes.
            Object(SavedAssetReferenceFields),
            /// The guid alone, as older files write it.
            BareGuid(AssetGuid),
        }
        Ok(match StoredForm::deserialize(deserializer)? {
            StoredForm::Object(fields) => Self {
                guid: fields.guid,
                path: fields.path,
                sub_asset: fields.sub_asset,
            },
            StoredForm::BareGuid(guid) => Self::new(guid),
        })
    }
}

// =============================================================================
// One reference field's saved value
// =============================================================================

/// The saved value of one reference field.
///
/// An asset reference is an object holding its guid, its path hint and its
/// sub-asset key. An entity reference is the entity's stable id, written as 32
/// hex digits. Reading cannot confuse the two, because only the asset form is
/// an object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum SavedReference {
    /// A reference to an asset.
    Asset(SavedAssetReference),
    /// A reference to an entity, by its stable id.
    Entity(EntityId),
}

impl<'de> Deserialize<'de> for SavedReference {
    /// Reads an asset reference from an object and an entity id from a string.
    ///
    /// A bare string is never read as an asset here. This build always writes
    /// an asset reference as an object, and the guid-only string an older
    /// `.material` file holds is read by
    /// [`AssetReference`](crate::AssetReference) itself.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReferenceVisitor;
        impl<'de> serde::de::Visitor<'de> for ReferenceVisitor {
            type Value = SavedReference;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an asset reference object or an entity id")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                let reference = SavedAssetReference::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;
                Ok(SavedReference::Asset(reference))
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Self::Value, E> {
                let id = EntityId::parse(text).ok_or_else(|| {
                    E::custom(format!("`{text}` is not an entity id (32 hex digits)"))
                })?;
                Ok(SavedReference::Entity(id))
            }
        }
        deserializer.deserialize_any(ReferenceVisitor)
    }
}

/// Returns the saved form of one field value, or `None` when the value
/// references nothing: an unset handle, the null entity, or a field that is
/// not a reference at all.
///
/// The world turns a stored entity handle into an [`EntityId`]. The asset
/// manager and the import registry turn an asset handle into the guid and the
/// path that identify the asset. An asset handle whose asset has no guid
/// cannot be saved, so it comes back as `None` here rather than as a reference
/// that would later resolve to the wrong asset.
pub fn saved_reference(
    world: &World,
    assets: &AssetManager,
    registry: &ImportRegistry,
    value: &FieldValue,
) -> Option<SavedReference> {
    match value {
        FieldValue::Asset {
            asset_type,
            index,
            generation,
        } => {
            if value.references_nothing() {
                return None;
            }
            let type_name = asset_type.as_deref()?;
            let identity = registry.asset_identity(assets, type_name, *index, *generation)?;
            let guid = identity.guid?;
            let reference = match identity.name {
                Some(name) => SavedAssetReference::new(guid).with_path(name),
                None => SavedAssetReference::new(guid),
            };
            Some(SavedReference::Asset(reference))
        }
        FieldValue::Entity { id, generation } => {
            let entity = Entity::from_raw(*id, *generation);
            if entity.is_null() {
                return None;
            }
            world.entity_id(entity).map(SavedReference::Entity)
        }
        _ => None,
    }
}

// =============================================================================
// Resolving a saved asset back
// =============================================================================

/// Returns the field value that an asset reference field holds for `saved`,
/// loading the asset on demand.
///
/// The guid is tried first, looked up in the project's asset database. A guid
/// nothing knows falls back to the path hint, which keeps a reference working
/// after its asset was copied into another project that gave it a different
/// guid. That fallback is logged once per guid, and so is a reference that
/// resolves to nothing: it comes back invalid, so one missing asset does not
/// fail the document that names it.
///
/// The caller writes the value into the field. Loading needs a mutable asset
/// manager, and the world that holds the field cannot be borrowed mutably at
/// the same time.
pub fn restore_asset_reference(
    assets: &mut AssetManager,
    database: &AssetDatabase,
    registry: &ImportRegistry,
    saved: &SavedAssetReference,
) -> FieldValue {
    // Try the guid first, then the path hint. The steps stay flat rather than
    // nested, so each failure is handled once: every miss is logged once per
    // guid and leaves the field invalid instead of failing the document.
    let guid_error =
        match registry.load_by_guid(assets, database, saved.guid, MetadataPolicy::ReadIfPresent) {
            Ok(outcome) => return asset_field_value_from_outcome(&outcome),
            Err(error) => error,
        };
    let Some(path) = saved.path.as_deref() else {
        if first_report_of(saved.guid) {
            pill_core::warn!(
                target: telemetry_target::RESOURCES,
                guid = %saved.guid,
                error = %guid_error,
                "saved asset reference resolves to no asset and has no path hint; using the invalid handle"
            );
        }
        return FieldValue::unset_asset();
    };
    match registry.load_by_path(assets, Path::new(path), MetadataPolicy::ReadIfPresent) {
        Ok(outcome) => {
            if first_report_of(saved.guid) {
                pill_core::warn!(
                    target: telemetry_target::RESOURCES,
                    guid = %saved.guid,
                    path,
                    "saved asset reference is unknown here; resolved by its path hint instead"
                );
            }
            asset_field_value_from_outcome(&outcome)
        }
        Err(path_error) => {
            if first_report_of(saved.guid) {
                pill_core::warn!(
                    target: telemetry_target::RESOURCES,
                    guid = %saved.guid,
                    path,
                    error = %format!("{guid_error}; the path hint: {path_error}"),
                    "saved asset reference resolves to no asset by guid or path hint; using the invalid handle"
                );
            }
            FieldValue::unset_asset()
        }
    }
}

/// The asset field value that a loaded asset's outcome describes.
fn asset_field_value_from_outcome(
    outcome: &crate::asset_import_registry::ErasedImportOutcome,
) -> FieldValue {
    FieldValue::Asset {
        asset_type: Some(outcome.type_name.clone()),
        index: outcome.index,
        generation: outcome.generation,
    }
}

/// Returns whether this is the first report of `guid` in this process, and
/// records the guid so later reports of it are not.
///
/// One set serves every place that reports an unresolvable reference, so a
/// guid named by both a typed field and a scene entry produces one warning,
/// not one per caller.
pub(crate) fn first_report_of(guid: AssetGuid) -> bool {
    static REPORTED: OnceLock<Mutex<HashSet<AssetGuid>>> = OnceLock::new();
    REPORTED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(guid)
}

// =============================================================================
// Deferred entity references
// =============================================================================

/// One entity reference a document names, waiting for its entity to exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedEntityLink {
    /// The entity holding the field.
    pub entity: Entity,
    /// The component's registered name.
    pub component: String,
    /// The field's name.
    pub field: String,
    /// The id the document named.
    pub id: EntityId,
}

/// The entity references a document holds, resolved in one pass.
///
/// A document is read in two steps, because a reference may name an entity
/// that comes later in the same document. Every entity is created first, and
/// then the ids are resolved against a world that holds them all. A link whose
/// id matches no entity names something that was deleted before the document
/// was written, so it comes back unresolved instead of being silently
/// cleared.
#[derive(Debug, Default, Clone)]
pub struct SavedEntityLinks {
    /// The links still waiting for their entity.
    links: Vec<SavedEntityLink>,
}

impl SavedEntityLinks {
    /// Records that `entity`'s `component.field` names the entity `id`.
    pub fn push(&mut self, entity: Entity, component: &str, field: &str, id: EntityId) {
        self.links.push(SavedEntityLink {
            entity,
            component: component.to_owned(),
            field: field.to_owned(),
            id,
        });
    }

    /// Returns how many links are waiting.
    #[must_use]
    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// Returns whether no link is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// Writes every link into its field.
    ///
    /// Returns how many links were written, and the links that stayed
    /// unresolved. A link stays unresolved when its id names no entity, and
    /// also when the field refuses the write, because the document named a
    /// reference the world could not take.
    pub fn resolve(self, world: &mut World) -> (usize, Vec<SavedEntityLink>) {
        let mut resolved_count = 0;
        let mut unresolved_links = Vec::new();
        for link in self.links {
            let Some(target) = world.entity_by_id(link.id) else {
                unresolved_links.push(link);
                continue;
            };
            let value = FieldValue::Entity {
                id: target.id(),
                generation: target.generation(),
            };
            match world.write_component_field(link.entity, &link.component, &link.field, value) {
                Ok(()) => resolved_count += 1,
                Err(_) => unresolved_links.push(link),
            }
        }
        (resolved_count, unresolved_links)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Asset, AssetLoadResult, Handle};
    use crate::asset_database::AssetDatabase;
    use crate::asset_metadata::ImportedAsset;
    use crate::asset_store::{mount_directory, mounted_directory_test_lock};
    use crate::component_registry::ComponentFieldDescriptor;
    use std::borrow::Cow;
    use std::path::PathBuf;
    use trait_type_map::impl_trait_accessible;

    /// The test asset: a text file's contents.
    #[derive(Debug)]
    struct Text(String);
    impl Asset for Text {}
    impl_trait_accessible!(dyn Asset; Text);

    impl Text {
        /// Returns the text the asset holds.
        fn text(&self) -> &str {
            &self.0
        }
    }

    impl ImportedAsset for Text {
        type ImportSettings = ();
        const SOURCE_EXTENSIONS: &'static [&'static str] = &["txt"];

        fn metadata_type_name() -> &'static str {
            "pill_engine::saved_references::tests::Text"
        }

        fn import(_name: &str, bytes: &[u8], _settings: &()) -> AssetLoadResult<Self> {
            Ok(Self(String::from_utf8_lossy(bytes).into_owned()))
        }
    }

    /// A component holding one asset handle and one entity reference.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct Refs {
        asset: Handle<Text>,
        target: Entity,
    }
    impl crate::component::Component for Refs {}

    /// Its layout, tagged the way the derive tags the two fields.
    static REFS_FIELDS: &[ComponentFieldDescriptor] = &[
        ComponentFieldDescriptor {
            name: "asset",
            type_tag: "struct:Handle<Text>",
            offset: 0,
            size: std::mem::size_of::<Handle<Text>>(),
            align: 4,
            element_count: 0,
            decorators: Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "target",
            type_tag: "entity",
            offset: 8,
            size: std::mem::size_of::<Entity>(),
            align: 8,
            element_count: 0,
            decorators: Cow::Borrowed(&[]),
        },
    ];

    /// The guid the note's sidecar carries.
    const NOTE_GUID: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";

    /// A scratch `res` directory, mounted while the test holds the lock, and a
    /// world with the text type and the reference component registered.
    struct Scratch {
        root_directory: PathBuf,
        world: World,
        _mounted: std::sync::MutexGuard<'static, ()>,
    }

    impl Scratch {
        /// Creates the scratch directory, mounts it, and builds a world that
        /// knows the text type and the reference component.
        fn new(label: &str) -> Self {
            let mounted = mounted_directory_test_lock();
            let root_directory = std::env::temp_dir().join(format!(
                "pill-saved-references-{label}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root_directory);
            std::fs::create_dir_all(&root_directory).unwrap();
            mount_directory(&root_directory);
            let mut world = World::new();
            world.register_imported_asset::<Text>();
            world.register_component_with_layout::<Refs>(REFS_FIELDS);
            // A derive records the asset type of a handle field for a real
            // component. This layout is written by hand, so the asset type is
            // recorded by hand too.
            world.record_asset_field_type(
                crate::component::ComponentId::of::<Refs>(),
                "asset",
                Text::metadata_type_name(),
            );
            Self {
                root_directory,
                world,
                _mounted: mounted,
            }
        }

        /// Writes `text` to `relative_path` under the scratch directory.
        fn write(&self, relative_path: &str, text: &str) {
            let path = self.root_directory.join(relative_path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        /// Writes a text asset at `relative_path` whose sidecar carries `guid`.
        fn write_text_asset(&self, relative_path: &str, guid: &str) {
            self.write(relative_path, "hello");
            self.write(
                &format!("{relative_path}.meta"),
                &format!(
                    r#"{{"format_version": 1, "asset_type": "pill_engine::saved_references::tests::Text", "guid": "{guid}", "settings": null}}"#
                ),
            );
        }

        fn registry(&self) -> ImportRegistry {
            self.world.get_resource::<ImportRegistry>().unwrap().clone()
        }

        /// Builds the asset database for the scratch directory.
        fn database(&self) -> AssetDatabase {
            AssetDatabase::from_directory(&self.root_directory, &self.registry())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root_directory);
        }
    }

    /// Returns the registered name of the test's reference component.
    fn refs_component_name() -> String {
        crate::component::ComponentRegistry::registered_name::<Refs>()
    }

    /// Parses a 32 hex digit guid written in the test source.
    fn parse_guid(text: &str) -> AssetGuid {
        AssetGuid::parse(text).expect("the test's guid is 32 hexadecimal digits")
    }

    /// A component holding a handle and an entity reference round-trips through
    /// the saved form into a fresh world, where it names the same asset and the
    /// same entity.
    #[test]
    fn references_round_trip_into_a_fresh_world() {
        // Step 1: the world that saves, with the asset loaded and the fields
        // set from code. Its scratch directory is released before the second
        // one is mounted, because the mounted directory is state shared by the
        // whole process.
        let (saved_asset, saved_entity) = {
            let mut saving_scratch = Scratch::new("round-trip-first");
            saving_scratch.write_text_asset("notes/a.txt", NOTE_GUID);
            let registry = saving_scratch.registry();
            let database = saving_scratch.database();
            let world = &mut saving_scratch.world;
            let load_outcome = {
                let assets = world.get_resource_mut::<AssetManager>().unwrap();
                registry
                    .load_by_guid(
                        assets,
                        &database,
                        parse_guid(NOTE_GUID),
                        MetadataPolicy::ReadIfPresent,
                    )
                    .unwrap()
            };
            {
                let assets = world.get_resource::<AssetManager>().unwrap();
                let text = assets
                    .get(Handle::<Text>::from_raw(
                        load_outcome.index,
                        load_outcome.generation,
                    ))
                    .expect("the note loaded");
                assert_eq!(text.text(), "hello");
            }
            let target = world.create_entity().build().unwrap();
            let holder = world
                .create_entity()
                .with(Refs {
                    asset: Handle::from_raw(load_outcome.index, load_outcome.generation),
                    target,
                })
                .build()
                .unwrap();

            let assets = world.get_resource::<AssetManager>().unwrap();
            let asset_value = world
                .read_component_field(holder, &refs_component_name(), "asset")
                .unwrap();
            let entity_value = world
                .read_component_field(holder, &refs_component_name(), "target")
                .unwrap();
            let saved_asset = match saved_reference(world, assets, &registry, &asset_value) {
                Some(SavedReference::Asset(reference)) => reference,
                other => panic!("the handle saved as {other:?}"),
            };
            let saved_entity = match saved_reference(world, assets, &registry, &entity_value) {
                Some(SavedReference::Entity(id)) => id,
                other => panic!("the entity saved as {other:?}"),
            };
            assert_eq!(saved_asset.guid, parse_guid(NOTE_GUID));
            assert_eq!(saved_asset.path.as_deref(), Some("notes/a.txt"));
            (saved_asset, saved_entity)
        };

        // Step 2: a fresh world with its own manager. The asset loads on
        // demand and the entity is created before the link is resolved.
        let mut restoring_scratch = Scratch::new("round-trip-second");
        restoring_scratch.write_text_asset("notes/a.txt", NOTE_GUID);
        let registry = restoring_scratch.registry();
        let database = restoring_scratch.database();
        let world = &mut restoring_scratch.world;
        // The document carries the entity's stable id, so the loader builds the
        // entity under that id: that is what makes the link resolvable.
        let new_target = world.create_entity().with_id(saved_entity).build().unwrap();
        let new_holder = world
            .create_entity()
            .with(Refs {
                asset: Handle::INVALID,
                target: Entity::NULL,
            })
            .build()
            .unwrap();
        let restored = {
            let assets = world.get_resource_mut::<AssetManager>().unwrap();
            restore_asset_reference(assets, &database, &registry, &saved_asset)
        };
        world
            .write_component_field(new_holder, &refs_component_name(), "asset", restored)
            .unwrap();
        let mut links = SavedEntityLinks::default();
        links.push(new_holder, &refs_component_name(), "target", saved_entity);
        let (resolved_count, unresolved_links) = links.resolve(world);
        assert_eq!((resolved_count, unresolved_links.len()), (1, 0));

        let refreshed: Refs = *world.get_component::<Refs>(new_holder).unwrap();
        let assets = world.get_resource::<AssetManager>().unwrap();
        assert_eq!(assets.guid_of(refreshed.asset), Some(saved_asset.guid));
        assert_eq!(
            assets.get(refreshed.asset).map(Text::text),
            Some("hello"),
            "the reloaded handle addresses the note"
        );
        assert_eq!(world.entity_id(refreshed.target), Some(saved_entity));
        assert_eq!(refreshed.target, new_target);
    }

    /// A guid nothing knows falls back to the path hint, which loads the file
    /// under the guid its own metadata carries.
    #[test]
    fn an_unknown_guid_resolves_by_its_path_hint() {
        let mut scratch = Scratch::new("path-fallback");
        scratch.write_text_asset("notes/moved.txt", "11111111111111111111111111111111");
        let registry = scratch.registry();
        let database = scratch.database();
        let world = &mut scratch.world;
        let holder = world
            .create_entity()
            .with(Refs {
                asset: Handle::INVALID,
                target: Entity::NULL,
            })
            .build()
            .unwrap();

        let stale = SavedAssetReference::new(parse_guid("99999999999999999999999999999999"))
            .with_path("notes/moved.txt");
        let restored = {
            let assets = world.get_resource_mut::<AssetManager>().unwrap();
            restore_asset_reference(assets, &database, &registry, &stale)
        };
        world
            .write_component_field(holder, &refs_component_name(), "asset", restored)
            .unwrap();

        let refreshed: Refs = *world.get_component::<Refs>(holder).unwrap();
        let assets = world.get_resource::<AssetManager>().unwrap();
        assert_eq!(
            assets.guid_of(refreshed.asset),
            Some(parse_guid("11111111111111111111111111111111")),
            "the file's own guid wins over the document's unknown one"
        );
    }

    /// A reference with neither a known guid nor a resolving path restores as
    /// the invalid handle instead of failing, and is reported once.
    #[test]
    fn a_reference_to_nothing_restores_as_the_invalid_handle() {
        let mut scratch = Scratch::new("missing");
        let registry = scratch.registry();
        let database = scratch.database();
        let world = &mut scratch.world;
        let missing = SavedAssetReference::new(parse_guid("88888888888888888888888888888888"));
        let restored = {
            let assets = world.get_resource_mut::<AssetManager>().unwrap();
            restore_asset_reference(assets, &database, &registry, &missing)
        };
        assert_eq!(restored, FieldValue::unset_asset());
    }

    /// Reads both stored spellings: the object with a path hint, the object
    /// without one, and the bare hex string an older file holds.
    #[test]
    fn the_saved_asset_reference_reads_both_spellings() {
        let with_hint = SavedAssetReference::new(parse_guid(NOTE_GUID)).with_path("notes/a.txt");
        let json = serde_json::to_string(&with_hint).unwrap();
        assert_eq!(
            json,
            format!(r#"{{"guid":"{NOTE_GUID}","path":"notes/a.txt"}}"#)
        );
        assert_eq!(
            serde_json::from_str::<SavedAssetReference>(&json).unwrap(),
            with_hint
        );

        let bare = serde_json::to_string(&SavedAssetReference::new(parse_guid(NOTE_GUID))).unwrap();
        assert_eq!(bare, format!(r#"{{"guid":"{NOTE_GUID}"}}"#));
        assert_eq!(
            serde_json::from_str::<SavedAssetReference>(&format!("\"{NOTE_GUID}\"")).unwrap(),
            SavedAssetReference::new(parse_guid(NOTE_GUID))
        );
    }

    /// A saved entity reference is the stable id, and reads back as one.
    #[test]
    fn a_saved_entity_reference_is_its_stable_id() {
        let id = EntityId::from_name("hero");
        let saved = SavedReference::Entity(id);
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(
            serde_json::from_str::<SavedReference>(&json).unwrap(),
            saved
        );
    }
}
