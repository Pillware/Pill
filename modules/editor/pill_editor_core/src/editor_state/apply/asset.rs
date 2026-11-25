//! Applies the asset-file command family: every editor command that reads or
//! writes a file under the asset directory.
//!
//! # Responsibilities
//!
//! - Apply the asset-file commands: import, remove, create, delete, restore,
//!   move, rename, write a sidecar and change a file's settings.
//! - Take a failed command's own work back, and name the files a rollback
//!   could not remove.
//!
//! # Design
//!
//! One `match` over the family's own variants, called from
//! [`EditorCommand::apply_one`](super::EditorCommand). Every arm ends in a
//! [`CommandOutcome`]: the inverse the history records, or `plain()` when the
//! command changed nothing and so has nothing to undo.

// External crates
use pill_engine::Engine;
use pill_renderer_api::editor::DefaultShader;

// Current crate
use super::CommandOutcome;
use super::*;

/// Applies one asset command.
///
/// # Errors
///
/// A message naming the command and the reason when:
///
/// - a path is outside the asset directory, or names a module's resource,
///   which the editor may only read;
/// - a file cannot be read, written, copied, moved or removed, or the file a
///   command would write already exists;
/// - the engine has no asset directory, no registered asset type, no
///   `AssetManager`, or no random source for a new guid;
/// - a written asset does not load, or the file the new settings go into is
///   not a YAML mapping.
///
/// A rollback that cannot finish adds the files it left on disk to the
/// message.
///
/// # Panics
///
/// Panics when `command` belongs to another family; `EditorCommand::apply_one`
/// routes only asset commands here.
pub(super) fn apply(
    engine: &mut Engine,
    command: &EditorCommand,
) -> Result<CommandOutcome, String> {
    match command {
        EditorCommand::ImportFiles {
            sources,
            destination,
        } => {
            // Check every source before copying anything, so one bad path in
            // a multi-file drop changes nothing.
            for source in sources {
                if !std::path::Path::new(source).is_file() {
                    return Err(format!("{command:?}: `{source}` is not a file"));
                }
            }
            std::fs::create_dir_all(destination)
                .map_err(|error| format!("{command:?}: {error}"))?;
            let mut created_paths: Vec<String> = Vec::new();
            for source in sources {
                let file_name = std::path::Path::new(source)
                    .file_name()
                    .ok_or_else(|| format!("{command:?}: `{source}` has no file name"))?;
                let target_path = std::path::Path::new(destination).join(file_name);
                // A source that is already the destination file (a drop of a
                // file the project has) is skipped: copying a file onto
                // itself would fail, and may truncate it.
                let source_path = std::path::Path::new(source);
                let is_same_file = match (source_path.canonicalize(), target_path.canonicalize()) {
                    (Ok(source), Ok(target)) => source == target,
                    _ => source_path == target_path,
                };
                if is_same_file {
                    continue;
                }
                if let Err(error) = std::fs::copy(source, &target_path) {
                    // Take back the copies this command already made. A copy
                    // that cannot be removed is named in the error, so the
                    // failure says what is still on disk.
                    let unremoved_paths = roll_back_files(&created_paths);
                    return Err(with_rollback_paths(
                        format!("{command:?}: {error}"),
                        &unremoved_paths,
                    ));
                }
                created_paths.push(target_path.to_string_lossy().into_owned());
            }
            if created_paths.is_empty() {
                // Every source was already in place: nothing changed, and
                // nothing is recorded.
                return Ok(CommandOutcome::plain());
            }
            Ok(CommandOutcome::inverse(EditorCommand::RemoveFiles {
                paths: created_paths,
                sources: sources.clone(),
                destination: destination.clone(),
            }))
        }

        EditorCommand::RemoveFiles {
            paths,
            sources,
            destination,
        } => {
            for path in paths {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    // Already gone is the state undo wanted anyway.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(format!("{command:?}: {error}")),
                }
            }
            Ok(CommandOutcome::inverse(EditorCommand::ImportFiles {
                sources: sources.clone(),
                destination: destination.clone(),
            }))
        }

        EditorCommand::CreateStandaloneAsset {
            type_name,
            folder,
            name,
            overwrite,
        } => {
            let root = asset_authoring_root()?;
            let registry = engine
                .world()
                .get_resource::<pill_engine::ImportRegistry>()
                .cloned()
                .ok_or_else(|| format!("{command:?}: no asset types are registered"))?;
            // Take the asset store before the file is written: an engine
            // without one must refuse the create rather than leave a file
            // that no asset manager will ever hold.
            let default_shader = engine
                .world()
                .get_resource::<DefaultShader>()
                .and_then(|default_shader| default_shader.guid());
            let assets = engine
                .world_mut()
                .get_resource_mut::<pill_engine::AssetManager>()
                .ok_or_else(|| format!("{command:?}: the engine has no AssetManager"))?;
            let extension = registry
                .standalone_types()
                .into_iter()
                .find(|(registered, _)| registered == type_name)
                .map(|(_, extension)| extension)
                .ok_or_else(|| {
                    format!("{command:?}: `{type_name}` is not a standalone asset type")
                })?;
            let folder = folder.trim().trim_matches('/');
            let relative_folder = if folder.is_empty() {
                PathBuf::new()
            } else {
                pill_engine::asset_metadata::checked_relative(folder)
                    .map_err(|message| format!("{command:?}: {message}"))?
            };
            let directory = root.join(&relative_folder);
            if !directory.is_dir() {
                // The asset root is created when the project has none yet.
                // A folder below it must already exist; the development
                // host's asset browser refuses the same create.
                if relative_folder.as_os_str().is_empty() {
                    std::fs::create_dir_all(&directory)
                        .map_err(|error| format!("{command:?}: {error}"))?;
                } else {
                    return Err(format!(
                        "{command:?}: the folder `{}` does not exist",
                        asset_slash(&relative_folder)
                    ));
                }
            }
            let file_name = pill_engine::asset_metadata::standalone_file_name(name, &extension)
                .map_err(|message| format!("{command:?}: {message}"))?;
            let relative = relative_folder.join(&file_name);
            refuse_module_resource(command, &relative)?;
            let destination = root.join(&relative);
            // An existing file is replaced only when the dialog said so,
            // and the dialog says which guid survives.
            let existing = destination.exists();
            if existing && overwrite.is_none() {
                return Err(format!(
                    "{command:?}: `{}` already exists",
                    asset_slash(&relative)
                ));
            }
            // The bytes the replace overwrites, kept so a write that fails
            // to load can be taken back byte for byte.
            let previous_bytes = if existing {
                Some(std::fs::read(&destination).map_err(|error| {
                    format!(
                        "{command:?}: cannot read `{}`: {error}",
                        asset_slash(&relative)
                    )
                })?)
            } else {
                None
            };
            // The asset store's guid for this path wins. A reference to an
            // asset is written as its guid, so the guid the store already
            // holds is the one to keep; only a path the store does not know
            // falls back to the guid in the file being replaced.
            let loaded_guid = registry.guid_by_path(assets, &relative);
            let kept_guid = match overwrite {
                Some(true) => {
                    loaded_guid.or_else(|| previous_bytes.as_deref().and_then(document_guid))
                }
                _ => None,
            };
            let document = registry
                .default_document(type_name)
                .map_err(|error| format!("{command:?}: {error}"))?;
            let document = with_default_shader(default_shader, document);
            let guid = match kept_guid {
                Some(guid) => guid,
                None => pill_engine::AssetGuid::random().map_err(|error| {
                    format!("{command:?}: no random source for a guid: {error}")
                })?,
            };
            // Drop an asset the store still holds under this path, unless
            // this write keeps its guid: an import refuses a path that is
            // loaded under a different guid than the file carries, and the
            // file written below carries the guid chosen here.
            let reloads_loaded = loaded_guid == Some(guid);
            if loaded_guid.is_some() && !reloads_loaded {
                registry.remove_loaded(assets, &relative);
            }
            let file_document = serde_json::json!({
                "format_version": pill_engine::asset_metadata::METADATA_FORMAT_VERSION,
                "asset_type": type_name,
                "guid": guid.to_string(),
                "asset": document,
            });
            let bytes = pill_engine::asset_document::render(&file_document)
                .map_err(|error| format!("{command:?}: {error}"))?;
            let sidecar = pill_engine::asset_metadata::metadata_path_for(&relative);
            let sidecar_existed = sidecar
                .as_ref()
                .is_some_and(|meta| root.join(meta).exists());
            if existing {
                pill_engine::asset_store::write_replacing(&destination, &bytes)
                    .map_err(|error| format!("{command:?}: {error}"))?;
            } else {
                pill_engine::asset_store::write_new(&destination, &bytes)
                    .map_err(|error| format!("{command:?}: {error}"))?;
            }
            // A kept guid means the store already holds this asset, and an
            // import would hand back that handle without decoding the new
            // file. A reimport reads the file again and replaces the value in
            // place, so the handle stays valid.
            let load_outcome = if reloads_loaded {
                registry
                    .reimport(assets, &relative)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            } else {
                registry
                    .import(
                        assets,
                        &relative,
                        pill_engine::MetadataPolicy::ReadIfPresent,
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            };
            if let Err(error) = load_outcome {
                // A file is only an asset once it loads. If it does not load,
                // take the write back rather than leave a file the project
                // cannot read: a replaced file gets its previous bytes again,
                // a new file is removed, and so is the sidecar this import
                // created.
                let mut unremoved_paths = Vec::new();
                match previous_bytes {
                    Some(previous_bytes) => {
                        let restored = pill_engine::asset_store::write_replacing(
                            &destination,
                            &previous_bytes,
                        );
                        if restored.is_err() {
                            unremoved_paths.push(destination.display().to_string());
                        }
                    }
                    None => {
                        let destination_text = destination.display().to_string();
                        unremoved_paths.extend(roll_back_files(&[destination_text]));
                        if let Some(meta) = sidecar.filter(|_| !sidecar_existed) {
                            let meta_text = root.join(meta).display().to_string();
                            unremoved_paths.extend(roll_back_files(&[meta_text]));
                        }
                    }
                }
                return Err(with_rollback_paths(
                    format!(
                        "{command:?}: `{}` did not load: {error}",
                        asset_slash(&relative)
                    ),
                    &unremoved_paths,
                ));
            }
            Ok(CommandOutcome::inverse(EditorCommand::DeleteAssetFile {
                path: asset_slash(&relative),
            }))
        }

        EditorCommand::DeleteAssetFile { path } => {
            let root = asset_authoring_root()?;
            let relative = pill_engine::asset_metadata::checked_relative(path)
                .map_err(|message| format!("{command:?}: {message}"))?;
            refuse_module_resource(command, &relative)?;
            let asset_file = root.join(&relative);
            let bytes = std::fs::read(&asset_file)
                .map_err(|error| format!("{command:?}: cannot read `{path}`: {error}"))?;
            let meta_relative = pill_engine::asset_metadata::metadata_path_for(&relative);
            let meta = match &meta_relative {
                Some(meta) => match std::fs::read(root.join(meta)) {
                    Ok(bytes) => Some(bytes),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(format!("{command:?}: {error}")),
                },
                None => None,
            };
            std::fs::remove_file(&asset_file)
                .map_err(|error| format!("{command:?}: cannot delete `{path}`: {error}"))?;
            if let Some(meta_relative) = meta_relative {
                match std::fs::remove_file(root.join(&meta_relative)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        // The asset file is already deleted. Put it back, so
                        // a delete that fails on the sidecar leaves the asset
                        // as it was.
                        return Err(match write_asset_bytes(&asset_file, &bytes) {
                            Ok(()) => format!(
                                "{command:?}: cannot delete `{path}`'s sidecar, so nothing was deleted: {error}"
                            ),
                            Err(restore) => format!(
                                "{command:?}: cannot delete `{path}`'s sidecar ({error}) and cannot restore the asset: {restore}"
                            ),
                        });
                    }
                }
            }
            // The file is gone, so drop the asset the store still holds for
            // this path. A file that appears here again carries its own guid,
            // and an import refuses a path that is loaded under a different
            // guid than the file.
            if let Some(registry) = engine
                .world()
                .get_resource::<pill_engine::ImportRegistry>()
                .cloned()
            {
                if let Some(assets) = engine
                    .world_mut()
                    .get_resource_mut::<pill_engine::AssetManager>()
                {
                    registry.remove_loaded(&mut *assets, &relative);
                }
            }
            Ok(CommandOutcome::inverse(EditorCommand::RestoreAssetFile {
                path: path.clone(),
                bytes,
                meta,
            }))
        }

        EditorCommand::RestoreAssetFile { path, bytes, meta } => {
            let root = asset_authoring_root()?;
            let relative = pill_engine::asset_metadata::checked_relative(path)
                .map_err(|message| format!("{command:?}: {message}"))?;
            refuse_module_resource(command, &relative)?;
            write_asset_bytes(&root.join(&relative), bytes)
                .map_err(|message| format!("{command:?}: {message}"))?;
            if let Some(meta_relative) = pill_engine::asset_metadata::metadata_path_for(&relative) {
                let meta_path = root.join(&meta_relative);
                match meta {
                    Some(meta) => write_asset_bytes(&meta_path, meta)
                        .map_err(|message| format!("{command:?}: {message}"))?,
                    // The deleted asset had no sidecar, so the restore writes
                    // none. A sidecar sitting there now belongs to nothing,
                    // and is removed; one that cannot be removed is named in
                    // the warning.
                    None => {
                        let meta_text = meta_path.display().to_string();
                        warn_left_behind(&roll_back_files(&[meta_text]));
                    }
                }
            }
            Ok(CommandOutcome::inverse(EditorCommand::DeleteAssetFile {
                path: path.clone(),
            }))
        }

        EditorCommand::MoveAssetFile { from, to } => {
            let root = asset_authoring_root()?;
            let from_relative = pill_engine::asset_metadata::checked_relative(from)
                .map_err(|message| format!("{command:?}: {message}"))?;
            let to_relative = pill_engine::asset_metadata::checked_relative(to)
                .map_err(|message| format!("{command:?}: {message}"))?;
            refuse_module_resource(command, &from_relative)?;
            refuse_module_resource(command, &to_relative)?;
            let destination = root.join(&to_relative);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| format!("{command:?}: {error}"))?;
            }
            if destination.exists() {
                return Err(format!(
                    "{command:?}: `{}` already exists",
                    asset_slash(&to_relative)
                ));
            }
            pill_assets::move_asset(&root.join(&from_relative), &destination)
                .map_err(|error| format!("{command:?}: {error}"))?;
            Ok(CommandOutcome::inverse(EditorCommand::MoveAssetFile {
                from: to.clone(),
                to: from.clone(),
            }))
        }

        EditorCommand::WriteAssetMeta { path } => {
            let root = asset_authoring_root()?;
            let relative = pill_engine::asset_metadata::checked_relative(path)
                .map_err(|message| format!("{command:?}: {message}"))?;
            refuse_module_resource(command, &relative)?;
            let Some(meta_relative) = pill_engine::asset_metadata::metadata_path_for(&relative)
            else {
                return Err(format!("{command:?}: `{path}` cannot have a sidecar"));
            };
            let meta_path = root.join(&meta_relative);
            if meta_path.is_file() {
                // A sidecar that already holds a guid is not missing: nothing
                // changes and nothing is recorded. A sidecar left empty or
                // unreadable by an interrupted write counts as missing and is
                // replaced; the undo removes the replacement.
                let carries_guid = std::fs::read(&meta_path)
                    .ok()
                    .and_then(|bytes| pill_engine::asset_document::parse(&bytes).ok())
                    .and_then(|document| {
                        document
                            .get("guid")
                            .and_then(|guid| guid.as_str())
                            .map(str::to_owned)
                    })
                    .is_some();
                if carries_guid {
                    return Ok(CommandOutcome::plain());
                }
                std::fs::remove_file(&meta_path)
                    .map_err(|error| format!("{command:?}: {error}"))?;
            }
            let registry = engine
                .world()
                .get_resource::<pill_engine::ImportRegistry>()
                .cloned()
                .ok_or_else(|| format!("{command:?}: no asset types are registered"))?;
            let assets = engine
                .world_mut()
                .get_resource_mut::<pill_engine::AssetManager>()
                .ok_or_else(|| format!("{command:?}: the engine has no AssetManager"))?;
            registry
                .ensure_metadata(assets, &relative)
                .map_err(|error| format!("{command:?}: {error}"))?;
            Ok(CommandOutcome::inverse(EditorCommand::DeleteAssetFile {
                path: asset_slash(&meta_relative),
            }))
        }

        EditorCommand::SetAssetSettings { path, settings } => {
            let root = asset_authoring_root()?;
            let relative = pill_engine::asset_metadata::checked_relative(path)
                .map_err(|message| format!("{command:?}: {message}"))?;
            refuse_module_resource(command, &relative)?;
            let registry = engine
                .world()
                .get_resource::<pill_engine::ImportRegistry>()
                .cloned()
                .ok_or_else(|| format!("{command:?}: no asset types are registered"))?;
            let extension = relative
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_owned();
            let type_name = registry
                .type_for_extension(&extension)
                .ok_or_else(|| format!("{command:?}: no asset type imports `.{extension}` files"))?
                .to_owned();
            let normalized_settings = registry
                .normalize_settings_json(&type_name, settings.clone())
                .map_err(|error| format!("{command:?}: {error}"))?;
            let previous_settings = registry
                .settings_json(&relative)
                .map_err(|error| format!("{command:?}: {error}"))?;
            let is_standalone = registry.is_standalone_extension(&extension);
            let body_field = if is_standalone { "asset" } else { "settings" };
            let settings_file = if is_standalone {
                root.join(&relative)
            } else {
                let meta_relative = pill_engine::asset_metadata::metadata_path_for(&relative)
                    .ok_or_else(|| format!("{command:?}: `{path}` cannot have a sidecar"))?;
                let settings_file = root.join(&meta_relative);
                if !settings_file.is_file() {
                    // The import writes the sidecar with the type's default
                    // settings and a guid. The write below then rewrites that
                    // document with the new settings and the same guid.
                    let assets = engine
                        .world_mut()
                        .get_resource_mut::<pill_engine::AssetManager>()
                        .ok_or_else(|| format!("{command:?}: the engine has no AssetManager"))?;
                    registry
                        .import(
                            assets,
                            &relative,
                            pill_engine::MetadataPolicy::CreateIfMissing,
                        )
                        .map_err(|error| format!("{command:?}: {error}"))?;
                }
                settings_file
            };
            let file_bytes = std::fs::read(&settings_file).map_err(|error| {
                format!(
                    "{command:?}: cannot read `{}`: {error}",
                    settings_file.display()
                )
            })?;
            let mut document = pill_engine::asset_document::parse(&file_bytes)
                .map_err(|error| format!("{command:?}: `{}`: {error}", settings_file.display()))?;
            let document_object = document.as_object_mut().ok_or_else(|| {
                format!(
                    "{command:?}: `{}` is not a YAML mapping",
                    settings_file.display()
                )
            })?;
            document_object.insert(body_field.to_owned(), normalized_settings);
            let bytes = pill_engine::asset_document::render(&document)
                .map_err(|error| format!("{command:?}: {error}"))?;
            write_asset_bytes(&settings_file, &bytes)
                .map_err(|message| format!("{command:?}: {message}"))?;
            Ok(CommandOutcome::inverse(EditorCommand::SetAssetSettings {
                path: path.clone(),
                settings: previous_settings,
            }))
        }

        other => unreachable!("{:?} is not an asset command", other.family()),
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Writes the renderer's default shader into a created document that names none.
///
/// A new material - or a new pass - is written with the shader it draws through,
/// so the file says which one that is instead of leaving it to a substitution
/// the person cannot see. The renderer names that shader through
/// [`DefaultShader`], because only it knows its chain.
///
/// A document with no `shader` key belongs to a type that has no shader slot, and
/// one that already names a shader keeps it. A world whose renderer never
/// answered - no renderer loaded, or one that could not name a shader - leaves
/// the document exactly as its type defaulted it, which is what makes this
/// additive.
fn with_default_shader(
    default_shader: Option<pill_engine::AssetGuid>,
    mut document: serde_json::Value,
) -> serde_json::Value {
    let Some(guid) = default_shader else {
        return document;
    };
    let Some(fields) = document.as_object_mut() else {
        return document;
    };
    if !fields.get("shader").is_some_and(serde_json::Value::is_null) {
        return document;
    }
    fields.insert(
        "shader".to_owned(),
        serde_json::Value::String(guid.to_string()),
    );
    document
}
