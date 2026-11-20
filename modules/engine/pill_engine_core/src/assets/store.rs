//! Where an asset path's bytes come from: mounted packs and the filesystem.
//!
//! # Responsibilities
//!
//! - Parse an asset pack ([`AssetPack`]). A pack holds the files of a
//!   project's `res` directory, plus the module resources a shipped build
//!   adds. `pill_assets` writes it at build time.
//! - Keep the places an [`AssetLoader::Path`](crate::AssetLoader::Path) looks
//!   in: the packs ([`mount_pack`]), the project's asset directory
//!   ([`mount_directory`]) and the read-only directories extension modules
//!   declare ([`declare_module_root`]). A module resource is found by the same
//!   relative path as a project asset, and it is never written
//!   ([`is_module_asset`]).
//! - Read a path's bytes from the first place that has it ([`read`]).
//! - Write a file beside an asset on the filesystem ([`write_beside`]), which
//!   is how a development build records an asset's metadata, or write a file
//!   that must not exist yet ([`write_new`]). A pack is never written.
//!
//! # Design
//!
//! Loading code names a path relative to the project's `res` directory and
//! never the target it runs on. Only what is mounted differs per target:
//!
//! - **development:** the filesystem - the project's asset directory, every
//!   module root, then the working directory, its `res`, `$PROJECT_PATH/res`
//!   and the executable's `res`;
//! - **native shipping:** the pack the shipping bundle embeds, mounted by the
//!   runtime before any module initializes;
//! - **web:** a pack the frontend fetched, mounted the same way.
//!
//! Packs are searched before the filesystem, newest mount first. A shipped
//! game therefore reads the assets it was built with, not whatever file
//! happens to sit at a development path on the machine. The store's state is
//! a static in this crate, and `pill_engine_core` is a shared `dylib` (one
//! copy per process), so the host, every module and every project all resolve
//! against one set of mounts.
//!
//! # Pack format (version 1)
//!
//! Every number is little-endian. The pack starts with `PILLPACK`, a `u32`
//! format version and a `u32` entry count. Each entry then holds a `u32` path
//! length, the path itself (UTF-8, `/`-separated and relative), a `u64` offset
//! and a `u64` length. Both count from the start of the pack. The file bytes
//! follow the index. `pill_assets::write_asset_pack_with_roots` writes this
//! format, so the two change together.

// Standard library
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

// External crates
// The pack's magic and version come from the crate its writer also compiles
// against, so the two sides cannot drift; see `pill_asset_format`.
pub use pill_asset_format::{ASSET_PACK_MAGIC, ASSET_PACK_VERSION};

// Current crate
use crate::asset::{AssetLoadError, AssetLoadResult};

// =============================================================================
// AssetPack
// =============================================================================

/// A parsed asset pack: the pack's bytes, and where each file sits in them.
///
/// A file is looked up by its `/`-separated path relative to its asset root.
#[derive(Debug)]
pub struct AssetPack {
    /// The whole pack, borrowed when embedded in the binary, owned when fetched.
    bytes: Cow<'static, [u8]>,
    /// Each file's byte range within `bytes`, by `/`-separated relative path.
    entries: HashMap<String, (usize, usize)>,
}

// =============================================================================
// AssetPackError
// =============================================================================

/// Why bytes could not be read as an asset pack.
#[derive(Debug, thiserror::Error)]
pub enum AssetPackError {
    /// The bytes do not start with [`ASSET_PACK_MAGIC`].
    #[error("not an asset pack: the header is missing")]
    NotAPack,
    /// The pack was written in a format version this engine does not read.
    #[error("asset pack version {found} is not supported (expected {ASSET_PACK_VERSION})")]
    UnsupportedVersion {
        /// The version the pack declares.
        found: u32,
    },
    /// The index or a file's bytes run past the end of the pack.
    #[error("asset pack is truncated")]
    Truncated,
    /// An entry's path is not UTF-8.
    #[error("asset pack entry {index} has a path that is not UTF-8")]
    InvalidPath {
        /// Position of the entry in the index.
        index: u32,
    },
}

impl AssetPack {
    /// Parse a pack, validating its index against its length.
    ///
    /// Takes `&'static [u8]` for a pack embedded with `include_bytes!` without
    /// copying it, or `Vec<u8>` for one fetched at runtime.
    ///
    /// # Errors
    ///
    /// Returns an [`AssetPackError`] when the bytes are not a pack of this
    /// version, or its index does not fit in it.
    pub fn parse(bytes: impl Into<Cow<'static, [u8]>>) -> Result<Self, AssetPackError> {
        // Step 1: Read the header: the magic, the format version and the
        // entry count.
        let bytes = bytes.into();
        let mut reader = PackReader {
            bytes: &bytes,
            position: 0,
        };
        if reader.take(ASSET_PACK_MAGIC.len())? != ASSET_PACK_MAGIC {
            return Err(AssetPackError::NotAPack);
        }
        let version = reader.u32()?;
        if version != ASSET_PACK_VERSION {
            return Err(AssetPackError::UnsupportedVersion { found: version });
        }
        let count = reader.u32()?;

        // Step 2: Size the map from the bytes that are left, not from the
        // count. The count comes from the header and is not to be trusted:
        // sizing the map from it alone asks the allocator for hundreds of
        // gigabytes before the first entry is read. An entry cannot be shorter
        // than its path length, an empty path and the two `u64`s, so the bytes
        // that are left bound how many there can really be. The loop below
        // still refuses a count that overstates that bound.
        let remaining = bytes.len() - reader.position;
        let mut entries = HashMap::with_capacity((count as usize).min(remaining / MIN_ENTRY_LEN));
        // Step 3: Read each entry and check its byte range here, once, so a
        // lookup can slice without checking again.
        for index in 0..count {
            let path_length = reader.u32()? as usize;
            let path = std::str::from_utf8(reader.take(path_length)?)
                .map_err(|_| AssetPackError::InvalidPath { index })?
                .to_owned();
            let offset = usize::try_from(reader.u64()?).map_err(|_| AssetPackError::Truncated)?;
            let length = usize::try_from(reader.u64()?).map_err(|_| AssetPackError::Truncated)?;
            let end = offset
                .checked_add(length)
                .ok_or(AssetPackError::Truncated)?;
            if end > bytes.len() {
                return Err(AssetPackError::Truncated);
            }
            entries.insert(path, (offset, length));
        }
        Ok(Self { bytes, entries })
    }

    /// The bytes of the file at `path` (`/`-separated, relative), if packed.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        let &(offset, length) = self.entries.get(path)?;
        Some(&self.bytes[offset..offset + length])
    }

    /// The relative path of every file in the pack, in no particular order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Number of files in the pack.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pack holds no files.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The fewest bytes one index entry can take. It holds a `u32` path length,
/// an empty path, and the `u64` offset and `u64` length.
const MIN_ENTRY_LEN: usize = 4 + 8 + 8;

// =============================================================================
// PackReader
// =============================================================================

/// A cursor over a pack's index.
struct PackReader<'bytes> {
    bytes: &'bytes [u8],
    position: usize,
}

impl<'bytes> PackReader<'bytes> {
    /// The next `length` bytes.
    ///
    /// # Errors
    ///
    /// [`AssetPackError::Truncated`] when fewer than `length` bytes are left.
    fn take(&mut self, length: usize) -> Result<&'bytes [u8], AssetPackError> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(AssetPackError::Truncated)?;
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    /// The next little-endian `u32`.
    ///
    /// # Errors
    ///
    /// [`AssetPackError::Truncated`] when fewer than four bytes are left.
    fn u32(&mut self) -> Result<u32, AssetPackError> {
        let mut value = [0; 4];
        value.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(value))
    }

    /// The next little-endian `u64`.
    ///
    /// # Errors
    ///
    /// [`AssetPackError::Truncated`] when fewer than eight bytes are left.
    fn u64(&mut self) -> Result<u64, AssetPackError> {
        let mut value = [0; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(value))
    }
}

// =============================================================================
// Mount points
// =============================================================================

/// Everything the store can read from: the packs in search order, the
/// project's asset directory and the directories extension modules declared.
#[derive(Default)]
struct Mounts {
    packs: Vec<Arc<AssetPack>>,
    project_directory: Option<PathBuf>,
    module_roots: Vec<ModuleAssetRoot>,
}

/// The process-wide mounts, created on first use.
fn mounts() -> &'static RwLock<Mounts> {
    static MOUNTS: OnceLock<RwLock<Mounts>> = OnceLock::new();
    MOUNTS.get_or_init(Default::default)
}

/// Mounts `pack`. Its files are found before the files of any pack mounted
/// earlier.
pub fn mount_pack(pack: AssetPack) {
    let mut mounts = mounts()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    mounts.packs.insert(0, Arc::new(pack));
}

/// Mount `directory` as the project's asset directory, replacing the previous
/// one. Searched after every pack.
pub fn mount_directory(directory: impl Into<PathBuf>) {
    let mut mounts = mounts()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    mounts.project_directory = Some(directory.into());
}

/// The project's asset directory, when one is mounted.
pub fn mounted_directory() -> Option<PathBuf> {
    let mounts = mounts()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    mounts
        .project_directory
        .clone()
        .filter(|directory| !directory.as_os_str().is_empty())
}

// =============================================================================
// Module asset roots
// =============================================================================

/// A read-only directory an extension module contributes to the store.
///
/// The module declares the directory and the files under it that it cannot
/// run without. The declaration panics when either is missing, so a build that
/// loses a default resource stops with the module and the path in the message
/// instead of loading a blank texture three systems later. Nothing writes into
/// a root, because a module's resources ship with the module; the editor shows
/// the root read-only.
///
/// One module may contribute several roots: its `res`, the sources its
/// configuration is read from, the images it ships. The editor shows all of
/// them under one tree named after the module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleAssetRoot {
    /// The module that declared the root, as the host names it.
    pub module: String,
    /// The directory the module's resources live in.
    pub directory: PathBuf,
    /// The files below `directory` the module cannot run without.
    pub files: Vec<String>,
    /// File extensions, lowercase and without the dot, that the listing leaves
    /// out. A module's cooked output sits beside the sources it was built from
    /// and is a build product, not content, so the editor's tree hides it. The
    /// store still answers those files by name.
    pub hidden: Vec<String>,
}

/// Declare `directory` as one of `module`'s read-only asset roots.
///
/// Loading code names a module resource by a path relative to one of these
/// roots, exactly as it names a project asset relative to `res`. A module
/// registers once per generation, so a reload declares the same root again;
/// that changes nothing.
///
/// # Panics
///
/// Panics when `directory` is not a directory, when one of `required_files`
/// is not a file below it, or when the same directory is declared twice with
/// a different file or hidden-extension list: each is a packaging mistake,
/// and each names the module and the path.
pub fn declare_module_root(module: &str, directory: impl Into<PathBuf>, required_files: &[&str]) {
    declare_module_root_hiding(module, directory, required_files, &[]);
}

/// [`declare_module_root`], with file extensions the listing leaves out.
///
/// A directory whose sources are cooked into a file beside them passes the
/// cooked file's extension here. The store still reads the cooked file by path
/// (a shader's stages, for example), and the editor's tree shows only the
/// sources a person edits.
///
/// # Panics
///
/// Panics on the same mistakes as [`declare_module_root`]: a missing
/// directory or file, or the same directory declared twice with a different
/// file or hidden-extension list.
pub fn declare_module_root_hiding(
    module: &str,
    directory: impl Into<PathBuf>,
    required_files: &[&str],
    hidden_extensions: &[&str],
) {
    let root = ModuleAssetRoot {
        module: module.to_owned(),
        directory: directory.into(),
        files: required_files
            .iter()
            .map(|file| (*file).to_owned())
            .collect(),
        hidden: hidden_extensions
            .iter()
            .map(|extension| extension.to_ascii_lowercase())
            .collect(),
    };
    assert_module_root_complete(&root);
    let mut mounts = mounts()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(declared) = mounts
        .module_roots
        .iter_mut()
        .find(|declared| declared.module == root.module && declared.directory == root.directory)
    {
        assert!(
            *declared == root,
            "module '{}' declared its root '{}' twice, and the two disagree about its files",
            root.module,
            declared.directory.display()
        );
        return;
    }
    mounts.module_roots.push(root);
}

/// Assert every declared module root still holds its resources.
///
/// A module declares its roots while its init runs, and the host runs init
/// inside `catch_unwind`. This re-checks the roots outside that, so a file
/// that went missing stops the process with the module and the path in the
/// message. It also refuses two roots of one module that hold a file at the
/// same relative path, which the editor would show twice under two keys.
///
/// # Panics
///
/// Panics when a declared directory or required file is missing, or when two
/// of a module's roots claim one path.
pub fn verify_module_roots() {
    // Step 1: Refuse two roots of one module that hold the same path.
    let roots = module_roots();
    assert_roots_do_not_overlap(&roots);
    // Step 2: Check that every root still holds its directory and its files.
    for root in &roots {
        assert_module_root_complete(root);
    }
}

/// Panic when two roots of one module hold a file at the same relative path.
fn assert_roots_do_not_overlap(roots: &[ModuleAssetRoot]) {
    let mut claimed: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for root in roots {
        let paths = claimed.entry(root.module.as_str()).or_default();
        for file in relative_files(&root.directory) {
            assert!(
                paths.insert(file.clone()),
                "module '{}' claims '{file}' from two of its roots",
                root.module
            );
        }
    }
}

/// Every file below `directory`, relative and `/`-separated.
fn relative_files(directory: &Path) -> Vec<String> {
    fn walk(directory: &Path, prefix: &Path, files: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = prefix.join(entry.file_name());
            if path.is_dir() {
                walk(&path, &relative, files);
            } else {
                files.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut files = Vec::new();
    walk(directory, Path::new(""), &mut files);
    files
}

/// Panic unless `root`'s directory and files are all there.
fn assert_module_root_complete(root: &ModuleAssetRoot) {
    assert!(
        root.directory.is_dir(),
        "module '{}' declares its asset root at '{}', which is not a directory",
        root.module,
        root.directory.display()
    );
    for file in &root.files {
        let path = root.directory.join(file);
        assert!(
            path.is_file(),
            "module '{}' requires its resource '{file}', which is missing at '{}'",
            root.module,
            path.display()
        );
    }
}

/// The roots modules have declared, in declaration order.
pub fn module_roots() -> Vec<ModuleAssetRoot> {
    mounts()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .module_roots
        .clone()
}

/// Forget every root `module` declared, so its resources stop resolving and
/// stop being listed.
pub fn forget_module_root(module: &str) {
    mounts()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .module_roots
        .retain(|root| root.module != module);
}

/// Forget every declared root.
///
/// The store's tests call it so one test does not decide the others. A host
/// that replaces its whole module set can call it before it mounts the new
/// one, so a root from the previous set cannot answer a path.
pub fn forget_all_module_roots() {
    mounts()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .module_roots
        .clear();
}

/// Whether `path` is one of a module's resources.
///
/// An absolute path counts when it lies inside a declared root. A relative
/// path counts when a root holds it and the project's asset directory does
/// not. The project's directory is checked first, exactly as [`resolve_file`]
/// orders them, so a project file that shares a module resource's path is the
/// project's and stays writable. The editor asks this before it writes
/// anything, which keeps module resources out of the authoring tools.
pub fn is_module_asset(path: &Path) -> bool {
    if path.is_absolute() {
        return module_roots()
            .iter()
            .any(|root| path.starts_with(&root.directory));
    }
    if mounted_directory().is_some_and(|directory| directory.join(path).is_file()) {
        return false;
    }
    module_roots()
        .iter()
        .any(|root| root.directory.join(path).is_file())
}

// =============================================================================
// Reading
// =============================================================================

/// Read the bytes of the asset at `path` from the first place that has it.
///
/// An absolute path is read from the filesystem as it is. A relative path is
/// looked up in every pack, then below the project's asset directory, then in
/// each module root, and last in the development fallbacks: the working
/// directory itself, its `res`, `$PROJECT_PATH/res` and the executable's
/// `res`.
///
/// Once [`restrict_to_packs`] has been called, only the packs, a mounted
/// directory and the module roots are read. A shipped build calls it right
/// after it mounts the pack it was built with, so a pack miss is a missing
/// asset rather than a file the machine happens to have.
///
/// # Errors
///
/// [`AssetLoadError::PathNotFound`] when no mount has the path, or
/// [`AssetLoadError::Read`] when the file was found but could not be read.
pub fn read(path: &Path) -> AssetLoadResult<Vec<u8>> {
    // Step 1: Look in the packs first. A shipped game reads what it was built
    // with.
    if let Some(key) = pack_key(path) {
        let packs = mounts()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .packs
            .clone();
        if let Some(bytes) = packs.iter().find_map(|pack| pack.get(&key)) {
            return Ok(bytes.to_vec());
        }
    }
    // Step 2: Fall back to the filesystem in the order `resolve_file` lists.
    let file = resolve_file(path).ok_or_else(|| AssetLoadError::PathNotFound {
        path: path.to_owned(),
    })?;
    std::fs::read(&file).map_err(|source| AssetLoadError::Read { path: file, source })
}

/// The key `path` is packed under: its components joined with `/`.
///
/// Returns `None` for a path no pack can hold: an empty path, an absolute
/// path, one that climbs out with `..`, or one that is not UTF-8.
///
/// The key is also the name an asset loaded from this path gets, so the two
/// can never disagree. A `\` separates components on every target, not only
/// on Windows, so a path written on one machine names the same asset on
/// another.
pub(crate) fn pack_key(path: &Path) -> Option<String> {
    let unified = path.to_str()?.replace('\\', "/");
    let mut parts = Vec::new();
    for component in Path::new(&unified).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Find `path` on the filesystem. An absolute path is used as it is. Otherwise
/// the search runs below the mounted directory, then each module root, then
/// the development fallbacks.
///
/// The fallbacks are off once [`restrict_to_packs`] has been called, because a
/// shipped build reads what it was built with. An absolute path, a mounted
/// directory and the module roots keep working.
fn resolve_file(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return path.is_file().then(|| path.to_owned());
    }
    // Step 1: Collect the candidate paths in lookup order.
    let mut candidates = Vec::new();
    if let Some(directory) = mounted_directory() {
        candidates.push(directory.join(path));
    }
    // The module roots come next, in declaration order. A default that an
    // extension ships lives beside the module, not in the project.
    for root in module_roots() {
        candidates.push(root.directory.join(path));
    }
    if !packs_only() {
        candidates.push(path.to_owned());
        candidates.push(Path::new("res").join(path));
        if let Some(project_path) = std::env::var_os("PROJECT_PATH") {
            candidates.push(PathBuf::from(project_path).join("res").join(path));
        }
        if let Some(parent) = std::env::current_exe()
            .ok()
            .and_then(|executable| executable.parent().map(Path::to_owned))
        {
            candidates.push(parent.join("res").join(path));
        }
    }
    // Step 2: Return the first candidate that is a file.
    candidates.into_iter().find(|candidate| candidate.is_file())
}

// =============================================================================
// Pack-only store
// =============================================================================

/// Whether the store reads only what its packs hold.
///
/// It is off until [`restrict_to_packs`] turns it on. A development host runs
/// a project straight out of its source tree, and the fallbacks are how it
/// finds the files.
static PACKS_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Read only what the mounted packs hold, never a development fallback.
///
/// A shipped build calls this where it mounts the pack it was built with.
/// From then on, a pack miss is a missing asset, not a file the machine it
/// runs on happens to have at the same name.
pub fn restrict_to_packs() {
    PACKS_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether [`restrict_to_packs`] has been called.
fn packs_only() -> bool {
    PACKS_ONLY.load(std::sync::atomic::Ordering::Relaxed)
}

/// Turn the pack-only posture back off, so one test does not decide it for the
/// others.
#[cfg(test)]
fn set_packs_only(packs_only: bool) {
    PACKS_ONLY.store(packs_only, std::sync::atomic::Ordering::Relaxed);
}

/// The filesystem path the asset at `path` resolves to, or `None` when the
/// asset is packed only or missing.
///
/// Resolves exactly as [`read`] does after the packs: as is when absolute,
/// else below the mounted directory, the module roots and the development
/// fallbacks.
pub fn locate_file(path: &Path) -> Option<PathBuf> {
    resolve_file(path)
}

// =============================================================================
// Writing
// =============================================================================

/// How a file being placed treats a destination that already exists.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Existing {
    /// Someone else's file is as good as this one: leave it in place.
    Keep,
    /// A create must never replace one: report the collision.
    Refuse,
}

/// The link operation [`place_file`] tries first, so a test can stand in a
/// filesystem that has no hard links.
type Link = fn(&Path, &Path) -> std::io::Result<()>;

/// Write `bytes` to a file named like `target`, in the directory of the asset
/// `source`, unless that file already exists.
///
/// Returns the written file, or the one that was already there. Returns
/// `None` when `source` is not on the filesystem: a packed asset (every asset
/// of a shipping or web build) has no directory to write into, and that is
/// not an error.
///
/// The bytes go to a temporary sibling first and are linked into place only
/// when they are complete, so a crash or a second writer never leaves a
/// half-written file. An existing file is never replaced, even one that
/// appears while this runs, because it may hold a person's edits.
///
/// # Errors
///
/// Returns [`AssetLoadError::Write`] when the file cannot be written.
pub fn write_beside(
    source: &Path,
    target: &Path,
    bytes: &[u8],
) -> AssetLoadResult<Option<PathBuf>> {
    // Step 1: Find the source on the filesystem. A packed source has no
    // directory to write into.
    let Some(source_file) = resolve_file(source) else {
        return Ok(None);
    };
    let directory = source_file.parent().unwrap_or(Path::new("."));
    let file_name = target.file_name().ok_or_else(|| AssetLoadError::Write {
        path: target.to_owned(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"),
    })?;
    let destination = directory.join(file_name);
    // Step 2: Leave an existing file alone. It may hold a person's edits.
    if destination.exists() {
        return Ok(Some(destination));
    }
    // Step 3: Write through a temporary sibling and link it into place.
    write_then_place(&destination, bytes, |temporary| {
        place_file(temporary, &destination, Existing::Keep, hard_link)
    })?;
    Ok(Some(destination))
}

/// Write `bytes` as the new file `destination`, refusing when it exists.
///
/// The bytes go to a temporary sibling first and are linked into place only
/// when complete, so a crash never leaves a half-written file, and an existing
/// file - even one that appears while this runs - is never replaced. Missing
/// parent directories are not created: the caller names a folder that exists.
///
/// # Errors
///
/// [`AssetLoadError::Write`] with [`std::io::ErrorKind::AlreadyExists`] when
/// the file exists, or with the underlying error when it cannot be written.
pub fn write_new(destination: &Path, bytes: &[u8]) -> AssetLoadResult<()> {
    // Step 1: Refuse a destination with no file name, and one that exists.
    let write_error = |source| AssetLoadError::Write {
        path: destination.to_owned(),
        source,
    };
    if destination.file_name().is_none() {
        return Err(write_error(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "no file name",
        )));
    }
    if destination.exists() {
        return Err(write_error(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "the file already exists",
        )));
    }
    // Step 2: Place the new file. The hard link fails when the destination
    // exists, which is the guarantee wanted here: unlike `write_beside`, an
    // existing file is an error and not someone else's equally good copy.
    write_then_place(destination, bytes, |temporary| {
        place_file(temporary, destination, Existing::Refuse, hard_link)
    })
}

/// Write `bytes` as `destination`, replacing a file that is already there.
///
/// The overwrite counterpart of [`write_new`]. The bytes go to a temporary
/// sibling first and are renamed over the destination only when they are
/// complete, so a crash never leaves a half-written asset and a reader never
/// sees one.
///
/// # Errors
///
/// [`AssetLoadError::Write`] with the underlying error when the file cannot be
/// written or moved into place.
pub fn write_replacing(destination: &Path, bytes: &[u8]) -> AssetLoadResult<()> {
    write_then_place(destination, bytes, |temporary| {
        std::fs::rename(temporary, destination)
    })
}

/// Write `bytes` to a temporary sibling of `destination`, let `place` move it
/// into place, and clean the sibling up whatever happened.
///
/// The three writers differ only in how they treat a destination that already
/// exists. The temporary-file half (choosing the sibling name, writing it
/// whole, removing it) lives here once, so no writer leaves a half-written
/// file behind.
///
/// # Errors
///
/// [`AssetLoadError::Write`] when the temporary file cannot be written, or
/// with the error `place` reports when it cannot move the file into place.
fn write_then_place(
    destination: &Path,
    bytes: &[u8],
    place: impl FnOnce(&Path) -> std::io::Result<()>,
) -> AssetLoadResult<()> {
    let write_error = |source| AssetLoadError::Write {
        path: destination.to_owned(),
        source,
    };
    let file_name = destination.file_name().ok_or_else(|| {
        write_error(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "no file name",
        ))
    })?;
    // Step 1: Write the whole file under a temporary sibling name.
    let directory = destination.parent().unwrap_or(Path::new("."));
    let temporary = directory.join(temporary_name(file_name));
    std::fs::write(&temporary, bytes).map_err(write_error)?;
    // Step 2: Move the sibling into place, then remove it. A successful
    // fallback rename consumed the temporary, so a failed removal says nothing
    // about the write.
    let placed = place(&temporary);
    let _ = std::fs::remove_file(&temporary);
    placed.map_err(write_error)
}

/// A sibling name for `file_name` that no other writer in this or another
/// process picks at the same time.
fn temporary_name(file_name: &std::ffi::OsStr) -> std::ffi::OsString {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = std::ffi::OsString::from(".");
    name.push(file_name);
    name.push(format!(".{}-{sequence}.tmp", std::process::id()));
    name
}

/// Creates a hard link, as the [`Link`] a real write passes to [`place_file`].
fn hard_link(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::hard_link(temporary, destination)
}

/// Move the complete file `temporary` to `destination`, treating a
/// destination that already exists as `existing` says.
///
/// The hard link is tried first, because it fails when the destination exists
/// and no rename does. A filesystem without hard links (exFAT, FAT, some
/// network shares) falls back to a rename. There the existence check below
/// stands in for the link's refusal. The check and the rename are not one
/// operation, but only two writers racing on one file could slip between them.
///
/// # Errors
///
/// [`std::io::ErrorKind::AlreadyExists`] when the destination exists and
/// `existing` is [`Existing::Refuse`], or the error the link or the rename
/// reports.
fn place_file(
    temporary: &Path,
    destination: &Path,
    existing: Existing,
    link: Link,
) -> std::io::Result<()> {
    match link(temporary, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => match existing {
            Existing::Keep => Ok(()),
            Existing::Refuse => Err(error),
        },
        Err(_) if destination.exists() => match existing {
            Existing::Keep => Ok(()),
            Existing::Refuse => Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "the file already exists",
            )),
        },
        Err(_) => std::fs::rename(temporary, destination),
    }
}

// =============================================================================
// Test locks
// =============================================================================

/// Serializes tests that change the process-wide mounted directory.
///
/// `mount_directory` replaces one global, and tests run in parallel threads,
/// so two tests mounting their own scratch directory would read each other's
/// files. Every test that mounts a directory holds this for its whole run.
#[cfg(test)]
pub(crate) fn mounted_directory_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A test that failed while holding the lock poisons it. The next holder
    // re-mounts the directory anyway, so the poison does not matter.
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Serializes tests that declare module roots. The registry is process-wide.
#[cfg(test)]
pub(crate) fn module_roots_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Drop every declared module root, so one test does not decide the others.
#[cfg(test)]
pub(crate) fn clear_module_roots() {
    forget_all_module_roots();
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh scratch directory named after `label`.
    fn scratch(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("pill-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// A module root answers a module resource by name, and the store calls
    /// that path read-only.
    #[test]
    fn a_module_root_answers_a_relative_path() {
        let _guard = module_roots_test_lock();
        clear_module_roots();
        let directory = scratch("module-root");
        std::fs::create_dir_all(directory.join("overlay")).unwrap();
        std::fs::write(directory.join("overlay/grid.wgsl"), b"grid").unwrap();

        declare_module_root("pill_test_module", &directory, &["overlay/grid.wgsl"]);

        assert_eq!(read(Path::new("overlay/grid.wgsl")).unwrap(), b"grid");
        assert!(is_module_asset(Path::new("overlay/grid.wgsl")));
        assert!(is_module_asset(&directory.join("overlay/grid.wgsl")));
        assert!(!is_module_asset(Path::new("overlay/missing.wgsl")));
        let roots = module_roots();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].module, "pill_test_module");
        assert_eq!(roots[0].files, vec!["overlay/grid.wgsl"]);

        clear_module_roots();
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// A declaration names the module and the file it cannot find.
    #[test]
    fn a_missing_resource_stops_the_declaration() {
        let _guard = module_roots_test_lock();
        clear_module_roots();
        let directory = scratch("module-root-missing-file");

        let panic = std::panic::catch_unwind(|| {
            declare_module_root("pill_test_module", &directory, &["overlay/grid.wgsl"]);
        });
        let message = panic_message(panic);
        assert!(message.contains("pill_test_module"), "{message}");
        assert!(message.contains("overlay/grid.wgsl"), "{message}");
        assert!(
            module_roots().is_empty(),
            "the refused root is not recorded"
        );

        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// A directory that is not there is named too.
    #[test]
    fn a_missing_directory_stops_the_declaration() {
        let _guard = module_roots_test_lock();
        clear_module_roots();
        let directory = scratch("module-root-missing-directory").join("gone");

        let panic =
            std::panic::catch_unwind(|| declare_module_root("pill_test_module", &directory, &[]));
        let message = panic_message(panic);
        assert!(message.contains("is not a directory"), "{message}");
        assert!(message.contains("pill_test_module"), "{message}");
    }

    /// A module registers once per generation, so declaring the same root
    /// twice is a reload. The same directory with a different file list is a
    /// mistake.
    #[test]
    fn declaring_the_same_root_again_changes_nothing() {
        let _guard = module_roots_test_lock();
        clear_module_roots();
        let directory = scratch("module-root-twice");
        std::fs::write(directory.join("grid.wgsl"), b"grid").unwrap();
        declare_module_root("pill_test_module", &directory, &["grid.wgsl"]);
        declare_module_root("pill_test_module", &directory, &["grid.wgsl"]);
        assert_eq!(module_roots().len(), 1, "the reload kept one root");

        let panic = std::panic::catch_unwind(|| {
            declare_module_root("pill_test_module", &directory, &[]);
        });
        let message = panic_message(panic);
        assert!(message.contains("disagree"), "{message}");
        assert_eq!(module_roots().len(), 1);

        clear_module_roots();
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// A module may contribute several directories, and each answers its own
    /// relative paths.
    #[test]
    fn a_module_may_declare_a_second_root() {
        let _guard = module_roots_test_lock();
        clear_module_roots();
        let resources = scratch("module-root-res");
        let sources = scratch("module-root-sources");
        std::fs::write(resources.join("default.material"), b"{{}}").unwrap();
        std::fs::write(sources.join("fragment.hlsl"), b"//").unwrap();

        declare_module_root("pill_test_module", &resources, &["default.material"]);
        declare_module_root("pill_test_module", &sources, &["fragment.hlsl"]);

        assert_eq!(module_roots().len(), 2, "both directories are the module's");
        assert_eq!(read(Path::new("default.material")).unwrap(), b"{{}}");
        assert_eq!(read(Path::new("fragment.hlsl")).unwrap(), b"//");

        clear_module_roots();
        std::fs::remove_dir_all(&resources).unwrap();
        std::fs::remove_dir_all(&sources).unwrap();
    }

    /// Two roots of one module that hold one path would show it twice, so the
    /// verification refuses them by name.
    #[test]
    fn two_roots_of_one_module_may_not_claim_one_path() {
        let _guard = module_roots_test_lock();
        clear_module_roots();
        let first = scratch("module-root-overlap-first");
        let second = scratch("module-root-overlap-second");
        std::fs::write(first.join("shared.wgsl"), b"a").unwrap();
        std::fs::write(second.join("shared.wgsl"), b"b").unwrap();
        declare_module_root("pill_test_module", &first, &[]);
        declare_module_root("pill_test_module", &second, &[]);

        let panic = std::panic::catch_unwind(verify_module_roots);
        let message = panic_message(panic);
        assert!(message.contains("shared.wgsl"), "{message}");
        assert!(message.contains("pill_test_module"), "{message}");

        clear_module_roots();
        std::fs::remove_dir_all(&first).unwrap();
        std::fs::remove_dir_all(&second).unwrap();
    }

    /// A resource deleted after the declaration stops the host's verification.
    #[test]
    fn a_vanished_resource_fails_the_verification() {
        let _guard = module_roots_test_lock();
        clear_module_roots();
        let directory = scratch("module-root-vanished");
        std::fs::write(directory.join("grid.wgsl"), b"grid").unwrap();
        declare_module_root("pill_test_module", &directory, &["grid.wgsl"]);
        std::fs::remove_file(directory.join("grid.wgsl")).unwrap();

        let panic = std::panic::catch_unwind(verify_module_roots);
        let message = panic_message(panic);
        assert!(message.contains("grid.wgsl"), "{message}");

        clear_module_roots();
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// The message a caught panic carried.
    fn panic_message(panic_result: std::thread::Result<()>) -> String {
        let panic_payload = panic_result.expect_err("the call was expected to panic");
        panic_payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                panic_payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
            })
            .unwrap_or_default()
    }

    #[test]
    fn write_new_writes_once_and_never_replaces() {
        let directory = std::env::temp_dir().join(format!("pill-write-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("a.render_pass");

        write_new(&file, b"first").unwrap();
        let error = write_new(&file, b"second").unwrap_err();

        assert!(matches!(
            error,
            AssetLoadError::Write { ref source, .. } if source.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(std::fs::read(&file).unwrap(), b"first");
        let leftovers = std::fs::read_dir(&directory).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary file is left behind");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// A filesystem with no hard links (exFAT, FAT, some network shares)
    /// refuses the link every writer tries first. A create must then fall back
    /// to the rename instead of reporting that refusal.
    fn no_hard_links(_: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "this filesystem has no hard links",
        ))
    }

    #[test]
    fn a_create_lands_on_a_filesystem_without_hard_links() {
        let directory =
            std::env::temp_dir().join(format!("pill-no-hard-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("a.render_pass");
        let temporary = directory.join(".a.render_pass.tmp");
        std::fs::write(&temporary, b"first").unwrap();

        place_file(&temporary, &destination, Existing::Refuse, no_hard_links)
            .expect("a filesystem without hard links still takes the new file");

        assert_eq!(std::fs::read(&destination).unwrap(), b"first");
        assert!(!temporary.exists(), "the temporary moved into place");

        // The fallback must not turn a create into a replace. The file that is
        // already there stays, and the second write is refused.
        std::fs::write(&temporary, b"second").unwrap();
        let error = place_file(&temporary, &destination, Existing::Refuse, no_hard_links)
            .expect_err("a create never replaces an existing file");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&destination).unwrap(), b"first");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// A version 1 pack of `files`, written the way `pill_assets` writes one.
    fn pack_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let index_length: usize = files.iter().map(|(path, _)| 4 + path.len() + 16).sum();
        let mut offset = (8 + 4 + 4 + index_length) as u64;
        let mut pack = Vec::new();
        pack.extend_from_slice(ASSET_PACK_MAGIC);
        pack.extend_from_slice(&ASSET_PACK_VERSION.to_le_bytes());
        pack.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for (path, bytes) in files {
            pack.extend_from_slice(&(path.len() as u32).to_le_bytes());
            pack.extend_from_slice(path.as_bytes());
            pack.extend_from_slice(&offset.to_le_bytes());
            pack.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            offset += bytes.len() as u64;
        }
        for (_, bytes) in files {
            pack.extend_from_slice(bytes);
        }
        pack
    }

    #[test]
    fn a_pack_returns_each_file_by_its_relative_path() {
        let pack =
            AssetPack::parse(pack_of(&[("models/a.obj", b"mesh"), ("b.png", b"image")])).unwrap();
        assert_eq!(pack.len(), 2);
        assert_eq!(pack.get("models/a.obj"), Some(&b"mesh"[..]));
        assert_eq!(pack.get("b.png"), Some(&b"image"[..]));
        assert_eq!(pack.get("missing"), None);
    }

    #[test]
    fn a_damaged_pack_is_refused() {
        assert!(matches!(
            AssetPack::parse(b"NOTAPACK".to_vec()),
            Err(AssetPackError::NotAPack)
        ));
        let mut future = pack_of(&[]);
        future[8] = 9;
        assert!(matches!(
            AssetPack::parse(future),
            Err(AssetPackError::UnsupportedVersion { found: 9 })
        ));
        let mut cut = pack_of(&[("a", b"0123456789")]);
        cut.truncate(cut.len() - 1);
        assert!(matches!(
            AssetPack::parse(cut),
            Err(AssetPackError::Truncated)
        ));
        // A damaged header may declare far more entries than the pack can
        // hold. Sizing the index map from that count alone asks the allocator
        // for hundreds of gigabytes before the first entry is read, which
        // aborts the process instead of reporting the truncation.
        let mut huge_count = pack_of(&[]);
        huge_count[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            AssetPack::parse(huge_count),
            Err(AssetPackError::Truncated)
        ));
    }

    #[test]
    fn pack_keys_are_slash_separated_and_stay_inside_the_pack() {
        assert_eq!(
            pack_key(Path::new("models/./a.obj")).as_deref(),
            Some("models/a.obj")
        );
        assert_eq!(pack_key(Path::new("../outside.txt")), None);
        assert_eq!(pack_key(Path::new("")), None);
    }

    /// The pack key doubles as an asset's name, so every spelling of one path
    /// must give one key, on every target.
    #[test]
    fn backslashes_and_dots_normalize_to_one_key() {
        let expected = Some("textures/a.jpg".to_owned());
        assert_eq!(pack_key(Path::new("textures\\a.jpg")), expected);
        assert_eq!(pack_key(Path::new("textures/./a.jpg")), expected);
        assert_eq!(pack_key(Path::new(".\\textures\\.\\a.jpg")), expected);
        assert_eq!(pack_key(Path::new("textures\\..\\a.jpg")), None);
        assert_eq!(pack_key(&std::env::temp_dir().join("a.jpg")), None);
    }

    /// A fresh directory under the system temp directory, removed on drop.
    struct ScratchDirectory(PathBuf);

    impl ScratchDirectory {
        fn new(label: &str) -> Self {
            static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pill-asset-store-{label}-{}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for ScratchDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn locate_file_finds_files_on_disk_only() {
        let scratch = ScratchDirectory::new("locate");
        let source = scratch.0.join("present.txt");
        std::fs::write(&source, b"x").unwrap();
        assert_eq!(locate_file(&source), Some(source.clone()));
        assert_eq!(locate_file(&scratch.0.join("absent.txt")), None);
    }

    /// Checks that the metadata file is written into the source's own
    /// directory, under the target's file name, and that no temporary file is
    /// left behind.
    #[test]
    fn write_beside_writes_next_to_the_source() {
        let scratch = ScratchDirectory::new("write");
        let source = scratch.0.join("a.jpg");
        std::fs::write(&source, b"image").unwrap();

        let written = write_beside(&source, Path::new("textures/a.jpg.meta"), b"{}")
            .unwrap()
            .expect("the source is on disk");

        assert_eq!(written, scratch.0.join("a.jpg.meta"));
        assert_eq!(std::fs::read(&written).unwrap(), b"{}");
        let leftovers: Vec<_> = std::fs::read_dir(&scratch.0)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");
    }

    /// An existing file may hold a person's edits, so it is kept as it is.
    #[test]
    fn write_beside_never_replaces_an_existing_file() {
        let scratch = ScratchDirectory::new("keep");
        let source = scratch.0.join("a.jpg");
        std::fs::write(&source, b"image").unwrap();
        let existing = scratch.0.join("a.jpg.meta");
        std::fs::write(&existing, b"edited by hand").unwrap();

        let written = write_beside(&source, Path::new("a.jpg.meta"), b"generated").unwrap();

        assert_eq!(written, Some(existing.clone()));
        assert_eq!(std::fs::read(&existing).unwrap(), b"edited by hand");
    }

    /// A source that is packed only, as in every shipping and web build, has
    /// no directory to write beside, and that is not an error.
    #[test]
    fn write_beside_a_source_not_on_disk_writes_nothing() {
        let key = "pill-store-test/packed-only-source.jpg";
        mount_pack(AssetPack::parse(pack_of(&[(key, b"image")])).unwrap());
        assert_eq!(read(Path::new(key)).unwrap(), b"image");

        let written = write_beside(
            Path::new(key),
            Path::new("packed-only-source.jpg.meta"),
            b"{}",
        )
        .unwrap();

        assert_eq!(written, None);
    }

    #[test]
    fn a_mounted_pack_answers_before_the_filesystem() {
        mount_pack(
            AssetPack::parse(pack_of(&[("pill-store-test/only-packed.txt", b"packed")])).unwrap(),
        );
        assert_eq!(
            read(Path::new("pill-store-test/only-packed.txt")).unwrap(),
            b"packed"
        );
        assert!(matches!(
            read(Path::new("pill-store-test/nowhere.txt")),
            Err(AssetLoadError::PathNotFound { .. })
        ));
    }

    /// A shipped build reads exactly what it was built with. A file the
    /// machine happens to have at a development fallback - here the working
    /// directory - is not the asset, and a missing key is a missing asset.
    #[test]
    fn a_pack_only_store_never_reads_the_machines_files() {
        let _lock = mounted_directory_test_lock();
        let decoy = PathBuf::from(format!("pill-store-decoy-{}.txt", std::process::id()));
        std::fs::write(&decoy, b"a file the machine happens to have").unwrap();

        // The development posture resolves it, because that is what the
        // fallbacks are for.
        assert_eq!(read(&decoy).unwrap(), b"a file the machine happens to have");

        set_packs_only(true);
        let miss = read(&decoy);
        set_packs_only(false);
        let _ = std::fs::remove_file(&decoy);

        assert!(
            matches!(miss, Err(AssetLoadError::PathNotFound { .. })),
            "a pack-only store must not read the machine's files"
        );
    }
}
