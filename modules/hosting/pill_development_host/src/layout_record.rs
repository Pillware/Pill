//! What a module's resources said their layouts are made of, and whether that
//! changed since the last load.
//!
//! # Responsibilities
//!
//! - Reads the layout declaration of every resource a generation claimed, and
//!   fingerprints the declared files' contents and the declared crates'
//!   versions from the workspace lockfile.
//! - Writes that record after a successful load, beside the artifact stamps,
//!   and reads it back before the next load.
//! - Reports which declared resources now differ from what the record holds, so
//!   the reload boundary can drop exactly those and let the module rebuild
//!   them.
//!
//! # Design
//!
//! A resource value outlives the module that inserted it. A reload keeps it and
//! only re-points its function table at the new image, which is sound exactly
//! while the two images agree on the type's layout. Once a field is added,
//! removed or retyped the bytes stay the same size and mean something else, so
//! the value has to be dropped and rebuilt instead.
//!
//! The module cannot stamp this itself: the wrapper crate the host generates is
//! a manifest and one `src/lib.rs`, with no build script. The host builds the
//! module, so the host is the one place that knows both the sources and the
//! binary, and it writes the record right after a load.
//!
//! The fingerprint must compare across two separately built artifacts, so it is
//! FNV-1a over the file contents and the declared versions. `DefaultHasher` is
//! not stable across compilers, which is the same reason the engine's schema
//! hash is FNV.
//!
//! A record declares the resource by the full path of its Rust type. A
//! reloaded generation gives the same type a new `TypeId`, so nothing else is
//! stable enough to match on.

// Standard library
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// External crates
use serde::{Deserialize, Serialize};

// Current crate
use pill_engine::{KeptLayoutDeclaration, ResourceId, World};

// =============================================================================
// Constants
// =============================================================================

/// Directory under the workspace root that holds one record per subject.
const LAYOUT_RECORD_DIRECTORY: &str = "pill_standalone_temp/layout_records";

/// Version of the record format.
///
/// A record written by a different version is discarded instead of compared: a
/// change to what is hashed would otherwise read as "every layout changed" and
/// drop every declared resource once.
pub(crate) const LAYOUT_RECORD_VERSION: u32 = 1;

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

// =============================================================================
// Types
// =============================================================================

/// Where a subject's record lives, and what its declared paths resolve against.
///
/// A declaration names its sources relative to the crate that declares the
/// resource, which is the parent of the watched `src` directory. The record is
/// keyed by the subject, so one module's record never answers for another's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LayoutRecordTarget<'a> {
    /// Root of the engine workspace: the lockfile and the record directory.
    workspace_root: &'a Path,
    /// Crate root of the subject: what a declared source path resolves against.
    source_directory: &'a Path,
    /// Subject name, which is also the record's file name.
    record_key: &'a str,
}

impl<'a> LayoutRecordTarget<'a> {
    /// Builds a target from the values a module configuration carries.
    ///
    /// `watch_directory` is the `src` directory the host watches, relative to
    /// the workspace root, so its parent is the crate root.
    pub(crate) fn new(
        workspace_root: &'a Path,
        watch_directory: &'a Path,
        record_key: &'a str,
    ) -> Self {
        let source_directory = match watch_directory.file_name() {
            Some(_) => watch_directory.parent().unwrap_or(watch_directory),
            None => watch_directory,
        };
        Self {
            workspace_root,
            source_directory,
            record_key,
        }
    }

    /// Path of this subject's record file.
    pub(crate) fn record_path(&self) -> PathBuf {
        self.workspace_root
            .join(LAYOUT_RECORD_DIRECTORY)
            .join(format!("{}.json", self.record_key))
    }

    /// Path of the workspace lockfile.
    ///
    /// Every artifact resolves its dependencies through this one file, so it is
    /// the authority on a declared crate's version.
    fn cargo_lock_path(&self) -> PathBuf {
        self.workspace_root.join("Cargo.lock")
    }
}

/// One declared source file and the hash of its contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceDigest {
    /// The path as the resource declared it.
    pub(crate) path: String,
    /// FNV-1a of the file's contents, or [`MISSING_SOURCE_HASH`] when the file
    /// could not be read.
    pub(crate) hash: u64,
}

/// One declared crate and the versions the lockfile resolved for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DependencyVersion {
    /// The crate name as the resource declared it.
    pub(crate) name: String,
    /// Versions in lockfile order, or empty when the lockfile lists none.
    pub(crate) versions: Vec<String>,
}

/// One resource's declared layout, as one load saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResourceLayoutRecord {
    /// Full path of the declaring Rust type.
    pub(crate) resource: String,
    /// The declared sources with their content hashes.
    pub(crate) sources: Vec<SourceDigest>,
    /// The declared crates with their resolved versions.
    pub(crate) dependencies: Vec<DependencyVersion>,
    /// One hash over both lists, which is what a later load compares.
    pub(crate) fingerprint: u64,
}

/// Every declared resource one subject claimed when it loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LayoutRecord {
    /// Format version the record was written with.
    pub(crate) version: u32,
    /// One entry per resource that declared a layout, sorted by resource.
    pub(crate) resources: Vec<ResourceLayoutRecord>,
}

impl LayoutRecord {
    /// An empty record, which compares equal to nothing but reads as "no
    /// declared resource" rather than as a failure.
    fn empty() -> Self {
        Self {
            version: LAYOUT_RECORD_VERSION,
            resources: Vec::new(),
        }
    }

    /// The entry recorded for one declaring type.
    fn layout_of(&self, resource: &str) -> Option<&ResourceLayoutRecord> {
        self.resources
            .iter()
            .find(|entry| entry.resource == resource)
    }
}

/// One declared resource whose layout differs from what the record holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayoutChange {
    /// Full path of the declaring type, for the warning.
    pub(crate) resource: String,
    /// The resource's world id, so the caller can release and drop it.
    pub(crate) id: ResourceId,
    /// Declared sources whose content differs, or that appeared or vanished.
    pub(crate) changed_sources: Vec<String>,
    /// Declared crates whose resolved version differs.
    pub(crate) changed_dependencies: Vec<String>,
}

/// Hash a file cannot be read at all.
///
/// Distinct from any content hash a file can produce, so a source that goes
/// missing reads as a change and one that comes back reads as a second one.
const MISSING_SOURCE_HASH: u64 = 0;

// =============================================================================
// Fingerprinting
// =============================================================================

/// Folds `bytes` into an FNV-1a hash.
fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Hash of one file's contents, or [`MISSING_SOURCE_HASH`] when it cannot be
/// read.
///
/// A file of length zero hashes to the offset basis, never to the sentinel, so
/// "empty" and "missing" stay apart.
pub(crate) fn hash_source_file(source_directory: &Path, declared_path: &str) -> u64 {
    match std::fs::read(source_directory.join(declared_path)) {
        Ok(contents) => fnv1a(FNV_OFFSET_BASIS, &contents),
        Err(_) => MISSING_SOURCE_HASH,
    }
}

/// One hash over a layout's sources and dependencies.
///
/// Both lists are sorted by name first, so the order a resource declares them
/// in cannot change the fingerprint. Adding a crate to the lockfile that no
/// resource declares changes nothing.
fn fold_layout(sources: &[SourceDigest], dependencies: &[DependencyVersion]) -> u64 {
    let mut sorted_sources: Vec<&SourceDigest> = sources.iter().collect();
    sorted_sources.sort_by(|left, right| left.path.cmp(&right.path));
    let mut sorted_dependencies: Vec<&DependencyVersion> = dependencies.iter().collect();
    sorted_dependencies.sort_by(|left, right| left.name.cmp(&right.name));

    let mut hash = fnv1a(FNV_OFFSET_BASIS, b"pill-layout-v1");
    for source in sorted_sources {
        hash = fnv1a(hash, source.path.as_bytes());
        hash = fnv1a(hash, &source.hash.to_le_bytes());
    }
    for dependency in sorted_dependencies {
        hash = fnv1a(hash, dependency.name.as_bytes());
        for version in &dependency.versions {
            hash = fnv1a(hash, version.as_bytes());
        }
    }
    hash
}

/// The versions the workspace lockfile resolves for each package.
///
/// A package can appear more than once when two dependents pin different
/// versions, so each name maps to a list. The lockfile is read once per load
/// rather than once per resource.
#[derive(Debug, Default)]
pub(crate) struct LockfileVersions {
    versions_by_package: BTreeMap<String, Vec<String>>,
}

impl LockfileVersions {
    /// Reads the lockfile at `path`.
    ///
    /// A missing or unparsable lockfile yields an empty map, so every declared
    /// crate reads as "not in the lockfile" - the same answer a crate that was
    /// never there gets. The load itself does not fail over a record.
    pub(crate) fn read(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(document) = text.parse::<toml_edit::DocumentMut>() else {
            return Self::default();
        };
        let mut versions_by_package: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let Some(packages) = document
            .get("package")
            .and_then(|item| item.as_array_of_tables())
        else {
            return Self::default();
        };
        for package in packages {
            let name = package.get("name").and_then(|name| name.as_str());
            let version = package.get("version").and_then(|version| version.as_str());
            if let (Some(name), Some(version)) = (name, version) {
                versions_by_package
                    .entry(name.to_owned())
                    .or_default()
                    .push(version.to_owned());
            }
        }
        for versions in versions_by_package.values_mut() {
            versions.sort();
            versions.dedup();
        }
        Self {
            versions_by_package,
        }
    }

    /// The versions resolved for `name`, empty when the lockfile lists none.
    fn of(&self, name: &str) -> Vec<String> {
        self.versions_by_package
            .get(name)
            .cloned()
            .unwrap_or_default()
    }
}

/// Builds one resource's record from its declaration, reading the files now.
pub(crate) fn measure_layout(
    declaration: &KeptLayoutDeclaration,
    source_directory: &Path,
    lockfile: &LockfileVersions,
) -> ResourceLayoutRecord {
    let sources: Vec<SourceDigest> = declaration
        .sources
        .iter()
        .map(|path| SourceDigest {
            path: path.clone(),
            hash: hash_source_file(source_directory, path),
        })
        .collect();
    let dependencies: Vec<DependencyVersion> = declaration
        .dependencies
        .iter()
        .map(|name| DependencyVersion {
            name: name.clone(),
            versions: lockfile.of(name),
        })
        .collect();
    ResourceLayoutRecord {
        resource: declaration.declaring_type_name.clone(),
        fingerprint: fold_layout(&sources, &dependencies),
        sources,
        dependencies,
    }
}

// =============================================================================
// Reading and writing the record
// =============================================================================

/// Reads a subject's record.
///
/// A missing, unreadable or version-mismatched record reads as empty: the next
/// load then finds nothing to compare against and keeps every value, which is
/// the answer that cannot break a run.
pub(crate) fn read_record(target: &LayoutRecordTarget<'_>) -> LayoutRecord {
    let Ok(text) = std::fs::read_to_string(target.record_path()) else {
        return LayoutRecord::empty();
    };
    let Ok(record) = serde_json::from_str::<LayoutRecord>(&text) else {
        return LayoutRecord::empty();
    };
    if record.version != LAYOUT_RECORD_VERSION {
        return LayoutRecord::empty();
    }
    record
}

/// Writes a subject's record, describing every declared resource it claims now.
///
/// Best effort, like the artifact stamp: a record that cannot be written leaves
/// the next load treating every declared resource as undeclared, which keeps
/// the values instead of dropping them.
pub(crate) fn write_record(world: &World, target: &LayoutRecordTarget<'_>, claimed: &[ResourceId]) {
    let lockfile = LockfileVersions::read(&target.cargo_lock_path());
    let mut resources = Vec::new();
    for id in claimed {
        let Some(declaration) = world.kept_layout_declaration(*id) else {
            continue;
        };
        resources.push(measure_layout(
            &declaration,
            target.source_directory,
            &lockfile,
        ));
    }
    resources.sort_by(|left, right| left.resource.cmp(&right.resource));
    let record = LayoutRecord {
        version: LAYOUT_RECORD_VERSION,
        resources,
    };
    let Ok(text) = serde_json::to_string_pretty(&record) else {
        return;
    };
    let path = target.record_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text);
}

/// Compares each claimed resource's current layout with the record, reporting
/// the ones that changed.
///
/// Only resources that declared a layout are compared. A resource with no
/// declaration, or one the record does not list, is not a change: it keeps
/// today's keep-on-re-register behaviour. A record that lists a resource the
/// current generation no longer registers is ignored for the same reason - the
/// retirement path owns that case.
pub(crate) fn changed_layouts(
    world: &World,
    target: &LayoutRecordTarget<'_>,
    recorded: &LayoutRecord,
    claimed: &[ResourceId],
) -> Vec<LayoutChange> {
    // Nothing declared: the common case, and the one that must not pay for
    // reading the lockfile on every reload.
    let declarations: Vec<(ResourceId, KeptLayoutDeclaration)> = claimed
        .iter()
        .filter_map(|id| {
            world
                .kept_layout_declaration(*id)
                .map(|declaration| (*id, declaration))
        })
        .collect();
    if declarations.is_empty() {
        return Vec::new();
    }
    let lockfile = LockfileVersions::read(&target.cargo_lock_path());
    let mut changes = Vec::new();
    for (id, declaration) in &declarations {
        let current = measure_layout(declaration, target.source_directory, &lockfile);
        let Some(previous) = recorded.layout_of(&current.resource) else {
            continue;
        };
        if previous.fingerprint == current.fingerprint {
            continue;
        }
        let changed_sources = changed_source_paths(previous, &current);
        let changed_dependencies = changed_dependency_names(previous, &current);
        changes.push(LayoutChange {
            resource: current.resource,
            id: *id,
            changed_sources,
            changed_dependencies,
        });
    }
    changes.sort_by(|left, right| left.resource.cmp(&right.resource));
    changes
}

/// The declared sources whose hash differs between two measurements.
fn changed_source_paths(
    previous: &ResourceLayoutRecord,
    current: &ResourceLayoutRecord,
) -> Vec<String> {
    let hash_of = |record: &ResourceLayoutRecord, path: &str| -> Option<u64> {
        record
            .sources
            .iter()
            .find(|source| source.path == path)
            .map(|source| source.hash)
    };
    let mut changed: Vec<String> = current
        .sources
        .iter()
        .filter(|source| hash_of(previous, &source.path) != Some(source.hash))
        .map(|source| source.path.clone())
        .collect();
    changed.extend(
        previous
            .sources
            .iter()
            .filter(|source| hash_of(current, &source.path).is_none())
            .map(|source| source.path.clone()),
    );
    changed.sort();
    changed.dedup();
    changed
}

/// The declared crates whose resolved versions differ between two measurements.
fn changed_dependency_names(
    previous: &ResourceLayoutRecord,
    current: &ResourceLayoutRecord,
) -> Vec<String> {
    let versions_of = |record: &ResourceLayoutRecord, name: &str| -> Vec<String> {
        record
            .dependencies
            .iter()
            .find(|dependency| dependency.name == name)
            .map(|dependency| dependency.versions.clone())
            .unwrap_or_default()
    };
    let mut changed: Vec<String> = current
        .dependencies
        .iter()
        .filter(|dependency| versions_of(previous, &dependency.name) != dependency.versions)
        .map(|dependency| dependency.name.clone())
        .collect();
    changed.extend(
        previous
            .dependencies
            .iter()
            .filter(|dependency| versions_of(current, &dependency.name).is_empty())
            .filter(|dependency| !dependency.versions.is_empty())
            .map(|dependency| dependency.name.clone()),
    );
    changed.sort();
    changed.dedup();
    changed
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A throwaway directory that is removed with the test.
    struct ScratchDirectory {
        path: PathBuf,
    }

    impl ScratchDirectory {
        fn new(label: &str) -> Self {
            let unique = format!(
                "pill_layout_record_{label}_{}",
                std::process::id() as u64 * 1_000_000
                    + std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|elapsed| u64::from(elapsed.subsec_nanos()))
                        .unwrap_or(0)
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("a scratch directory");
            Self { path }
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("a scratch subdirectory");
            }
            std::fs::write(path, contents).expect("a scratch file");
        }
    }

    impl Drop for ScratchDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// A declaration naming one source and no crates.
    fn declaration(sources: &[&str], dependencies: &[&str]) -> KeptLayoutDeclaration {
        KeptLayoutDeclaration {
            declaring_type_name: "fixture::Witness".to_owned(),
            sources: sources.iter().map(|path| (*path).to_owned()).collect(),
            dependencies: dependencies.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    /// Two loads of unchanged files fingerprint the same; an edited file does
    /// not.
    #[test]
    fn a_source_edit_changes_the_fingerprint() {
        let scratch = ScratchDirectory::new("source_edit");
        scratch.write("src/witness.rs", "pub struct Witness;");
        let lockfile = LockfileVersions::default();

        let before = measure_layout(
            &declaration(&["src/witness.rs"], &[]),
            &scratch.path,
            &lockfile,
        );
        let unchanged = measure_layout(
            &declaration(&["src/witness.rs"], &[]),
            &scratch.path,
            &lockfile,
        );
        assert_eq!(before.fingerprint, unchanged.fingerprint);

        scratch.write("src/witness.rs", "pub struct Witness { extra: u32 }");
        let after = measure_layout(
            &declaration(&["src/witness.rs"], &[]),
            &scratch.path,
            &lockfile,
        );
        assert_ne!(before.fingerprint, after.fingerprint);
    }

    /// A declared crate whose version changes changes the fingerprint, and the
    /// order a resource declares its sources in never does.
    #[test]
    fn a_dependency_version_changes_the_fingerprint_but_declaration_order_does_not() {
        let scratch = ScratchDirectory::new("dependency_version");
        scratch.write("src/first.rs", "first");
        scratch.write("src/second.rs", "second");
        scratch.write(
            "Cargo.lock",
            "[[package]]\nname = \"fixture_dependency\"\nversion = \"1.0.0\"\n",
        );
        let lockfile = LockfileVersions::read(&scratch.path.join("Cargo.lock"));

        let forward = measure_layout(
            &declaration(&["src/first.rs", "src/second.rs"], &["fixture_dependency"]),
            &scratch.path,
            &lockfile,
        );
        let reversed = measure_layout(
            &declaration(&["src/second.rs", "src/first.rs"], &["fixture_dependency"]),
            &scratch.path,
            &lockfile,
        );
        assert_eq!(forward.fingerprint, reversed.fingerprint);

        scratch.write(
            "Cargo.lock",
            "[[package]]\nname = \"fixture_dependency\"\nversion = \"1.1.0\"\n",
        );
        let upgraded_lockfile = LockfileVersions::read(&scratch.path.join("Cargo.lock"));
        let upgraded = measure_layout(
            &declaration(&["src/first.rs", "src/second.rs"], &["fixture_dependency"]),
            &scratch.path,
            &upgraded_lockfile,
        );
        assert_ne!(forward.fingerprint, upgraded.fingerprint);
    }

    /// A record written by one load reads back as the same measurements, and a
    /// version from another format reads as no record at all.
    #[test]
    fn a_record_round_trips_through_the_file_and_an_old_version_is_discarded() {
        let scratch = ScratchDirectory::new("record_round_trip");
        scratch.write("module/src/witness.rs", "contents");
        std::fs::create_dir_all(scratch.path.join("module/src")).expect("the crate root");
        let watch_directory = scratch.path.join("module/src");
        let target = LayoutRecordTarget::new(&scratch.path, &watch_directory, "fixture_module");

        let entry = ResourceLayoutRecord {
            resource: "fixture::Witness".to_owned(),
            sources: vec![SourceDigest {
                path: "src/witness.rs".to_owned(),
                hash: hash_source_file(target.source_directory, "src/witness.rs"),
            }],
            dependencies: Vec::new(),
            fingerprint: 7,
        };
        let record = LayoutRecord {
            version: LAYOUT_RECORD_VERSION,
            resources: vec![entry.clone()],
        };
        std::fs::create_dir_all(target.record_path().parent().expect("a record directory"))
            .expect("a record directory");
        std::fs::write(
            target.record_path(),
            serde_json::to_string(&record).expect("a serialized record"),
        )
        .expect("a written record");

        assert_eq!(read_record(&target), record);

        let stale = LayoutRecord {
            version: LAYOUT_RECORD_VERSION + 1,
            resources: vec![entry],
        };
        std::fs::write(
            target.record_path(),
            serde_json::to_string(&stale).expect("a serialized record"),
        )
        .expect("a written record");
        assert_eq!(read_record(&target), LayoutRecord::empty());
    }

    /// The target resolves declared paths against the crate root, which is the
    /// parent of the watched `src` directory.
    #[test]
    fn the_source_directory_is_the_crate_root_of_the_watched_src_directory() {
        let workspace_root = Path::new("modules");
        let watch_directory = Path::new("../examples/local/project_rs/src");
        let target = LayoutRecordTarget::new(workspace_root, watch_directory, "project_rs");

        assert_eq!(
            target.source_directory,
            Path::new("../examples/local/project_rs")
        );
        assert_eq!(
            target.record_path(),
            Path::new("modules/pill_standalone_temp/layout_records/project_rs.json")
        );
    }
}
