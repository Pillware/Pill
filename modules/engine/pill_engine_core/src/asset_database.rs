//! An index of every asset a project holds, by guid and by path, whether or
//! not it is loaded: [`AssetDatabase`].
//!
//! # Responsibilities
//!
//! - Index each asset in a project's `res` directory by guid and by path, and
//!   record the type its header names and whether it is standalone
//!   ([`AssetRecord`]). The index is built from a directory
//!   ([`AssetDatabase::from_directory`]) or from a shipping pack
//!   ([`AssetDatabase::from_pack`]).
//! - Follow edits on disk one path at a time
//!   ([`AssetDatabase::refresh`]): a new file, a deleted one, and a file
//!   moved together with its `.meta`.
//! - Report a guid that two files claim, instead of indexing the asset twice
//!   ([`AssetDatabase::conflicts`]).
//!
//! # Design
//!
//! The [`AssetManager`](crate::AssetManager) knows which assets are loaded;
//! this index knows which ones exist. A reference names an asset by guid (see
//! [`AssetReference`](crate::AssetReference)), so resolving an asset that is
//! not loaded yet needs a step from its guid to its path. The import registry
//! takes that step ([`ImportRegistry::load_by_guid`]).
//!
//! The identity rules are the importer's (`asset_metadata.rs`): one path is
//! one asset, and the guid is the one its `.meta` holds (or, for a standalone
//! file, the one in its own header). A guid that a second path claims is
//! refused, not moved. The index takes only the identity out of a header -
//! `format_version`, `asset_type` and `guid` - and never decodes an asset's
//! settings, so indexing a large project costs one small YAML read per asset.
//! A source with no `.meta` has no guid yet and is not indexed; the write that
//! gives it one is a change [`AssetDatabase::refresh`] picks up.
//!
//! [`ImportRegistry::load_by_guid`]: crate::ImportRegistry::load_by_guid

// Standard library
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

// Current crate
use crate::asset::AssetGuid;
use crate::asset_import_registry::ImportRegistry;
use crate::asset_metadata::METADATA_EXTENSION;
use crate::asset_store::AssetPack;
use crate::resource::Resource;

// =============================================================================
// AssetRecord
// =============================================================================

/// One asset the project holds, whether or not it is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRecord {
    /// The asset's stable identity.
    pub guid: AssetGuid,
    /// The path the asset loads under, relative to `res` and separated by `/`.
    pub path: String,
    /// The asset type its header names (`pill_master_renderer::assets::Mesh`).
    pub asset_type: String,
    /// Whether the file is the asset itself and holds its own header, rather
    /// than a source with a `.meta` sidecar.
    pub standalone: bool,
}

/// Two paths that claim one guid. The first path keeps it; the second is not
/// indexed.
///
/// The importer refuses the same thing when a source is copied together with
/// its `.meta`: the copied file needs a guid of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidConflict {
    /// The guid both paths claim.
    pub guid: AssetGuid,
    /// The path that keeps the guid.
    pub kept: String,
    /// The path that also claims it and is not indexed.
    pub refused: String,
}

// =============================================================================
// AssetDatabase
// =============================================================================

/// Every asset of a project, indexed by guid and by path; see the module docs.
///
/// The host inserts it as a world resource when it mounts the project's
/// assets. Whatever resolves a reference or lists the assets of a type reads
/// it from there.
#[derive(Debug, Default, Clone)]
pub struct AssetDatabase {
    /// Every indexed asset, keyed by path and kept sorted so listings are
    /// stable.
    records: BTreeMap<String, AssetRecord>,
    /// The path that holds each indexed guid.
    paths_by_guid: HashMap<AssetGuid, String>,
    /// Guids that a second path claimed, as found by the build and by each
    /// refresh.
    conflicts: Vec<GuidConflict>,
}

impl Resource for AssetDatabase {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Keep
    }
}

impl AssetDatabase {
    /// An empty database.
    pub fn new() -> Self {
        Self::default()
    }

    /// Indexes every asset under `root`, which is a project's `res` directory.
    ///
    /// `registry` says which file extensions belong to standalone types, where
    /// the file itself holds the header. Every other file is indexed through
    /// its `.meta`, when it has one. Files are visited in path order, so when
    /// two paths claim one guid, the first path keeps it on every machine.
    pub fn from_directory(root: &Path, registry: &ImportRegistry) -> Self {
        let mut database = Self::new();
        for relative_path in files_under(root) {
            database.index_from_disk(root, &relative_path, registry);
        }
        database
    }

    /// Indexes every asset that a shipping pack carries, from the `.meta`
    /// entries and the standalone files it holds beside the sources.
    ///
    /// This needs no registry, because a shipping build indexes its pack before
    /// any module has registered a type. An entry with a `.meta` beside it is a
    /// sourced asset. An entry without one is a standalone asset when the entry
    /// itself is an asset header, so a binary source is skipped.
    pub fn from_pack(pack: &AssetPack) -> Self {
        let mut database = Self::new();
        // Step 1: Collect every entry that is not a `.meta` file or a
        // dot-file, in path order.
        let mut paths: Vec<&str> = pack
            .paths()
            .filter(|path| !is_metadata_path(path) && !is_hidden_path(path))
            .collect();
        paths.sort_unstable();
        // Step 2: Read each entry's identity from its `.meta` when it has one,
        // and from the entry itself when it does not.
        for path in paths {
            let sidecar_identity = pack
                .get(&format!("{path}.{METADATA_EXTENSION}"))
                .and_then(read_identity);
            let (identity, standalone) = match sidecar_identity {
                Some(identity) => (Some(identity), false),
                None => (pack.get(path).and_then(read_identity), true),
            };
            if let Some((asset_type, guid)) = identity {
                database.insert(AssetRecord {
                    guid,
                    path: path.to_owned(),
                    asset_type,
                    standalone,
                });
            }
        }
        database
    }

    /// Re-reads one path under `root` after the file changed on disk.
    ///
    /// `relative_path` is the source's path relative to `root`. A change to a
    /// `.meta` file is reported under its source's path. A file that is gone,
    /// or that no longer has a header, leaves the index.
    ///
    /// A file whose guid another path holds takes the guid over when that other
    /// path no longer exists, which is what a file moved with its `.meta`
    /// looks like. While the other path still exists, the file is refused as a
    /// conflict.
    pub fn refresh(&mut self, root: &Path, relative_path: &str, registry: &ImportRegistry) {
        // Step 1: Drop what this path used to contribute.
        let path = to_slash_path(relative_path);
        self.remove_path(&path);
        self.conflicts.retain(|conflict| conflict.refused != path);
        if !root.join(&path).is_file() {
            return;
        }
        // Step 2: Read the path again, and follow the guid to its new path
        // when the old one is gone.
        let Some(record) = read_record(root, &path, registry) else {
            return;
        };
        // The path that held this guid is gone from disk, so this record is
        // the asset at its new path.
        if let Some(owner_path) = self.paths_by_guid.get(&record.guid).cloned() {
            if !root.join(&owner_path).is_file() {
                self.remove_path(&owner_path);
            }
        }
        self.insert(record);
    }

    /// The asset with `guid`, if one is indexed.
    pub fn by_guid(&self, guid: AssetGuid) -> Option<&AssetRecord> {
        self.records.get(self.paths_by_guid.get(&guid)?)
    }

    /// The asset at `path` (relative to `res`, either separator), if indexed.
    pub fn by_path(&self, path: &str) -> Option<&AssetRecord> {
        self.records.get(&to_slash_path(path))
    }

    /// Every indexed asset, in path order.
    pub fn records(&self) -> impl Iterator<Item = &AssetRecord> {
        self.records.values()
    }

    /// Every indexed asset of type `asset_type`, in path order. This is what a
    /// reference picker lists.
    pub fn records_of_type<'a>(
        &'a self,
        asset_type: &'a str,
    ) -> impl Iterator<Item = &'a AssetRecord> + 'a {
        self.records
            .values()
            .filter(move |record| record.asset_type == asset_type)
    }

    /// The guids that a second path claimed, as found by the build and by each
    /// refresh.
    pub fn conflicts(&self) -> &[GuidConflict] {
        &self.conflicts
    }

    /// How many assets are indexed.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Reads `relative_path` from disk under `root` and indexes it, when it
    /// carries a header.
    fn index_from_disk(&mut self, root: &Path, relative_path: &str, registry: &ImportRegistry) {
        if let Some(record) = read_record(root, relative_path, registry) {
            self.insert(record);
        }
    }

    /// Adds `record`. When another path already holds its guid, records a
    /// conflict instead and does not index the asset.
    fn insert(&mut self, record: AssetRecord) {
        if let Some(owner_path) = self.paths_by_guid.get(&record.guid) {
            if owner_path != &record.path {
                self.conflicts.push(GuidConflict {
                    guid: record.guid,
                    kept: owner_path.clone(),
                    refused: record.path,
                });
                return;
            }
        }
        self.paths_by_guid.insert(record.guid, record.path.clone());
        self.records.insert(record.path.clone(), record);
    }

    /// Drops the record at `path`, when there is one, and the guid entry that
    /// points at it.
    fn remove_path(&mut self, path: &str) {
        if let Some(record) = self.records.remove(path) {
            if self
                .paths_by_guid
                .get(&record.guid)
                .is_some_and(|owner_path| owner_path == path)
            {
                self.paths_by_guid.remove(&record.guid);
            }
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Builds the record for `relative_path` under `root`, or returns `None` when
/// it has no readable header.
///
/// The header is read from the file itself when its type is standalone, and
/// from its `.meta` otherwise.
fn read_record(root: &Path, relative_path: &str, registry: &ImportRegistry) -> Option<AssetRecord> {
    let standalone = is_standalone(relative_path, registry);
    let header_path = if standalone {
        root.join(relative_path)
    } else {
        root.join(format!("{relative_path}.{METADATA_EXTENSION}"))
    };
    let bytes = std::fs::read(header_path).ok()?;
    let (asset_type, guid) = read_identity(&bytes)?;
    Some(AssetRecord {
        guid,
        path: to_slash_path(relative_path),
        asset_type,
        standalone,
    })
}

/// Returns the `asset_type` and the `guid` of a header that this engine can
/// read, or `None` when it cannot.
///
/// The header rules live in
/// [`asset_metadata::read_header`](crate::asset_metadata::read_header), so the
/// index and the importer agree on which format versions are readable and what
/// a guid looks like. The index needs only the identity, not the body, and has
/// no one to report a refusal to, so a refused header becomes `None`.
fn read_identity(bytes: &[u8]) -> Option<(String, AssetGuid)> {
    let (header, _) = crate::asset_metadata::read_header(bytes).ok()?;
    Some((header.asset_type, header.guid))
}

/// Whether the extension of `path` belongs to a standalone type.
fn is_standalone(path: &str, registry: &ImportRegistry) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| registry.is_standalone_extension(extension))
}

/// Whether `path` names a `.meta` sidecar.
fn is_metadata_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(METADATA_EXTENSION))
}

/// Whether any part of `path` is a dot-file or a dot-directory.
///
/// `write_beside` writes a metadata update next to its source as
/// `.<file>.meta.<pid>-<seq>.tmp`, and a crash can leave one of those files
/// behind. It holds the asset's own header, so indexing it would take the guid
/// away from the asset it was written for. `files_under` skips dot-files on
/// disk, and a pack has to skip them too.
fn is_hidden_path(path: &str) -> bool {
    path.split('/').any(|component| component.starts_with('.'))
}

/// Returns `path` with `/` separators, the form the index uses as a key.
fn to_slash_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// Every file under `root` as a `/`-separated relative path, sorted, with
/// `.meta` files and dot-files left out. A directory that cannot be read
/// contributes nothing.
fn files_under(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut pending_directories = vec![String::new()];
    while let Some(directory) = pending_directories.pop() {
        let Ok(entries) = std::fs::read_dir(root.join(&directory)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let relative_path = if directory.is_empty() {
                name
            } else {
                format!("{directory}/{name}")
            };
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending_directories.push(relative_path),
                Ok(kind) if kind.is_file() && !is_metadata_path(&relative_path) => {
                    files.push(relative_path)
                }
                _ => {}
            }
        }
    }
    files.sort();
    files
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Asset, AssetLoadResult, AssetManager};
    use crate::asset_metadata::{ImportedAsset, MetadataPolicy};
    use crate::asset_standalone::StandaloneAsset;
    use crate::asset_store::{mount_directory, mounted_directory_test_lock};
    use crate::World;
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;
    use trait_type_map::impl_trait_accessible;

    /// An imported test asset: the text of a `.note` file.
    #[derive(Debug)]
    struct Note(String);
    impl Asset for Note {}
    impl_trait_accessible!(dyn Asset; Note);

    impl ImportedAsset for Note {
        type ImportSettings = NoteSettings;
        const SOURCE_EXTENSIONS: &'static [&'static str] = &["note"];

        fn metadata_type_name() -> &'static str {
            "pill_engine::asset_database::tests::Note"
        }

        fn import(_name: &str, bytes: &[u8], _settings: &NoteSettings) -> AssetLoadResult<Self> {
            Ok(Self(String::from_utf8_lossy(bytes).into_owned()))
        }
    }

    /// The note's (empty) import settings.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    struct NoteSettings {}

    /// A standalone test asset: the `.card` file is the asset itself.
    #[derive(Debug)]
    struct Card;
    impl Asset for Card {}
    impl_trait_accessible!(dyn Asset; Card);

    impl StandaloneAsset for Card {
        type Document = CardDocument;
        const FILE_EXTENSION: &'static str = "card";

        fn metadata_type_name() -> &'static str {
            "pill_engine::asset_database::tests::Card"
        }

        fn from_document(
            _name: &str,
            _document: CardDocument,
            _assets: &AssetManager,
        ) -> AssetLoadResult<Self> {
            Ok(Self)
        }
    }

    /// The card's (empty) document.
    #[derive(Debug, Default, Serialize, Deserialize)]
    struct CardDocument {}

    const NOTE_GUID: &str = "000000000000000000000000000000a1";
    const CARD_GUID: &str = "000000000000000000000000000000c1";

    /// A scratch `res` directory and a world with both test types registered.
    /// The directory stays mounted while the test holds the lock.
    struct Scratch {
        root: PathBuf,
        world: World,
        _mount_guard: std::sync::MutexGuard<'static, ()>,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            let mounted = mounted_directory_test_lock();
            let root = std::env::temp_dir().join(format!(
                "pill-asset-database-{label}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            mount_directory(&root);
            let mut world = World::new();
            world.register_imported_asset::<Note>();
            world.register_standalone_asset::<Card>();
            Self {
                root,
                world,
                _mount_guard: mounted,
            }
        }

        fn write(&self, relative_path: &str, text: &str) {
            let path = self.root.join(relative_path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        /// Writes a note at `relative_path` together with a `.meta` that holds
        /// `guid`.
        fn note(&self, relative_path: &str, guid: &str) {
            self.write(relative_path, "hello");
            self.write(
                &format!("{relative_path}.meta"),
                &header("pill_engine::asset_database::tests::Note", guid, "settings"),
            );
        }

        fn registry(&self) -> ImportRegistry {
            self.world.get_resource::<ImportRegistry>().unwrap().clone()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Builds a header document naming `asset_type` and `guid`, with an empty
    /// body under `body_field`.
    fn header(asset_type: &str, guid: &str, body_field: &str) -> String {
        format!(
            r#"{{"format_version": 1, "asset_type": "{asset_type}", "guid": "{guid}", "{body_field}": {{}}}}"#
        )
    }

    fn guid(text: &str) -> AssetGuid {
        AssetGuid::parse(text).unwrap()
    }

    #[test]
    fn a_directory_indexes_sourced_and_standalone_assets() {
        let scratch = Scratch::new("index");
        scratch.note("notes/a.note", NOTE_GUID);
        scratch.write(
            "cards/b.card",
            &header(
                "pill_engine::asset_database::tests::Card",
                CARD_GUID,
                "asset",
            ),
        );
        // A source without a `.meta` has no guid yet, and a `.meta` is never
        // an asset of its own.
        scratch.write("notes/unpaired.note", "no sidecar");

        let database = AssetDatabase::from_directory(&scratch.root, &scratch.registry());
        assert_eq!(database.len(), 2);
        let note = database
            .by_guid(guid(NOTE_GUID))
            .expect("the note is indexed");
        assert_eq!(note.path, "notes/a.note");
        assert_eq!(note.asset_type, "pill_engine::asset_database::tests::Note");
        assert!(!note.standalone);
        let card = database
            .by_path("cards\\b.card")
            .expect("either separator works");
        assert_eq!(card.guid, guid(CARD_GUID));
        assert!(card.standalone);
        assert_eq!(
            database
                .records_of_type("pill_engine::asset_database::tests::Card")
                .count(),
            1
        );
    }

    #[test]
    fn a_guid_claimed_twice_keeps_the_first_path() {
        let scratch = Scratch::new("duplicate");
        scratch.note("a.note", NOTE_GUID);
        scratch.note("b.note", NOTE_GUID);

        let database = AssetDatabase::from_directory(&scratch.root, &scratch.registry());
        assert_eq!(database.by_guid(guid(NOTE_GUID)).unwrap().path, "a.note");
        assert!(database.by_path("b.note").is_none());
        assert_eq!(
            database.conflicts(),
            &[GuidConflict {
                guid: guid(NOTE_GUID),
                kept: "a.note".to_owned(),
                refused: "b.note".to_owned(),
            }]
        );
    }

    #[test]
    fn refresh_follows_a_new_file_a_move_and_a_delete() {
        let scratch = Scratch::new("refresh");
        let registry = scratch.registry();
        let mut database = AssetDatabase::from_directory(&scratch.root, &registry);
        assert!(database.is_empty());

        // New: a note and its `.meta` appear.
        scratch.note("a.note", NOTE_GUID);
        database.refresh(&scratch.root, "a.note", &registry);
        assert_eq!(database.by_guid(guid(NOTE_GUID)).unwrap().path, "a.note");

        // Moved with its `.meta`: the watcher reports the new path first. It
        // takes the guid over because the old path is gone.
        std::fs::create_dir_all(scratch.root.join("moved")).unwrap();
        std::fs::rename(
            scratch.root.join("a.note"),
            scratch.root.join("moved/a.note"),
        )
        .unwrap();
        std::fs::rename(
            scratch.root.join("a.note.meta"),
            scratch.root.join("moved/a.note.meta"),
        )
        .unwrap();
        database.refresh(&scratch.root, "moved/a.note", &registry);
        database.refresh(&scratch.root, "a.note", &registry);
        assert_eq!(
            database.by_guid(guid(NOTE_GUID)).unwrap().path,
            "moved/a.note"
        );
        assert!(database.by_path("a.note").is_none());
        assert!(database.conflicts().is_empty());

        // Deleted: it leaves the index.
        std::fs::remove_file(scratch.root.join("moved/a.note")).unwrap();
        database.refresh(&scratch.root, "moved/a.note", &registry);
        assert!(database.by_guid(guid(NOTE_GUID)).is_none());
    }

    #[test]
    fn a_copy_with_its_meta_is_refused_while_the_original_exists() {
        let scratch = Scratch::new("copy");
        let registry = scratch.registry();
        scratch.note("a.note", NOTE_GUID);
        let mut database = AssetDatabase::from_directory(&scratch.root, &registry);
        scratch.note("copy.note", NOTE_GUID);
        database.refresh(&scratch.root, "copy.note", &registry);
        assert_eq!(database.by_guid(guid(NOTE_GUID)).unwrap().path, "a.note");
        assert_eq!(database.conflicts().len(), 1);
    }

    #[test]
    fn an_unloaded_asset_loads_by_guid_and_by_path_to_the_same_handle() {
        let mut scratch = Scratch::new("load");
        scratch.note("a.note", NOTE_GUID);
        let registry = scratch.registry();
        let database = AssetDatabase::from_directory(&scratch.root, &registry);
        let assets = scratch.world.get_resource_mut::<AssetManager>().unwrap();
        assert!(assets.handle_by_guid::<Note>(guid(NOTE_GUID)).is_none());

        let by_guid = registry
            .load_by_guid(
                assets,
                &database,
                guid(NOTE_GUID),
                MetadataPolicy::ReadIfPresent,
            )
            .expect("the guid resolves to a path and imports");
        assert!(!by_guid.previously_loaded());
        let by_path = registry
            .load_by_path(assets, Path::new("a.note"), MetadataPolicy::ReadIfPresent)
            .expect("the path imports");
        assert!(by_path.previously_loaded());
        assert_eq!(
            (by_guid.index, by_guid.generation),
            (by_path.index, by_path.generation)
        );
        let handle = registry
            .load_handle_by_guid::<Note>(
                assets,
                &database,
                guid(NOTE_GUID),
                MetadataPolicy::ReadIfPresent,
            )
            .expect("the typed form finds the same asset");
        assert_eq!(
            (handle.index(), handle.generation()),
            (by_path.index, by_path.generation)
        );
        assert_eq!(
            assets.get(handle).map(|note| note.0.as_str()),
            Some("hello")
        );
    }

    #[test]
    fn a_handle_names_its_asset_through_the_registry() {
        let mut scratch = Scratch::new("identity");
        scratch.note("a.note", NOTE_GUID);
        let registry = scratch.registry();
        let database = AssetDatabase::from_directory(&scratch.root, &registry);
        let assets = scratch.world.get_resource_mut::<AssetManager>().unwrap();
        let loaded = registry
            .load_by_guid(
                assets,
                &database,
                guid(NOTE_GUID),
                MetadataPolicy::ReadIfPresent,
            )
            .unwrap();
        let note_type = "pill_engine::asset_database::tests::Note";
        let identity = registry
            .asset_identity(assets, note_type, loaded.index, loaded.generation)
            .expect("a live handle has an identity");
        assert_eq!(identity.guid, Some(guid(NOTE_GUID)));
        assert_eq!(identity.name.as_deref(), Some("a.note"));
        assert!(
            registry
                .asset_identity(assets, note_type, loaded.index, loaded.generation + 1)
                .is_none(),
            "a stale handle has none"
        );
        assert_eq!(
            assets.registered_asset_type(std::any::type_name::<Note>()),
            Some(std::any::type_name::<Note>()),
            "a type without a shared name is registered under its Rust type name"
        );
    }

    #[test]
    fn an_unknown_guid_or_a_wrong_type_is_an_error() {
        let mut scratch = Scratch::new("errors");
        scratch.note("a.note", NOTE_GUID);
        let registry = scratch.registry();
        let database = AssetDatabase::from_directory(&scratch.root, &registry);
        let assets = scratch.world.get_resource_mut::<AssetManager>().unwrap();
        let unknown = registry.load_by_guid(
            assets,
            &database,
            guid(CARD_GUID),
            MetadataPolicy::ReadIfPresent,
        );
        assert!(matches!(
            unknown,
            Err(crate::ErasedImportError::UnknownGuid { .. })
        ));
        let wrong_type = registry.load_handle_by_guid::<Card>(
            assets,
            &database,
            guid(NOTE_GUID),
            MetadataPolicy::ReadIfPresent,
        );
        assert!(matches!(
            wrong_type,
            Err(crate::ErasedImportError::WrongType { .. })
        ));
    }

    #[test]
    fn a_pack_indexes_its_meta_entries() {
        let pack_bytes = pack(&[
            ("a.note", b"hello".to_vec()),
            (
                "a.note.meta",
                header(
                    "pill_engine::asset_database::tests::Note",
                    NOTE_GUID,
                    "settings",
                )
                .into_bytes(),
            ),
            ("loose.txt", b"not an asset".to_vec()),
            (
                "b.card",
                header(
                    "pill_engine::asset_database::tests::Card",
                    CARD_GUID,
                    "asset",
                )
                .into_bytes(),
            ),
        ]);
        let pack = AssetPack::parse(pack_bytes).unwrap();
        let database = AssetDatabase::from_pack(&pack);
        assert_eq!(
            database.len(),
            2,
            "the note and the standalone card; the loose text is not an asset"
        );
        let note = database.by_guid(guid(NOTE_GUID)).unwrap();
        assert_eq!((note.path.as_str(), note.standalone), ("a.note", false));
        let card = database.by_guid(guid(CARD_GUID)).unwrap();
        assert_eq!((card.path.as_str(), card.standalone), ("b.card", true));
    }

    /// A metadata document that a crashed `write_beside` left behind is a
    /// dot-file, and a dot-file is not an asset. It must not be indexed and
    /// must not take the guid that its own asset holds.
    #[test]
    fn a_packed_metadata_temp_file_does_not_steal_the_guid() {
        let meta = header(
            "pill_engine::asset_database::tests::Note",
            NOTE_GUID,
            "settings",
        )
        .into_bytes();
        let pack_bytes = pack(&[
            ("textures/.a.note.meta.4242-0.tmp", meta.clone()),
            ("textures/a.note", b"hello".to_vec()),
            ("textures/a.note.meta", meta),
        ]);
        let pack = AssetPack::parse(pack_bytes).unwrap();
        let database = AssetDatabase::from_pack(&pack);

        assert_eq!(database.len(), 1, "only the note is an asset");
        let note = database.by_guid(guid(NOTE_GUID)).unwrap();
        assert_eq!(note.path, "textures/a.note");
        assert!(
            database.conflicts().is_empty(),
            "the temp file must not claim the guid first"
        );
    }

    /// Builds the bytes of a version-1 pack that holds `files`, laid out the
    /// way the pack writer does (see the pack format section of
    /// `asset_store.rs`).
    fn pack(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut index_length = 0usize;
        for (path, _) in files {
            index_length += 4 + path.len() + 16;
        }
        let header_length = crate::asset_store::ASSET_PACK_MAGIC.len() + 8;
        let mut offset = header_length + index_length;
        let mut bytes = crate::asset_store::ASSET_PACK_MAGIC.to_vec();
        bytes.extend_from_slice(&crate::asset_store::ASSET_PACK_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for (path, content) in files {
            bytes.extend_from_slice(&(path.len() as u32).to_le_bytes());
            bytes.extend_from_slice(path.as_bytes());
            bytes.extend_from_slice(&(offset as u64).to_le_bytes());
            bytes.extend_from_slice(&(content.len() as u64).to_le_bytes());
            offset += content.len();
        }
        for (_, content) in files {
            bytes.extend_from_slice(content);
        }
        bytes
    }
}
