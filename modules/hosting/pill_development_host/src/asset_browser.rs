//! The project's assets as an editor or another tool sees them: the `res`
//! tree, plus each file's asset type, load state and guid.
//!
//! # Responsibilities
//!
//! - List the project's `res` and every module root as a tree ([`list_assets`]):
//!   the `.meta` sidecars are hidden, and every file carries its type, load
//!   state and guid.
//! - Read and save an asset's import settings as JSON, or a standalone asset's
//!   own document ([`asset_settings`], [`save_asset_settings`]), and move an
//!   asset together with its `.meta` ([`move_asset`]).
//! - Create a standalone asset from its type's default document, checking the
//!   name and the folder here instead of in a UI ([`create_standalone_asset`]).
//! - Refuse every path outside `res`. A module's resources are listed read-only,
//!   and the write functions change only the project's own `res`.
//!
//! # Design
//!
//! Nothing in this module names an asset type. Every asset type is handled
//! through the world's [`ImportRegistry`](pill_engine::ImportRegistry), so a
//! type that a module registers later is listed and edited without a change
//! here or in the editor. Each call imports at most once: saving writes the
//! file (and imports a sourced asset whose `.meta` is still missing), and a
//! move renames files. The dev host's asset watcher then sees the change - or
//! follows the move - and reimports at the next frame boundary, just as it
//! does for an edit made in another program.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use pill_engine::asset_metadata::METADATA_FORMAT_VERSION;
use pill_engine::{AssetGuid, AssetManager, ImportRegistry, MetadataPolicy, World};

// =============================================================================
// Re-exports
// =============================================================================

// These two rules for asset names and paths belong to the asset format. Every
// writer uses them; the editor core's asset commands call the same pair.
pub use pill_engine::asset_metadata::{checked_relative, standalone_file_name};

// =============================================================================
// Constants
// =============================================================================

/// The suffix of a metadata sidecar.
const METADATA_SUFFIX: &str = ".meta";

// =============================================================================
// Types
// =============================================================================

/// One row of an asset tree listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetEntry {
    /// The root the entry came from: empty for the project's own `res`, or
    /// otherwise the name of the module whose resources it is.
    pub root: String,
    /// Whether the entry belongs to a module's resources: it is listed and
    /// shown, but never written.
    pub read_only: bool,
    /// The path relative to its root, with `/` separators. A folder's path has
    /// no trailing slash.
    pub path: String,
    /// The last component of [`Self::path`].
    pub name: String,
    /// How many folders deep the entry is: `0` for an entry directly in its
    /// root.
    pub depth: usize,
    /// Whether the entry is a folder.
    pub is_directory: bool,
    /// The registered asset type that imports the file, if any.
    pub type_name: Option<String>,
    /// Whether the file is a standalone asset, so it holds its own guid.
    pub standalone: bool,
    /// Whether an asset is loaded under this path.
    pub loaded: bool,
    /// The guid held by the file's `.meta`, or by its own header, if any.
    pub guid: Option<String>,
}

/// A standalone asset type that a new file can be created for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneType {
    /// The registered type name, written to the file's `asset_type`.
    pub type_name: String,
    /// The last `::` segment of the type name, shown in a dropdown.
    pub display_name: String,
    /// The file extension without the dot (`render_pass`).
    pub extension: String,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Lists every folder and file under `asset_directory`, depth first: folders
/// come before files, and each level is sorted by name. `.meta` files and
/// dot-files are left out.
///
/// The project's tree comes first. After it comes one read-only tree per
/// loaded module's resources ([`pill_engine::asset_store::module_roots`]), and
/// every entry names its root so a UI can tell the trees apart.
pub fn list_assets(world: &World, asset_directory: &Path) -> Vec<AssetEntry> {
    // Step 1: List the project's own tree first; it is the only one a tool may
    // write.
    let mut entries = Vec::new();
    append_root(world, "", asset_directory, false, &[], &mut entries);
    // Step 2: Add one read-only tree for each module root. Every entry carries
    // the module's name, so a UI can group the files one module declared.
    for root in pill_engine::asset_store::module_roots() {
        append_root(
            world,
            &root.module,
            &root.directory,
            true,
            &root.hidden,
            &mut entries,
        );
    }
    entries
}

/// Walks one root and pushes its entries, naming the root and marking whether
/// the entries are read-only.
///
/// `hidden` names the extensions the root leaves out of its tree, such as a
/// module's cooked output beside its sources. The store still answers those
/// files by name.
fn append_root(
    world: &World,
    root_label: &str,
    directory: &Path,
    read_only: bool,
    hidden: &[String],
    entries: &mut Vec<AssetEntry>,
) {
    let registry = world.get_resource::<ImportRegistry>();
    let asset_manager = world.get_resource::<AssetManager>();
    let database = world.get_resource::<pill_engine::AssetDatabase>();
    visit(
        directory,
        Path::new(""),
        0,
        &mut |relative, is_directory, depth| {
            let path = slash_path(relative);
            let name = relative
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut entry = AssetEntry {
                root: root_label.to_owned(),
                read_only,
                path,
                name,
                depth,
                is_directory,
                type_name: None,
                standalone: false,
                loaded: false,
                guid: None,
            };
            if !is_directory {
                let extension = relative
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default();
                // A cooked file beside its source is a build product: the
                // store reads it by name, but the tree does not show it.
                if hidden
                    .iter()
                    .any(|hidden| hidden.eq_ignore_ascii_case(extension))
                {
                    return;
                }
                if let Some(registry) = registry {
                    entry.type_name = registry.type_for_extension(extension).map(str::to_owned);
                    entry.standalone = registry.is_standalone_extension(extension);
                    entry.loaded = asset_manager
                        .is_some_and(|asset_manager| registry.is_loaded(asset_manager, relative));
                }
                // The asset database knows every guid it indexed. Reading the
                // file that carries the guid - a standalone's own header or a
                // source's `.meta` - also covers a host without a database and
                // a file that was indexed late.
                entry.guid = database
                    .and_then(|database| database.by_path(&entry.path))
                    .map(|record| record.guid.to_string())
                    .or_else(|| {
                        let header = if entry.standalone {
                            directory.join(relative)
                        } else {
                            directory.join(format!("{}{METADATA_SUFFIX}", relative.display()))
                        };
                        read_guid(&header)
                    });
            }
            entries.push(entry);
        },
    );
}

/// Walks `directory/relative` recursively and calls `visit_entry(relative,
/// is_directory, depth)` for each folder and file, in the order
/// [`list_assets`] promises.
fn visit(
    directory: &Path,
    relative: &Path,
    depth: usize,
    visit_entry: &mut dyn FnMut(&Path, bool, usize),
) {
    // A directory that cannot be read has nothing to list, so the caller's
    // tree simply leaves it out.
    let Ok(directory_entries) = std::fs::read_dir(directory.join(relative)) else {
        return;
    };
    // Step 1: Split this level into folders and files, and drop the sidecars
    // and dot-files that a tree never shows.
    let mut folders = Vec::new();
    let mut files = Vec::new();
    for entry in directory_entries.flatten() {
        let name = entry.file_name();
        let file_name_text = name.to_string_lossy();
        if file_name_text.starts_with('.') || file_name_text.ends_with(METADATA_SUFFIX) {
            continue;
        }
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => folders.push(relative.join(&name)),
            Ok(file_type) if file_type.is_file() => files.push(relative.join(&name)),
            _ => {}
        }
    }
    // Step 2: Report each folder and then descend into it, then report the
    // files. This keeps folders before files at every level, and both are
    // sorted by name.
    folders.sort();
    files.sort();
    for folder in folders {
        visit_entry(&folder, true, depth);
        visit(directory, &folder, depth + 1, visit_entry);
    }
    for file in files {
        visit_entry(&file, false, depth);
    }
}

/// Returns the `guid` field of the asset header stored in `path`, if the file
/// has one.
fn read_guid(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let header_document = pill_engine::asset_document::parse(&bytes).ok()?;
    header_document.get("guid")?.as_str().map(str::to_owned)
}

/// Returns `path` as a string with `/` separators.
fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Returns the import settings of the asset at `path` as JSON, or the
/// standalone asset's own document. The value is what the file holds, or the
/// type's defaults when the file holds nothing yet.
///
/// # Errors
///
/// A message when the path leaves `res`, when no asset types are registered,
/// when no registered type imports the file, or when the file cannot be read.
pub fn asset_settings(world: &World, path: &str) -> Result<serde_json::Value, String> {
    let relative = checked_relative(path)?;
    let registry = world
        .get_resource::<ImportRegistry>()
        .ok_or("no asset types are registered")?;
    registry
        .settings_json(&relative)
        .map_err(|error| error.to_string())
}

/// Saves `settings` as the import settings of the asset at `path`, or as a
/// standalone asset's own document, and returns the normalized value that was
/// written.
///
/// The value is checked against the type first: missing fields get their
/// defaults and unknown ones are dropped, so a malformed edit is refused
/// before anything is written. A sourced asset without a `.meta` is imported
/// first, which writes one. The file is replaced atomically, and the asset
/// watcher then reimports the asset in place.
///
/// # Errors
///
/// A message when the path leaves `res`, when no asset types are registered,
/// when no type imports the file, when the settings do not fit the type, when
/// the source cannot be imported (which is what creates a missing `.meta`), or
/// when the file cannot be read, parsed or written.
pub fn save_asset_settings(
    world: &mut World,
    asset_directory: &Path,
    path: &str,
    settings: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // Step 1: Refuse a path outside `res` and find the type that imports the
    // file.
    let relative = checked_relative(path)?;
    let registry = world
        .get_resource::<ImportRegistry>()
        .cloned()
        .ok_or("no asset types are registered")?;
    let extension = relative
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_owned();
    let type_name = registry
        .type_for_extension(&extension)
        .ok_or_else(|| format!("no asset type imports `.{extension}` files"))?
        .to_owned();
    // Step 2: Normalize the value against the type, so a malformed edit is
    // refused before anything is written.
    let normalized = registry
        .normalize_settings_json(&type_name, settings)
        .map_err(|error| error.to_string())?;

    // Step 3: Choose where the settings are stored: the standalone file's own
    // document, or the `.meta` beside its source. A missing `.meta` is created
    // by importing the source once.
    let standalone = registry.is_standalone_extension(&extension);
    let (settings_file, body_field) = if standalone {
        (asset_directory.join(&relative), "asset")
    } else {
        (
            asset_directory.join(format!("{}{METADATA_SUFFIX}", relative.display())),
            "settings",
        )
    };
    if !standalone && !settings_file.is_file() {
        // The import writes a `.meta` with the type's defaults and a guid. The
        // settings written below replace the defaults; the guid stays.
        let asset_manager = world
            .get_resource_mut::<AssetManager>()
            .ok_or("the engine has no AssetManager")?;
        registry
            .import(asset_manager, &relative, MetadataPolicy::CreateIfMissing)
            .map_err(|error| error.to_string())?;
    }

    // Step 4: Replace only the body field and keep the rest of the header. The
    // result is put in place atomically.
    let file_bytes = std::fs::read(&settings_file)
        .map_err(|error| format!("cannot read `{}`: {error}", settings_file.display()))?;
    let mut settings_document = pill_engine::asset_document::parse(&file_bytes)
        .map_err(|error| format!("`{}`: {error}", settings_file.display()))?;
    let settings_object = settings_document
        .as_object_mut()
        .ok_or_else(|| format!("`{}` is not a YAML mapping", settings_file.display()))?;
    settings_object.insert(body_field.to_owned(), normalized.clone());
    let bytes = pill_engine::asset_document::render(&settings_document)
        .map_err(|error| error.to_string())?;
    replace_atomically(&settings_file, &bytes)?;
    Ok(normalized)
}

/// Writes `bytes` to a temporary sibling of `target_file`, then renames it over
/// `target_file`, so a reader never sees a half-written file.
///
/// # Errors
///
/// A message when `target_file` has no name, when the temporary sibling cannot
/// be written, or when the rename fails.
fn replace_atomically(target_file: &Path, bytes: &[u8]) -> Result<(), String> {
    let target_file_name = target_file
        .file_name()
        .ok_or_else(|| format!("`{}` has no file name", target_file.display()))?;
    let temporary_file = target_file.with_file_name(format!(
        ".{}.{}.tmp",
        target_file_name.to_string_lossy(),
        std::process::id()
    ));
    std::fs::write(&temporary_file, bytes)
        .map_err(|error| format!("cannot write `{}`: {error}", temporary_file.display()))?;
    std::fs::rename(&temporary_file, target_file).map_err(|error| {
        let _ = std::fs::remove_file(&temporary_file);
        format!("cannot replace `{}`: {error}", target_file.display())
    })
}

/// Moves the asset at `from` to `to` (both relative to `res`) together with
/// its `.meta`, through [`pill_assets::move_asset`]. The asset watcher then
/// follows the move, so the loaded asset keeps its handle and guid.
///
/// # Errors
///
/// A message when either path leaves `res`, when the move is refused (the
/// source is missing or is a `.meta` file, or the target exists), or when a
/// rename fails.
pub fn move_asset(asset_directory: &Path, from: &str, to: &str) -> Result<(), String> {
    let from = checked_relative(from)?;
    let to = checked_relative(to)?;
    pill_assets::move_asset(&asset_directory.join(from), &asset_directory.join(to))
        .map_err(|error| error.to_string())
}

/// Returns the live standalone types, sorted by display name: the list a
/// "create asset" dropdown offers.
///
/// Sourced types (textures, meshes, sounds) are not listed, because they come
/// from adding a source file to `res`.
pub fn standalone_types(world: &World) -> Vec<StandaloneType> {
    let Some(registry) = world.get_resource::<ImportRegistry>() else {
        return Vec::new();
    };
    let mut types: Vec<StandaloneType> = registry
        .standalone_types()
        .into_iter()
        .map(|(type_name, extension)| StandaloneType {
            display_name: type_name
                .rsplit("::")
                .next()
                .unwrap_or(&type_name)
                .to_owned(),
            type_name,
            extension,
        })
        .collect();
    types.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    types
}

/// Creates a new standalone asset of `type_name` called `name` in `folder`
/// (relative to `res`; empty for `res` itself) and loads it. Returns its path
/// relative to `res`.
///
/// The file holds the standard header (format version, type name and a new
/// random guid) and the type's default document, which is a valid asset on its
/// own. It is written with [`pill_engine::asset_store::write_new`], so an
/// existing file is never replaced, and then imported through the registry, so
/// the caller gets a loaded asset at once. When the asset watcher sees the new
/// file later, it finds the asset already loaded and reimports it in place.
///
/// # Errors
///
/// A message when no asset types are registered, when the type is not a live
/// standalone type, when the folder leaves `res` or does not exist, when the
/// name is refused (see [`standalone_file_name`]), when the file already
/// exists, when the type's default document cannot be built, when no random
/// source exists for the new guid, or when the file cannot be written or
/// loaded.
pub fn create_standalone_asset(
    world: &mut World,
    asset_directory: &Path,
    type_name: &str,
    folder: &str,
    name: &str,
) -> Result<String, String> {
    // Step 1: Find the extension of the type being created; only a live
    // standalone type has one.
    let registry = world
        .get_resource::<ImportRegistry>()
        .cloned()
        .ok_or("no asset types are registered")?;
    let (_, extension) = registry
        .standalone_types()
        .into_iter()
        .find(|(registered, _)| registered == type_name)
        .ok_or_else(|| format!("`{type_name}` is not a standalone asset type"))?;

    // Step 2: Check the folder, the name and the target before anything is
    // written, so a refusal leaves `res` untouched.
    let folder = folder.trim().trim_matches('/');
    let relative_folder = if folder.is_empty() {
        PathBuf::new()
    } else {
        checked_relative(folder)?
    };
    let directory = asset_directory.join(&relative_folder);
    if !directory.is_dir() {
        return Err(format!(
            "the folder `res/{}` does not exist",
            slash_path(&relative_folder)
        ));
    }
    let file_name = standalone_file_name(name, &extension)?;
    let relative = relative_folder.join(&file_name);
    let destination = asset_directory.join(&relative);
    if destination.exists() {
        return Err(format!("`res/{}` already exists", slash_path(&relative)));
    }

    // Step 3: Write the standard header and the type's default document.
    // `write_new` creates the file in one step and never replaces an existing
    // one.
    let document = registry
        .default_document(type_name)
        .map_err(|error| error.to_string())?;
    let guid =
        AssetGuid::random().map_err(|error| format!("no random source for a guid: {error}"))?;
    let asset_document = serde_json::json!({
        "format_version": METADATA_FORMAT_VERSION,
        "asset_type": type_name,
        "guid": guid,
        "asset": document,
    });
    let bytes =
        pill_engine::asset_document::render(&asset_document).map_err(|error| error.to_string())?;
    pill_engine::asset_store::write_new(&destination, &bytes).map_err(|error| error.to_string())?;

    // Step 4: Import the finished file, so the caller holds a loaded asset
    // instead of waiting for the watcher to notice it.
    let asset_manager = world
        .get_resource_mut::<AssetManager>()
        .ok_or("the engine has no AssetManager")?;
    registry
        .import(asset_manager, &relative, MetadataPolicy::ReadIfPresent)
        .map_err(|error| {
            format!(
                "`res/{}` was written but did not load: {error}",
                slash_path(&relative)
            )
        })?;
    Ok(slash_path(&relative))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::{ImportedAsset, StandaloneAsset};

    /// Serializes the tests that mount a scratch `res`. The mount is one value
    /// for the whole process, and tests run on parallel threads.
    fn mount_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A standalone test asset: a note holding some text.
    #[derive(Debug)]
    struct Note {
        text: String,
    }
    impl pill_engine::Asset for Note {}
    trait_type_map::impl_trait_accessible!(dyn pill_engine::Asset; Note);

    /// `Note`'s saved document: the text a new note starts with.
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    #[serde(default)]
    struct NoteDocument {
        text: String,
    }

    impl Default for NoteDocument {
        fn default() -> Self {
            Self {
                text: "new note".to_owned(),
            }
        }
    }

    impl pill_engine::StandaloneAsset for Note {
        type Document = NoteDocument;
        const FILE_EXTENSION: &'static str = "note";

        fn metadata_type_name() -> &'static str {
            "pill_development_host::asset_browser::tests::Note"
        }

        fn from_document(
            _name: &str,
            document: NoteDocument,
            _assets: &AssetManager,
        ) -> pill_engine::AssetLoadResult<Self> {
            Ok(Self {
                text: document.text,
            })
        }
    }

    /// Creates a world with the standalone `Note` and the sourced `Swatch`
    /// registered, over a scratch `res` that holds a `notes` folder.
    fn create_fixture(label: &str) -> (World, PathBuf) {
        // Callers hold `mount_lock` for the whole test.
        let root =
            std::env::temp_dir().join(format!("pill-asset-create-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("notes")).unwrap();
        pill_engine::asset_store::mount_directory(&root);
        let mut world = World::new();
        world.insert_resource(AssetManager::new());
        world.register_standalone_asset::<Note>();
        world.register_imported_asset::<Swatch>();
        (world, root)
    }

    /// Only the live standalone type is offered, not the sourced one.
    #[test]
    fn only_standalone_types_are_offered() {
        let _mounted = mount_lock();
        let (world, root) = create_fixture("types");
        let types = standalone_types(&world);
        assert_eq!(
            types,
            [StandaloneType {
                type_name: Note::metadata_type_name().to_owned(),
                display_name: "Note".to_owned(),
                extension: "note".to_owned(),
            }]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A loaded module's resources come after the project's own entries. Each
    /// one is labelled with its module and marked read-only.
    #[test]
    fn a_module_root_is_listed_read_only() {
        let _mounted = mount_lock();
        let (world, root) = create_fixture("module-root");
        let module_directory =
            std::env::temp_dir().join(format!("pill-module-res-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&module_directory);
        std::fs::create_dir_all(module_directory.join("overlay")).unwrap();
        std::fs::write(module_directory.join("overlay/grid.wgsl"), b"grid").unwrap();
        pill_engine::asset_store::declare_module_root(
            "pill_test_module",
            &module_directory,
            &["overlay/grid.wgsl"],
        );

        let entries = list_assets(&world, &root);

        assert!(
            entries
                .iter()
                .filter(|entry| entry.root.is_empty())
                .all(|entry| !entry.read_only),
            "the project's own assets stay editable"
        );
        let module_entries: Vec<&AssetEntry> = entries
            .iter()
            .filter(|entry| entry.root == "pill_test_module")
            .collect();
        assert!(!module_entries.is_empty(), "the module root is listed");
        assert!(
            module_entries.iter().all(|entry| entry.read_only),
            "every module entry is read-only"
        );
        assert!(module_entries
            .iter()
            .any(|entry| entry.path == "overlay/grid.wgsl" && !entry.is_directory));

        pill_engine::asset_store::forget_all_module_roots();
        std::fs::remove_dir_all(&module_directory).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A module contributing two directories lists both under its own name, so
    /// the panel shows one module row with everything it ships.
    #[test]
    fn a_module_with_two_roots_lists_one_tree() {
        let _mounted = mount_lock();
        let (world, root) = create_fixture("module-two-roots");
        let resources =
            std::env::temp_dir().join(format!("pill-module-res-{}", std::process::id()));
        let sources = std::env::temp_dir().join(format!("pill-module-src-{}", std::process::id()));
        for directory in [&resources, &sources] {
            let _ = std::fs::remove_dir_all(directory);
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(resources.join("default.material"), b"{}").unwrap();
        std::fs::create_dir_all(sources.join("pipeline")).unwrap();
        std::fs::write(sources.join("pipeline/fragment.hlsl"), b"//").unwrap();

        pill_engine::asset_store::declare_module_root(
            "pill_two_roots",
            &resources,
            &["default.material"],
        );
        pill_engine::asset_store::declare_module_root(
            "pill_two_roots",
            &sources,
            &["pipeline/fragment.hlsl"],
        );

        let entries = list_assets(&world, &root);
        let paths: Vec<&str> = entries
            .iter()
            .filter(|entry| entry.root == "pill_two_roots" && !entry.is_directory)
            .map(|entry| entry.path.as_str())
            .collect();
        assert!(paths.contains(&"default.material"), "{paths:?}");
        assert!(paths.contains(&"pipeline/fragment.hlsl"), "{paths:?}");

        pill_engine::asset_store::forget_all_module_roots();
        std::fs::remove_dir_all(&resources).unwrap();
        std::fs::remove_dir_all(&sources).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A root's hidden extensions stay out of the tree, such as a module's
    /// cooked output beside its sources, while the store still answers them by
    /// name.
    #[test]
    fn a_root_keeps_its_hidden_extensions_out_of_the_tree() {
        let _mounted = mount_lock();
        let (world, root) = create_fixture("module-hidden");
        let sources =
            std::env::temp_dir().join(format!("pill-module-hidden-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sources);
        std::fs::create_dir_all(&sources).unwrap();
        std::fs::write(sources.join("fragment.hlsl"), b"//").unwrap();
        std::fs::write(sources.join("fragment.wgsl"), b"//").unwrap();

        pill_engine::asset_store::declare_module_root_hiding(
            "pill_hidden_module",
            &sources,
            &["fragment.hlsl"],
            &["wgsl"],
        );

        let entries = list_assets(&world, &root);
        let paths: Vec<&str> = entries
            .iter()
            .filter(|entry| entry.root == "pill_hidden_module")
            .map(|entry| entry.path.as_str())
            .collect();
        assert!(paths.contains(&"fragment.hlsl"), "{paths:?}");
        assert!(!paths.contains(&"fragment.wgsl"), "{paths:?}");

        pill_engine::asset_store::forget_all_module_roots();
        std::fs::remove_dir_all(&sources).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The created file holds the header and the type's default document, gets
    /// its own guid, and is loaded before the call returns.
    #[test]
    fn a_created_asset_has_a_header_a_fresh_guid_and_is_loaded() {
        let _mounted = mount_lock();
        let (mut world, root) = create_fixture("create");

        let first = create_standalone_asset(
            &mut world,
            &root,
            Note::metadata_type_name(),
            "notes",
            "todo",
        )
        .unwrap();
        let second = create_standalone_asset(
            &mut world,
            &root,
            Note::metadata_type_name(),
            "",
            "idea.note",
        )
        .unwrap();

        assert_eq!(first, "notes/todo.note");
        assert_eq!(
            second, "idea.note",
            "an extension the user typed is not doubled"
        );
        let file_document = pill_engine::asset_document::parse(
            &std::fs::read(root.join("notes/todo.note")).unwrap(),
        )
        .expect("the file is YAML");
        assert_eq!(file_document["format_version"], METADATA_FORMAT_VERSION);
        assert_eq!(file_document["asset_type"], Note::metadata_type_name());
        assert_eq!(
            file_document["asset"],
            serde_json::json!({"text": "new note"})
        );
        let guid = file_document["guid"].as_str().unwrap();
        assert!(AssetGuid::parse(guid).is_some());
        let idea_document =
            pill_engine::asset_document::parse(&std::fs::read(root.join("idea.note")).unwrap())
                .expect("the file is YAML");
        assert_ne!(
            idea_document["guid"], file_document["guid"],
            "every file gets its own guid"
        );
        assert!(
            !root.join("notes/todo.note.meta").exists(),
            "a standalone file has no .meta"
        );

        let asset_manager = world.get_resource::<AssetManager>().unwrap();
        let note_handle = asset_manager
            .handle_by_name::<Note>("notes/todo.note")
            .expect("loaded at once");
        assert_eq!(asset_manager.get(note_handle).unwrap().text, "new note");
        assert_eq!(
            asset_manager.guid_of(note_handle).unwrap().to_string(),
            guid
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Each refused create names its reason, and an existing file keeps its
    /// bytes.
    #[test]
    fn creation_refuses_bad_input_and_never_overwrites() {
        let _mounted = mount_lock();
        let (mut world, root) = create_fixture("refuse");
        let note_type_name = Note::metadata_type_name();
        create_standalone_asset(&mut world, &root, note_type_name, "notes", "taken").unwrap();
        let original_bytes = std::fs::read(root.join("notes/taken.note")).unwrap();

        let refusals = [
            (note_type_name, "notes", "taken", "already exists"),
            (note_type_name, "../outside", "a", "not a path inside"),
            (note_type_name, "missing", "a", "does not exist"),
            (note_type_name, "notes", "", "empty"),
            (note_type_name, "notes", "a/b", "cannot contain"),
            (note_type_name, "notes", "a\\b", "cannot contain"),
            (note_type_name, "notes", "..", "cannot contain"),
            (note_type_name, "notes", "what?", "cannot contain"),
            (note_type_name, "notes", "CON", "reserves"),
            (note_type_name, "notes", "nul.note", "reserves"),
            (
                Swatch::metadata_type_name(),
                "notes",
                "a",
                "not a standalone",
            ),
        ];
        for (type_name, folder, name, expected) in refusals {
            let error =
                create_standalone_asset(&mut world, &root, type_name, folder, name).unwrap_err();
            assert!(error.contains(expected), "{folder}/{name}: {error}");
        }
        assert_eq!(
            std::fs::read(root.join("notes/taken.note")).unwrap(),
            original_bytes,
            "never overwritten"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A sourced test asset with one setting.
    #[derive(Debug)]
    struct Swatch;
    impl pill_engine::Asset for Swatch {}
    trait_type_map::impl_trait_accessible!(dyn pill_engine::Asset; Swatch);

    /// `Swatch`'s import settings: one switch a save has to round-trip.
    #[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
    #[serde(default)]
    struct SwatchSettings {
        bright: bool,
    }

    impl pill_engine::ImportedAsset for Swatch {
        type ImportSettings = SwatchSettings;
        const SOURCE_EXTENSIONS: &'static [&'static str] = &["swatch"];

        fn metadata_type_name() -> &'static str {
            "pill_development_host::asset_browser::tests::Swatch"
        }

        fn import(
            _name: &str,
            _bytes: &[u8],
            _settings: &SwatchSettings,
        ) -> pill_engine::AssetLoadResult<Self> {
            Ok(Self)
        }
    }

    /// Saves settings to the `.meta` (creating the file when it is missing),
    /// refuses settings that do not fit the type, and moves the `.meta` along
    /// with a renamed asset.
    #[test]
    fn settings_are_saved_to_the_metadata_file_and_moves_carry_it() {
        let _mounted = mount_lock();
        let root =
            std::env::temp_dir().join(format!("pill-asset-browser-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.swatch"), b"x").unwrap();
        pill_engine::asset_store::mount_directory(&root);
        let mut world = World::new();
        world.insert_resource(AssetManager::new());
        world.register_imported_asset::<Swatch>();

        let saved_settings = save_asset_settings(
            &mut world,
            &root,
            "a.swatch",
            serde_json::json!({"bright": true}),
        )
        .unwrap();
        assert_eq!(saved_settings, serde_json::json!({"bright": true}));
        let meta_document =
            pill_engine::asset_document::parse(&std::fs::read(root.join("a.swatch.meta")).unwrap())
                .expect("the sidecar is YAML");
        assert_eq!(
            meta_document["settings"],
            serde_json::json!({"bright": true})
        );
        let guid = meta_document["guid"].clone();
        assert_eq!(
            asset_settings(&world, "a.swatch").unwrap(),
            serde_json::json!({"bright": true})
        );

        // A malformed edit is refused and the file keeps what it had.
        assert!(save_asset_settings(
            &mut world,
            &root,
            "a.swatch",
            serde_json::json!({"bright": "yes"})
        )
        .is_err());
        assert!(
            save_asset_settings(&mut world, &root, "../a.swatch", serde_json::json!({})).is_err()
        );

        // A second save replaces the settings in place and keeps the guid.
        save_asset_settings(&mut world, &root, "a.swatch", serde_json::json!({})).unwrap();
        let meta_document =
            pill_engine::asset_document::parse(&std::fs::read(root.join("a.swatch.meta")).unwrap())
                .expect("the sidecar is YAML");
        assert_eq!(
            meta_document["settings"],
            serde_json::json!({"bright": false})
        );
        assert_eq!(meta_document["guid"], guid);

        move_asset(&root, "a.swatch", "moved/b.swatch").unwrap();
        assert!(
            root.join("moved/b.swatch").is_file() && root.join("moved/b.swatch.meta").is_file()
        );
        assert!(move_asset(&root, "moved/b.swatch", "../escape.swatch").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Every path that would leave `res` is refused, and Windows separators are
    /// turned into `/`.
    #[test]
    fn paths_outside_res_are_refused() {
        assert!(checked_relative("../secret.png").is_err());
        assert!(checked_relative("/abs/a.png").is_err());
        assert!(checked_relative("").is_err());
        assert!(checked_relative("C:\\\\a.png").is_err());
        assert_eq!(
            checked_relative("textures\\\\a.png").unwrap(),
            PathBuf::from("textures/a.png")
        );
    }

    /// The tree lists folders before files, depth first, and leaves out the
    /// `.meta` sidecars and dot-files.
    #[test]
    fn the_tree_lists_folders_first_and_hides_metadata() {
        // The listing appends every mounted module root after the project's own
        // tree, so a test that mounts one has to be finished before this runs.
        let _mounted = mount_lock();
        let root = std::env::temp_dir().join(format!("pill-asset-browser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("textures/nested")).unwrap();
        std::fs::write(root.join("textures/a.png"), b"x").unwrap();
        std::fs::write(
            root.join("textures/a.png.meta"),
            br#"{"guid": "0000000000000000000000000000000a"}"#,
        )
        .unwrap();
        std::fs::write(root.join("textures/nested/b.png"), b"x").unwrap();
        std::fs::write(root.join("readme.txt"), b"x").unwrap();
        std::fs::write(root.join(".hidden"), b"x").unwrap();
        let world = World::new();

        let entries = list_assets(&world, &root);
        let paths: Vec<(&str, usize, bool)> = entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry.depth, entry.is_directory))
            .collect();
        assert_eq!(
            paths,
            [
                ("textures", 0, true),
                ("textures/nested", 1, true),
                ("textures/nested/b.png", 2, false),
                ("textures/a.png", 1, false),
                ("readme.txt", 0, false),
            ]
        );
        let texture = entries
            .iter()
            .find(|entry| entry.path == "textures/a.png")
            .unwrap();
        assert_eq!(
            texture.guid.as_deref(),
            Some("0000000000000000000000000000000a")
        );
        assert!(!texture.loaded, "no registry, so nothing is loaded");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
