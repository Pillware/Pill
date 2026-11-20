//! The [`Sound`] asset: an audio file kept as its encoded bytes.
//!
//! # Responsibilities
//!
//! - Declares [`Sound`], stored in the
//!   [`AssetManager`](pill_engine::AssetManager) and addressed by
//!   `Handle<Sound>`.
//! - Checks a file's extension and reads its bytes through the asset store
//!   ([`Sound::load`]), so a packed sound loads like one on disk.
//! - Imports a sound through its `.meta` file
//!   ([`ImportedAsset`](pill_engine::ImportedAsset), with
//!   [`SoundImportSettings`]), which gives it a stable guid.
//! - Gives the playback code a new decoder over those bytes.
//!
//! # Design
//!
//! A sound is an asset rather than a resource because a world holds many of
//! them, and the type alone does not say which one is meant - the distinction
//! `pill_engine` draws between [`Resource`](pill_engine::Resource) and
//! [`Asset`](pill_engine::Asset).
//!
//! The bytes stay encoded. A decoder can be used for one playback only, so
//! two entities that play one sound need two decoders. Holding decoded PCM
//! instead would take roughly ten times more memory per sound.

// External crates
use pill_core::utils::AssetPathError;
use pill_engine::{asset, AssetLoadError, AssetLoadResult};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// Audio container formats [`Sound::load`] accepts, lowercase and without the
/// leading dot.
///
/// A path's extension is checked before any bytes are read. An unsupported
/// extension is reported with the extension and this list, and a path with no
/// extension is reported with the path, so neither reaches the decoder.
pub const SUPPORTED_AUDIO_FORMATS: &[&str] = &["mp3", "wav", "ogg", "flac"];

// =============================================================================
// Sound
// =============================================================================

/// One loaded audio file, kept as its encoded bytes.
///
/// Stored in the [`AssetManager`](pill_engine::AssetManager) and addressed by
/// `Handle<Sound>`. A new decoder is built for each playback, so two entities
/// can play the same sound at once.
///
/// # Examples
///
/// ```no_run
/// # use pill_audio::Sound;
/// # fn demo(assets: &mut pill_engine::AssetManager) -> Result<(), pill_audio::SoundLoadError> {
/// let sound = Sound::load(std::path::Path::new("assets/footstep.wav"))?;
/// let handle = assets.add_named("footstep", sound).expect("a fresh name");
/// # Ok(())
/// # }
/// ```
///
/// `Debug` prints the path and the byte count instead of the bytes.
///
/// Mirrored to C# as `pill_audio.Sound`: a managed project imports one with
/// `assets.Import<Sound>(path, ...)` and builds one with `Sound.Load` or
/// `Sound.FromBytes`.
///
/// The attribute declares the type, so the world's asset store re-points its
/// table at the generation still mapped without a call in the module's
/// `register`.
// Pins the asset type's shared name, so the module DLL and every project that
// links this crate use one asset column: a `Handle<Sound>` issued by a project
// has to find the column the module's playback system reads. The `.meta` files
// record the same spelling, because `metadata_type_name` defaults to this
// name, so keeping the two equal also stops a move of the type from orphaning
// them.
#[asset(imported)]
#[pill_engine::pill_mirror_object(asset, import, shared = "pill_audio::Sound")]
pub struct Sound {
    /// Path the bytes were read from, or the label [`Sound::from_bytes`] was
    /// given.
    path: std::path::PathBuf,
    /// The encoded bytes, as they were read or handed over.
    bytes: std::sync::Arc<[u8]>,
}

impl std::fmt::Debug for Sound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Sound")
            .field("path", &self.path)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

#[pill_engine::pill_mirror_impl]
impl Sound {
    /// Read an audio file into memory.
    ///
    /// The path is read through [`asset_store`](pill_engine::asset_store) like
    /// every other asset: a relative path is looked up in a mounted pack first
    /// (a shipping or web build), then below the project's `res` directory.
    /// The extension is checked against [`SUPPORTED_AUDIO_FORMATS`] before any
    /// bytes are read.
    ///
    /// # Errors
    ///
    /// [`SoundLoadError::InvalidPath`] when the extension is not supported or
    /// no mount has the path, and [`SoundLoadError::Unreadable`] when the file
    /// was found but could not be read.
    #[pill_engine::pill_mirror_method]
    pub fn load(path: &std::path::Path) -> Result<Self, SoundLoadError> {
        // Step 1: Reject an unknown extension before any bytes are read.
        let path = path.to_path_buf();
        validate_format(&path).map_err(SoundLoadError::InvalidPath)?;

        // Step 2: Read the bytes through the asset store. A path no mount has
        // is reported as an invalid path; a file that is found but cannot be
        // read keeps its IO error.
        let bytes = pill_engine::asset_store::read(&path).map_err(|error| match error {
            AssetLoadError::Read { path, source } => SoundLoadError::Unreadable { path, source },
            // `read` fails only as not found or unreadable; anything else is
            // reported as a path no mount could serve, too.
            _ => SoundLoadError::InvalidPath(AssetPathError::InvalidPath {
                path: path.display().to_string(),
            }),
        })?;

        Ok(Self {
            path,
            bytes: bytes.into(),
        })
    }

    /// Build a sound from bytes already in memory.
    ///
    /// The path is kept for diagnostics only and is not checked, which is what
    /// an embedded asset (`include_bytes!`) or a cooked one needs.
    #[pill_engine::pill_mirror_method]
    pub fn from_bytes(path: &std::path::Path, bytes: Vec<u8>) -> Self {
        Self {
            path: path.to_path_buf(),
            bytes: bytes.into(),
        }
    }

    /// The path the bytes were read from, or the label [`Sound::from_bytes`]
    /// was given.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The encoded bytes, unchanged since they were read or handed over.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Build a decoder that plays this sound once.
    ///
    /// A decoder reads its bytes straight through, so it is used up by one
    /// playback and a second entity playing the same sound needs its own. It
    /// reuses the sound's bytes instead of copying them, since building one
    /// clones the `Arc` - a reference-count bump that costs the same for a
    /// large file as for a small one.
    ///
    /// Returns `None` when rodio cannot decode the bytes. [`Sound::load`]
    /// cannot rule that out, because it checks only the extension.
    pub fn decoder(&self) -> Option<rodio::Decoder<std::io::Cursor<ArcBytes>>> {
        rodio::Decoder::new(std::io::Cursor::new(ArcBytes(self.bytes.clone()))).ok()
    }
}

impl pill_engine::ImportedAsset for Sound {
    type ImportSettings = SoundImportSettings;
    const SOURCE_EXTENSIONS: &'static [&'static str] = SUPPORTED_AUDIO_FORMATS;

    // No `metadata_type_name` override: the default uses the shared name above,
    // which is the spelling a `.meta` file records.

    fn import(
        name: &str,
        source_bytes: &[u8],
        _settings: &SoundImportSettings,
    ) -> AssetLoadResult<Self> {
        let path = std::path::Path::new(name);
        validate_format(path).map_err(|error| AssetLoadError::Decode {
            label: name.to_owned(),
            detail: error.to_string(),
        })?;
        Ok(Self::from_bytes(path, source_bytes.to_vec()))
    }
}

// =============================================================================
// SoundImportSettings
// =============================================================================

/// How a sound file is read, as stored in its `.meta` file.
///
/// It has no fields, because a sound's bytes say everything about the sound.
/// The type exists so sound files get metadata files, and with them a guid
/// that survives a rename of the file. A field added later must have a
/// default, so files written before it existed still read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundImportSettings {}

// =============================================================================
// SoundLoadError
// =============================================================================

/// Why [`Sound::load`] could not produce a sound.
#[derive(Debug)]
pub enum SoundLoadError {
    /// No mount holds the path, or its extension is not one of
    /// [`SUPPORTED_AUDIO_FORMATS`].
    InvalidPath(pill_core::utils::AssetPathError),
    /// The file was found but could not be read.
    Unreadable {
        /// The path of the file that could not be read.
        path: std::path::PathBuf,
        /// The error the read returned.
        source: std::io::Error,
    },
}

impl std::fmt::Display for SoundLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath(error) => write!(formatter, "{error}"),
            Self::Unreadable { path, source } => {
                write!(formatter, "failed to read `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for SoundLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidPath(error) => Some(error),
            Self::Unreadable { source, .. } => Some(source),
        }
    }
}

// =============================================================================
// ArcBytes
// =============================================================================

/// A byte buffer shared through an `Arc`, which a decoder can read as a slice.
///
/// This is what lets a decoder read the sound's existing bytes instead of
/// copying them. Creating one clones the `Arc` (a reference-count bump, the
/// same cost for a large file and a small one), and the cursor then reads
/// through the shared buffer.
pub struct ArcBytes(std::sync::Arc<[u8]>);

impl AsRef<[u8]> for ArcBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Check that the extension of `path` is one of [`SUPPORTED_AUDIO_FORMATS`].
///
/// Only the extension is checked, never whether the file exists, because a
/// packed sound has no file on disk. Reading the path answers that question.
pub(crate) fn validate_format(path: &std::path::Path) -> Result<(), AssetPathError> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if SUPPORTED_AUDIO_FORMATS.contains(&extension) => Ok(()),
        Some(extension) => Err(AssetPathError::InvalidFormat {
            extension: extension.to_string(),
            allowed: SUPPORTED_AUDIO_FORMATS.join(", "),
        }),
        None => Err(AssetPathError::InvalidPath {
            path: path.display().to_string(),
        }),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::asset_store::AssetPack;
    use pill_engine::{AssetImport, AssetManager, ImportedAsset, MetadataPolicy, MetadataSource};

    /// A missing file is reported as an invalid path, not as a read failure.
    #[test]
    fn loading_a_missing_file_fails_with_the_path() {
        let error = Sound::load(std::path::Path::new("definitely/not/here.wav")).unwrap_err();
        assert!(matches!(error, SoundLoadError::InvalidPath(_)));
    }

    /// An unsupported extension is rejected as a format problem, not later as
    /// a decode failure, even for a file that exists.
    #[test]
    fn loading_an_unsupported_format_is_rejected() {
        let crate_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifest_path = crate_directory.join("Cargo.toml");
        assert!(manifest_path.exists(), "the crate manifest should exist");

        match Sound::load(&manifest_path).unwrap_err() {
            SoundLoadError::InvalidPath(pill_core::utils::AssetPathError::InvalidFormat {
                extension,
                ..
            }) => assert_eq!(extension, "toml"),
            other => panic!("expected a format rejection, got {other:?}"),
        }
    }

    /// A file given as an absolute path is read from the filesystem.
    #[test]
    fn a_sound_on_disk_loads() {
        let directory =
            std::env::temp_dir().join(format!("pill_audio_disk_{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("click.wav");
        std::fs::write(&file, [1, 2, 3, 4]).unwrap();

        let sound = Sound::load(&file).expect("the file exists");
        assert_eq!(sound.bytes(), &[1, 2, 3, 4]);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// A sound that exists only in a mounted pack loads. That is how a
    /// shipping or web build reads assets: no file is on disk at that path.
    #[test]
    fn a_sound_found_only_in_a_mounted_pack_loads() {
        // The path is unique to this test. The pack stays mounted for the rest
        // of the process, so a shared path could hide another test's file.
        let packed_path = "pill_audio_pack_test/only_in_pack.wav";
        let pack =
            AssetPack::parse(asset_pack_bytes(&[(packed_path, &[9, 8, 7])])).expect("a valid pack");
        pill_engine::asset_store::mount_pack(pack);

        let path = std::path::Path::new(packed_path);
        assert!(!path.exists() && pill_engine::asset_store::locate_file(path).is_none());
        let sound = Sound::load(path).expect("the pack has the file");
        assert_eq!(sound.bytes(), &[9, 8, 7]);
        assert_eq!(sound.path(), path);
    }

    /// Build a version 1 asset pack holding `files`, in the layout
    /// [`asset_store`](pill_engine::asset_store) documents.
    fn asset_pack_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        let index_length_bytes: usize = files.iter().map(|(path, _)| 4 + path.len() + 16).sum();
        let mut byte_offset = (8 + 4 + 4 + index_length_bytes) as u64;
        let mut pack = Vec::new();
        pack.extend_from_slice(pill_engine::asset_store::ASSET_PACK_MAGIC);
        pack.extend_from_slice(&pill_engine::asset_store::ASSET_PACK_VERSION.to_le_bytes());
        pack.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for (path, bytes) in files {
            pack.extend_from_slice(&(path.len() as u32).to_le_bytes());
            pack.extend_from_slice(path.as_bytes());
            pack.extend_from_slice(&byte_offset.to_le_bytes());
            pack.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            byte_offset += bytes.len() as u64;
        }
        for (_, bytes) in files {
            pack.extend_from_slice(bytes);
        }
        pack
    }

    /// Importing a sound with `CreateIfMissing` writes its `.meta` file. A
    /// later run reads the same guid from it, and importing again in the same
    /// run (a project reload) returns the handle that is already loaded.
    ///
    /// This is the only test in the crate that mounts a directory, so it needs
    /// no lock against another test replacing the mount.
    #[test]
    fn importing_a_sound_writes_its_metadata_and_keeps_its_guid() {
        let root = std::env::temp_dir().join(format!("pill_audio_import_{}", std::process::id()));
        std::fs::create_dir_all(root.join("sounds")).unwrap();
        std::fs::write(root.join("sounds/click.mp3"), [1, 2, 3]).unwrap();
        pill_engine::asset_store::mount_directory(&root);
        let request =
            || AssetImport::<Sound>::new("sounds/click.mp3", MetadataPolicy::CreateIfMissing);

        let mut first_run = AssetManager::new();
        let created = first_run.import(request()).expect("the file exists");
        assert_eq!(created.metadata, MetadataSource::CreatedOnDisk);
        let metadata = std::fs::read_to_string(root.join("sounds/click.mp3.meta")).unwrap();
        // The sidecar is YAML, so the type is a bare scalar rather than a
        // quoted JSON string.
        assert!(
            metadata.contains("asset_type: pill_audio::Sound"),
            "{metadata}"
        );
        assert_eq!(
            first_run.get(created.handle).map(Sound::bytes),
            Some(&[1, 2, 3][..])
        );

        let reloaded = first_run.import(request()).expect("still loadable");
        assert!(reloaded.already_loaded);
        assert_eq!(reloaded.handle, created.handle);

        let mut second_run = AssetManager::new();
        let read_from_file = second_run.import(request()).expect("the metadata exists");
        assert_eq!(read_from_file.metadata, MetadataSource::ReadFromFile);
        assert_eq!(read_from_file.guid, created.guid);

        // The same file again, reached by its extension through the registry
        // the module fills, with no mention of `Sound`.
        let mut world = pill_engine::World::new();
        world.register_imported_asset::<Sound>();
        let import_registry = world.get_resource::<pill_engine::ImportRegistry>().unwrap();
        let erased_import = import_registry
            .import(
                &mut second_run,
                std::path::Path::new("sounds/click.mp3"),
                MetadataPolicy::ReadIfPresent,
            )
            .expect("`.mp3` maps to Sound");
        assert_eq!(erased_import.type_name, "pill_audio::Sound");
        assert!(erased_import.previously_loaded());
        assert_eq!(erased_import.guid, created.guid);

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// An imported file whose extension is not an audio format is refused as
    /// a decode error naming the file.
    #[test]
    fn importing_an_unsupported_format_is_refused() {
        let error = Sound::import("sounds/notes.txt", &[1], &SoundImportSettings::default())
            .expect_err("a text file is not a sound");
        assert!(
            matches!(error, AssetLoadError::Decode { ref label, .. } if label == "sounds/notes.txt")
        );
    }

    /// Bytes already in memory skip path validation. An embedded or cooked
    /// asset needs exactly that.
    #[test]
    fn sound_can_be_built_from_bytes() {
        let sound = Sound::from_bytes(std::path::Path::new("embedded.wav"), vec![1, 2, 3]);
        assert_eq!(sound.bytes(), &[1, 2, 3]);
        assert_eq!(sound.path(), std::path::Path::new("embedded.wav"));
    }

    /// A sound is an asset, so a manager holds many of them and each is
    /// reached by handle, with its name as a second way to look it up.
    #[test]
    fn sounds_are_stored_as_assets() {
        let mut assets = AssetManager::new();
        let handle = assets
            .add_named(
                "footstep",
                Sound::from_bytes(std::path::Path::new("a.wav"), vec![7]),
            )
            .expect("a fresh name");

        assert_eq!(assets.get(handle).map(Sound::bytes), Some(&[7][..]));
        assert_eq!(
            assets.get_by_name::<Sound>("footstep").map(Sound::bytes),
            Some(&[7][..])
        );
        assert_eq!(assets.len::<Sound>(), 1);
    }

    /// Removing a sound leaves a handle to it resolving to nothing, so a
    /// component that holds one does not have to be told about the removal.
    #[test]
    fn unloading_a_sound_leaves_its_handle_unresolvable() {
        let mut assets = AssetManager::new();
        let handle = assets
            .add_named(
                "footstep",
                Sound::from_bytes(std::path::Path::new("a.wav"), vec![7]),
            )
            .expect("a fresh name");
        assets.remove(handle);

        assert!(assets.get(handle).is_none());
        assert!(assets.get_by_name::<Sound>("footstep").is_none());
    }
}
