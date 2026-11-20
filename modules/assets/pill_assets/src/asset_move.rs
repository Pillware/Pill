//! Moving a source asset together with its metadata file.
//!
//! # Responsibilities
//!
//! - Move a source file and its `<file>.meta` metadata file as one operation
//!   ([`move_asset`]), so the guid stored in the metadata follows the source.
//! - Refuse a move that would overwrite anything, that asks to move a metadata
//!   file itself, or whose source does not exist.
//!
//! # Design
//!
//! A source moved without its `.meta` loses its guid. The next import writes a
//! new guid into a new `.meta` file beside it, and every saved reference to the
//! old guid stops resolving. This is the one supported way to rename an asset.
//! The editor's asset commands call it instead of moving files themselves.
//!
//! The source moves first and the metadata second. If the second rename fails,
//! the first one is undone, so the two files are not left apart on purpose. A
//! crash between the two renames can still separate them. The dev host's
//! watcher and the `test_asset_metadata.py` check are what notice that.

// Standard library
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// =============================================================================
// Constants
// =============================================================================

/// The suffix a metadata file adds to its source's full file name.
const METADATA_SUFFIX: &str = ".meta";

// =============================================================================
// Types
// =============================================================================

/// Why [`move_asset`] did not move anything.
#[derive(Debug)]
pub enum MoveAssetError {
    /// There is no source file at the origin.
    SourceMissing {
        /// The origin that was asked for.
        path: PathBuf,
    },
    /// The origin is itself a metadata file; move its source instead.
    IsMetadataFile {
        /// The origin that was asked for.
        path: PathBuf,
    },
    /// Something already exists where the source or its metadata would go.
    TargetExists {
        /// The path that is already taken.
        path: PathBuf,
    },
    /// A filesystem operation failed; the move is undone when that is
    /// possible.
    Io {
        /// The path the operation touched.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
}

impl fmt::Display for MoveAssetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceMissing { path } => {
                write!(
                    formatter,
                    "there is no asset at `{}` to move",
                    path.display()
                )
            }
            Self::IsMetadataFile { path } => write!(
                formatter,
                "`{}` is a metadata file; move its source and it follows",
                path.display()
            ),
            Self::TargetExists { path } => write!(
                formatter,
                "`{}` already exists; an asset move never overwrites",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(formatter, "could not move `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for MoveAssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the metadata file path of the source at `path`.
///
/// The name is the source's full file name with `.meta` appended, so
/// `textures/stone.png` gives `textures/stone.png.meta`.
pub fn metadata_path_of(path: &Path) -> PathBuf {
    let mut metadata_file_name = path.as_os_str().to_owned();
    metadata_file_name.push(METADATA_SUFFIX);
    PathBuf::from(metadata_file_name)
}

/// Moves the source asset at `source_path` to `target_path`, together with its
/// `.meta` file when it has one.
///
/// Both arguments are filesystem paths. The destination's directory is created
/// when it is missing, and nothing is overwritten.
///
/// # Errors
///
/// - [`MoveAssetError::SourceMissing`] when `source_path` is not a file.
/// - [`MoveAssetError::IsMetadataFile`] when `source_path` is a `.meta` file
///   itself.
/// - [`MoveAssetError::TargetExists`] when `target_path`, or the path its
///   metadata file would take, already exists.
/// - [`MoveAssetError::Io`] when creating the destination's directory or a
///   rename fails; a failed metadata rename attempts to move the source back
///   before it returns.
pub fn move_asset(source_path: &Path, target_path: &Path) -> Result<(), MoveAssetError> {
    // Step 1: Refuse a metadata file, and an origin that is not a file.
    if source_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("meta"))
    {
        return Err(MoveAssetError::IsMetadataFile {
            path: source_path.to_owned(),
        });
    }
    if !source_path.is_file() {
        return Err(MoveAssetError::SourceMissing {
            path: source_path.to_owned(),
        });
    }
    // Step 2: Refuse an occupied destination before anything moves, so a
    // refusal leaves the origin exactly as it was.
    let from_metadata = metadata_path_of(source_path);
    let to_metadata = metadata_path_of(target_path);
    for target_path in [target_path, to_metadata.as_path()] {
        if target_path.exists() {
            return Err(MoveAssetError::TargetExists {
                path: target_path.to_owned(),
            });
        }
    }
    // Step 3: Create the destination's directory when it is missing.
    if let Some(parent_directory) = target_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent_directory).map_err(|error| MoveAssetError::Io {
            path: parent_directory.to_owned(),
            source: error,
        })?;
    }

    // Step 4: Move the source, then its metadata.
    fs::rename(source_path, target_path).map_err(|error| MoveAssetError::Io {
        path: source_path.to_owned(),
        source: error,
    })?;
    if from_metadata.is_file() {
        if let Err(error) = fs::rename(&from_metadata, &to_metadata) {
            // Undo the source move rather than leave the guid behind.
            let _ = fs::rename(target_path, source_path);
            return Err(MoveAssetError::Io {
                path: from_metadata,
                source: error,
            });
        }
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory, removed afterwards.
    struct ScratchDirectory(PathBuf);

    impl ScratchDirectory {
        /// Creates the directory, replacing any directory already at that
        /// path.
        ///
        /// # Panics
        ///
        /// Panics when the directory cannot be created.
        fn new(test_label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "pill-asset-move-{test_label}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        /// Writes `contents` to `relative_path` under the directory and returns
        /// the file's path.
        ///
        /// # Panics
        ///
        /// Panics when the file's directory cannot be created or the file
        /// cannot be written.
        fn write(&self, relative_path: &str, contents: &str) -> PathBuf {
            let path = self.0.join(relative_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for ScratchDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_source_and_its_metadata_move_together() {
        let scratch_directory = ScratchDirectory::new("together");
        let from = scratch_directory.write("a.png", "pixels");
        scratch_directory.write("a.png.meta", "guid");
        let to = scratch_directory.0.join("textures/b.png");

        move_asset(&from, &to).unwrap();

        assert_eq!(fs::read_to_string(&to).unwrap(), "pixels");
        assert_eq!(
            fs::read_to_string(scratch_directory.0.join("textures/b.png.meta")).unwrap(),
            "guid"
        );
        assert!(!from.exists() && !scratch_directory.0.join("a.png.meta").exists());
    }

    #[test]
    fn a_source_without_metadata_moves_alone() {
        let scratch_directory = ScratchDirectory::new("alone");
        let from = scratch_directory.write("a.png", "pixels");
        let to = scratch_directory.0.join("b.png");

        move_asset(&from, &to).unwrap();

        assert!(to.is_file());
        assert!(!scratch_directory.0.join("b.png.meta").exists());
    }

    #[test]
    fn an_existing_target_or_target_metadata_is_refused() {
        let scratch_directory = ScratchDirectory::new("refused");
        let from = scratch_directory.write("a.png", "pixels");
        scratch_directory.write("a.png.meta", "guid");
        let occupied_target = scratch_directory.write("b.png", "other");
        scratch_directory.write("c.png.meta", "stray");

        for target_path in [occupied_target.clone(), scratch_directory.0.join("c.png")] {
            let error = move_asset(&from, &target_path).unwrap_err();
            assert!(
                matches!(error, MoveAssetError::TargetExists { .. }),
                "{error}"
            );
        }
        // Nothing moved and nothing was overwritten.
        assert_eq!(fs::read_to_string(&from).unwrap(), "pixels");
        assert_eq!(fs::read_to_string(&occupied_target).unwrap(), "other");
        assert!(scratch_directory.0.join("a.png.meta").is_file());
    }

    #[test]
    fn a_missing_source_or_a_metadata_file_is_refused() {
        let scratch_directory = ScratchDirectory::new("bad-origin");
        let metadata_file_path = scratch_directory.write("a.png.meta", "guid");

        assert!(matches!(
            move_asset(
                &scratch_directory.0.join("missing.png"),
                &scratch_directory.0.join("x.png")
            ),
            Err(MoveAssetError::SourceMissing { .. })
        ));
        assert!(matches!(
            move_asset(&metadata_file_path, &scratch_directory.0.join("x.png.meta")),
            Err(MoveAssetError::IsMetadataFile { .. })
        ));
    }
}
