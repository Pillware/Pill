//! The executable's half of the Assets pipeline.
//!
//! # Responsibilities
//!
//! - Walk the asset tree into the listing the shell draws, and refresh it on
//!   its cadence or at once after a file command.
//! - Capture the selected asset's detail - its import settings and the
//!   reference fields its type declares - for the Inspector.
//! - Ask the renderer for the previews of the tiles that are on screen.
//! - Show what a drag from the OS would do, and import the files it drops
//!   into the open folder.
//!
//! # Design
//!
//! The shell never touches the filesystem: it queues file commands and draws
//! what the snapshot carries. This module is the executable's side of that
//! split: it walks the tree into the listing and reads an asset's settings
//! through the import registry, so the shell only draws data it was handed.

// Current crate
use super::*;

impl EditorWindow {
    /// Moves the previews the renderer finished into the shell's textures, then
    /// asks the renderer for one more.
    ///
    /// Only one request runs at a time: the renderer draws one preview per
    /// frame and reads it back between frames, so a grid of tiles fills in over
    /// frames instead of stalling one. A tile is asked for only once - its
    /// texture stays in the session until the listing changes - and the tiles
    /// to ask for come from the grid's own list of what it drew this pass, so
    /// scrolling only costs work for what is on screen.
    pub(super) fn apply_asset_previews(&mut self) {
        // Step 1: Move the previews the renderer finished into the shell's
        // textures.
        let images = {
            let world = self.host.host_mut().engine_mut().world_mut();
            world
                .get_resource_mut::<pill_player::PreviewResults>()
                .map(|preview_results| preview_results.take())
                .unwrap_or_default()
        };
        if !images.is_empty() {
            let world = self.host.host_mut().engine_mut().world_mut();
            let Some(state) = world.get_resource_mut::<EguiShellState>() else {
                return;
            };
            for image in images {
                let Some(path) = self.preview_in_flight.remove(&image.id) else {
                    continue;
                };
                // A refused preview carries no pixels: the tile keeps its type
                // icon, and the grid asks for the next tile instead of waiting
                // for this file forever.
                if image.is_empty() {
                    state.previews.mark_refused(&path);
                    info!(
                        target: telemetry_target::EDITOR,
                        path = path.as_str(),
                        id = image.id,
                        "[editor] preview refused"
                    );
                    continue;
                }
                let image_size = [image.width as usize, image.height as usize];
                let texture = state.context.load_texture(
                    format!("preview:{path}"),
                    egui::ColorImage::from_rgba_unmultiplied(image_size, &image.rgba),
                    egui::TextureOptions::LINEAR,
                );
                state.previews.mark_ready(&path, texture);
                info!(
                    target: telemetry_target::EDITOR,
                    path = path.as_str(),
                    width = image.width,
                    height = image.height,
                    "[editor] preview ready"
                );
            }
        }
        if !self.render_previews || !self.preview_in_flight.is_empty() {
            return;
        }
        // Step 2: Name the next tile that still wants an image, from the tiles
        // the grid drew this pass, in draw order.
        let waiting_keys: Vec<String> = {
            let world = self.host.host_mut().engine_mut().world_mut();
            let Some(state) = world.get_resource::<EguiShellState>() else {
                return;
            };
            state
                .previews
                .visible()
                .iter()
                .filter(|key| state.previews.wants(key))
                .cloned()
                .collect()
        };
        if waiting_keys.is_empty() {
            return;
        }
        // A texture or a mesh preview names the file to draw, so the request
        // needs the asset root the listing's paths are relative to.
        let asset_root = self
            .host
            .host_mut()
            .asset_directory()
            .map(std::path::Path::to_path_buf);
        let Some(asset_root) = asset_root else {
            return;
        };
        // The first tile the listing can give a preview for. A row with none -
        // a folder, a scene, a file type the renderer cannot draw - is recorded
        // as refused below, so the grid stops asking for it on every pass.
        let mut skipped_keys = Vec::new();
        let mut next_preview = None;
        for key in waiting_keys {
            match previews::preview_asset_for(self.asset_entries.iter(), &key, &asset_root) {
                Some(asset) => {
                    next_preview = Some((key, asset));
                    break;
                }
                None => skipped_keys.push(key),
            }
        }
        if !skipped_keys.is_empty() {
            let world = self.host.host_mut().engine_mut().world_mut();
            if let Some(state) = world.get_resource_mut::<EguiShellState>() {
                for key in &skipped_keys {
                    state.previews.mark_refused(key);
                }
            }
            for key in &skipped_keys {
                info!(
                    target: telemetry_target::EDITOR,
                    path = key.as_str(),
                    "[editor] preview skipped"
                );
            }
        }
        let Some((path, asset)) = next_preview else {
            return;
        };
        // Step 3: Ask the renderer to draw it. The id it answers with names the
        // tile while the request is in flight.
        let id = {
            let world = self.host.host_mut().engine_mut().world_mut();
            pill_player::ensure_preview_resources(world);
            let Some(requests) = world.get_resource_mut::<pill_player::PreviewRequests>() else {
                return;
            };
            requests.request(asset)
        };
        self.preview_in_flight.insert(id, path.clone());
        info!(
            target: telemetry_target::EDITOR,
            path = path.as_str(),
            id,
            "[editor] preview requested"
        );
    }

    /// Refreshes the asset browser's rows when the cadence is due or a file
    /// command marked them dirty, and picks up the live standalone types the
    /// create dialog offers.
    ///
    /// Returns whether the listing changed, so the captured asset detail is
    /// read again only when it may be stale.
    pub(super) fn refresh_asset_listing(&mut self) -> bool {
        if !(self.frame_index.is_multiple_of(ASSET_REFRESH_FRAMES) || self.asset_listing_dirty) {
            return false;
        }
        // A file command asked for this pass, not the cadence, so the tiles'
        // images and the Inspector's capture both have to follow it.
        let requested_by_edit = self.asset_listing_dirty;
        self.asset_listing_dirty = false;
        // Step 1: Walk the tree into one row per file and folder.
        let listing_rows: Vec<AssetListing> = self
            .host
            .host_mut()
            .asset_entries()
            .into_iter()
            .take(ASSET_ROWS_MAX)
            .map(|entry| AssetListing {
                root: entry.root,
                path: entry.path,
                read_only: entry.read_only,
                kind: if entry.is_directory {
                    AssetKind::Folder
                } else {
                    AssetKind::parse(&entry.type_name.unwrap_or_else(|| "file".to_string()))
                },
                standalone: entry.standalone,
                has_meta: entry.guid.is_some(),
                guid: entry.guid.map(|guid| guid.to_string()),
            })
            .collect();
        // Step 2: Keep the previews whose row did not change. The walk runs on
        // a cadence, so most passes find the tree exactly as it was; only a
        // moved tree or an edit makes a tile's image stale. Clearing every
        // preview on each pass would make the tiles blink back to their icons
        // for a moment.
        let tree_moved = listing_rows != self.asset_walk;
        // A tree that moved keeps the images of the rows that did not change:
        // a create or a delete touches one asset, and re-rendering every tile
        // for it would flicker the whole grid. An edit that left the tree alone
        // names no asset, so every image may be stale.
        let unchanged_keys: std::collections::BTreeSet<String> = if tree_moved {
            listing_rows
                .iter()
                .filter(|row| {
                    self.asset_walk
                        .iter()
                        .any(|old| old.key() == row.key() && *old == **row)
                })
                .map(AssetListing::key)
                .collect()
        } else {
            std::collections::BTreeSet::new()
        };
        self.asset_walk = listing_rows.clone();
        self.asset_entries = listing_rows;
        if tree_moved || requested_by_edit {
            let world = self.host.host_mut().engine_mut().world_mut();
            if let Some(state) = world.get_resource_mut::<EguiShellState>() {
                if tree_moved {
                    state.previews.retain_keys(&unchanged_keys);
                } else {
                    self.preview_in_flight.clear();
                    state.previews.clear();
                }
            }
        }
        // Step 3: Give every module root its own row under the extensions root.
        // The walk lists what is inside a root, and the tree can only open a
        // folder it has a row for; the extensions root itself is not a row,
        // because the panel draws that one from its own root list.
        let root_names: std::collections::BTreeSet<String> = self
            .asset_entries
            .iter()
            .map(|row| row.root.clone())
            .filter(|root| !root.is_empty())
            .collect();
        for root in root_names.into_iter().rev() {
            self.asset_entries.insert(
                0,
                AssetListing {
                    root,
                    path: String::new(),
                    read_only: true,
                    kind: AssetKind::Folder,
                    standalone: false,
                    has_meta: false,
                    guid: None,
                },
            );
        }
        // Step 4: Refresh the standalone types the create dialog offers.
        let standalone_types =
            pill_development_host::standalone_types(self.host.host_mut().engine_mut().world());
        self.standalone_types = standalone_types
            .into_iter()
            .map(|standalone_type| StandaloneTypeInfo {
                type_name: standalone_type.type_name,
                display_name: standalone_type.display_name,
                extension: standalone_type.extension,
            })
            .collect();
        let listing_changed = tree_moved || requested_by_edit;
        if listing_changed {
            info!(
                target: telemetry_target::EDITOR,
                count = self.asset_entries.len(),
                types = self.standalone_types.len(),
                "[editor] asset listing refreshed"
            );
        }
        listing_changed
    }

    /// Captures the selected asset's detail for the Inspector's asset section,
    /// when the selection or the listing changed. Reading the settings document
    /// is a disk read, so the capture follows the listing's cadence instead of
    /// running every frame. A settings write forgets the captured path, so the
    /// widgets see the document the command wrote.
    pub(super) fn refresh_asset_detail(&mut self, listing_changed: bool) {
        let selected_asset = {
            let world = self.host.host_mut().engine_mut().world_mut();
            world
                .get_resource::<EditorSelection>()
                .and_then(|selection| selection.asset.clone())
        };
        if !listing_changed && selected_asset == self.asset_detail_path {
            return;
        }
        self.asset_detail_path = selected_asset.clone();
        self.asset_detail = selected_asset.map(|key| {
            // A selection the listing no longer holds - an asset deleted
            // between two refreshes - reads as a plain file row at its own
            // path.
            let row = self
                .asset_entries
                .iter()
                .find(|row| row.key() == key)
                .cloned()
                .unwrap_or_else(|| AssetListing {
                    root: String::new(),
                    path: key.clone(),
                    read_only: false,
                    kind: AssetKind::Type("file".to_string()),
                    standalone: false,
                    has_meta: false,
                    guid: None,
                });
            // The settings document is read by the path inside the root: the
            // store searches every mounted root, so a module's resource reads
            // like a project asset.
            let settings_document = if row.kind.is_folder() {
                None
            } else {
                self.read_asset_settings(&row.path)
            };
            let references = self.asset_reference_fields(row.kind.text(), &settings_document);
            let decorators = self.asset_field_decorators(row.kind.text());
            AssetDetail {
                row,
                settings: settings_document,
                references,
                decorators,
            }
        });
    }

    /// Returns how the asset type's document fields draw, as the registry the
    /// registering crate declared them.
    ///
    /// A settings asset states that a value is a range or carries an
    /// explanation where the document is defined, so the Inspector draws the
    /// row the type asked for without knowing the type. A type that declares
    /// none gets the rows an undecorated document gets.
    pub(super) fn asset_field_decorators(
        &mut self,
        asset_type_name: &str,
    ) -> Vec<pill_editor_core::editor_state::AssetFieldDecorator> {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(registry) = world.get_resource::<pill_engine_core::ImportRegistry>() else {
            return Vec::new();
        };
        registry
            .field_decorators(asset_type_name)
            .iter()
            .map(
                |(key, decorator)| pill_editor_core::editor_state::AssetFieldDecorator {
                    key: key.clone(),
                    decorator: decorator.clone(),
                },
            )
            .collect()
    }

    /// Returns the reference fields the asset's type declares, expanded against
    /// its document: one entry per picker the Inspector draws, carrying the
    /// guid the document holds now.
    ///
    /// The registry names the fields and the type each one points at, such as
    /// `shader`, `textures.*` and `passes.*`. A name ending in `.*` stands for a
    /// group whose size only the document knows, so those are expanded here: a
    /// material's texture slots are read from the shader it uses, and a chain's
    /// entries from its own list.
    ///
    /// Returns an empty list when the asset has no settings document, or when
    /// no import registry holds a registered type.
    pub(super) fn asset_reference_fields(
        &mut self,
        asset_type_name: &str,
        settings_document: &Option<serde_json::Value>,
    ) -> Vec<AssetReferenceField> {
        let Some(settings_document) = settings_document.as_ref() else {
            return Vec::new();
        };
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(registry) = world
            .get_resource::<pill_engine_core::ImportRegistry>()
            .cloned()
        else {
            return Vec::new();
        };
        let declared_fields = registry.reference_fields(asset_type_name).to_vec();
        let mut fields = Vec::new();
        for (key, target) in declared_fields {
            let Some(reference_family) = key.strip_suffix(".*") else {
                let field_value = settings_document.get(&key);
                let guid = reference_guid(field_value);
                fields.push(AssetReferenceField {
                    key,
                    target,
                    label: self.asset_label(&guid),
                    guid,
                    nested: !matches!(field_value, Some(serde_json::Value::String(_))),
                });
                continue;
            };
            match reference_family {
                "textures" => {
                    for slot in self.shader_texture_slots(&Some(settings_document.clone())) {
                        let slot_value = settings_document
                            .get("textures")
                            .and_then(|map| map.get(&slot));
                        let guid = reference_guid(slot_value);
                        fields.push(AssetReferenceField {
                            key: format!("textures.{slot}"),
                            target: target.clone(),
                            label: self.asset_label(&guid),
                            guid,
                            nested: false,
                        });
                    }
                }
                "passes" => {
                    let pass_entries = settings_document
                        .get(reference_family)
                        .and_then(|list| list.as_array())
                        .cloned()
                        .unwrap_or_default();
                    for (index, entry) in pass_entries.iter().enumerate() {
                        let guid = reference_guid(Some(entry));
                        fields.push(AssetReferenceField {
                            key: format!("passes[{index}]"),
                            target: target.clone(),
                            label: self.asset_label(&guid),
                            guid,
                            nested: true,
                        });
                    }
                }
                other => debug_assert!(false, "unexpanded reference family `{other}`"),
            }
        }
        fields
    }

    /// Returns what a reference row shows for the asset `guid` names: the
    /// listing's path when the listing holds the file, otherwise the name the
    /// store loaded it under, or `None` when neither knows the guid. A chain's
    /// built-in passes are built in code, so their guid is the only thing a
    /// file walk can know about them.
    pub(super) fn asset_label(&mut self, guid: &Option<String>) -> Option<String> {
        let guid = guid.as_deref()?;
        if let Some(path) = self
            .asset_entries
            .iter()
            .find(|row| row.guid.as_deref() == Some(guid))
            .map(|row| row.path.clone())
        {
            return Some(path);
        }
        let asset_guid = pill_engine_core::AssetGuid::parse(guid)?;
        let world = self.host.host_mut().engine_mut().world_mut();
        let registry = world
            .get_resource::<pill_engine_core::ImportRegistry>()
            .cloned()?;
        let asset_manager = world.get_resource::<pill_engine_core::AssetManager>()?;
        registry
            .loaded_by_guid(asset_manager, asset_guid)
            .map(|(_type_name, name, _index, _generation)| name)
    }

    /// Returns the texture slots declared by the shader a material document
    /// names.
    ///
    /// A material's rows are the shader's slots: `RendererMaterial` builds its
    /// textures bind group from the slots the shader declares, so the Inspector
    /// asks the same source. The document names the shader by guid, the listing
    /// turns that guid into a path, and the shader's metadata file at that path
    /// declares the slots. An empty list means the document names no shader, or
    /// the shader is not in the listing or cannot be read.
    pub(super) fn shader_texture_slots(
        &mut self,
        settings_document: &Option<serde_json::Value>,
    ) -> Vec<String> {
        let Some(shader_reference) = settings_document
            .as_ref()
            .and_then(|document| document.get("shader"))
        else {
            return Vec::new();
        };
        let guid = match shader_reference {
            serde_json::Value::String(guid) => guid.clone(),
            other => match other.get("guid").and_then(|guid| guid.as_str()) {
                Some(guid) => guid.to_owned(),
                None => return Vec::new(),
            },
        };
        let Some(path) = self
            .asset_entries
            .iter()
            .find(|row| row.guid.as_deref() == Some(guid.as_str()))
            .map(|row| row.path.clone())
        else {
            return Vec::new();
        };
        let Some(shader_settings) = self.read_asset_settings(&path) else {
            return Vec::new();
        };
        shader_settings
            .get("textures")
            .and_then(|slots| slots.as_array())
            .map(|slots| {
                slots
                    .iter()
                    .filter_map(|slot| slot.get("name")?.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns the settings document an asset's editable rows build on, read
    /// through the import registry - the same reader the `SetAssetSettings`
    /// command uses. `None` when no registered type imports the path or its
    /// document cannot be read.
    pub(super) fn read_asset_settings(&mut self, path: &str) -> Option<serde_json::Value> {
        let world = self.host.host_mut().engine_mut().world_mut();
        let registry = world
            .get_resource::<pill_engine_core::ImportRegistry>()?
            .clone();
        let relative = pill_engine_core::asset_metadata::checked_relative(path).ok()?;
        registry.settings_json(&relative).ok()
    }

    /// Shows what a drag from the OS is carrying: a toast naming the file and
    /// the folder it would land in, so the person sees where a drop goes before
    /// letting go. The toast is not repeated while the same path stays the
    /// dragged one.
    pub(super) fn preview_drop(&mut self, path: &std::path::Path) {
        if self.drop_preview.as_deref() == Some(path) {
            return;
        }
        self.drop_preview = Some(path.to_path_buf());
        let folder = self.drop_folder();
        let file_name = path
            .file_name()
            .map(|file_name| file_name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        info!(
            target: telemetry_target::EDITOR,
            path = %path.display(),
            folder = folder.as_str(),
            "[editor] drop preview"
        );
        self.push_toast(NoticeLevel::Note, format!("drop {file_name} into {folder}"));
    }

    /// Clears the drop preview when the drag has left every window.
    pub(super) fn cancel_drop_preview(&mut self) {
        if self.drop_preview.take().is_some() {
            info!(
                target: telemetry_target::EDITOR,
                "[editor] drop preview cancelled"
            );
        }
    }

    /// Returns the Assets folder a drop lands in, as a store path: the
    /// session's open folder under `res`, or `res` itself for the root.
    pub(super) fn drop_folder(&mut self) -> String {
        let world = self.host.host_mut().engine_mut().world_mut();
        let folder = world
            .get_resource::<pill_editor_core::session::EguiShellState>()
            .map(|state| state.browser.folder.clone())
            .unwrap_or_default();
        if folder.is_empty() {
            "res".to_string()
        } else {
            format!("res/{folder}")
        }
    }

    /// Imports the paths a drop delivered into the open folder.
    ///
    /// A folder is refused by name, because the import copies files and a
    /// directory would need a recursive walk with its own refusals. The refusal
    /// is a toast as well as a log line. Files become one recorded
    /// `ImportFiles` step, so undo takes them off the disk again.
    pub(super) fn drop_files(&mut self, paths: Vec<std::path::PathBuf>) {
        self.drop_preview = None;
        // Step 1: Work out where the files go, refusing the drop when the host
        // has no asset directory.
        let Some(asset_root) = self
            .host
            .host_mut()
            .asset_directory()
            .map(|directory| directory.to_path_buf())
        else {
            warn!(
                target: telemetry_target::EDITOR,
                "[editor] drop refused: the host knows no asset directory"
            );
            self.push_toast(
                NoticeLevel::Warning,
                "drop refused: no asset directory".to_string(),
            );
            return;
        };
        let folder = self.drop_folder();
        let sub_folder = match pill_editor_core::browser::drop_folder_relative(&folder) {
            Ok(relative) => relative,
            Err(message) => {
                warn!(
                    target: telemetry_target::EDITOR,
                    folder = folder.as_str(),
                    "[editor] drop refused: {message}"
                );
                self.push_toast(NoticeLevel::Warning, format!("drop refused: {message}"));
                return;
            }
        };
        let destination = if sub_folder.as_os_str().is_empty() {
            asset_root.clone()
        } else {
            asset_root.join(&sub_folder)
        };
        // Step 2: Split the drop into files and folders, refusing a folder by
        // name.
        let mut sources = Vec::new();
        for path in paths {
            if path.is_dir() {
                info!(
                    target: telemetry_target::EDITOR,
                    path = %path.display(),
                    "[editor] drop refused: folder"
                );
                self.push_toast(
                    NoticeLevel::Warning,
                    format!(
                        "{} is a folder; drop its files instead",
                        path.file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.display().to_string())
                    ),
                );
                continue;
            }
            sources.push(path.display().to_string());
        }
        if sources.is_empty() {
            return;
        }
        // Step 3: Record one undoable import for every kept file.
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending_commands) = world.get_resource_mut::<PendingEditorCommands>() {
            pending_commands.push_recorded(
                vec![EditorCommand::ImportFiles {
                    sources: sources.clone(),
                    destination: destination.display().to_string(),
                }],
                "import files",
                None,
            );
        }
        self.asset_listing_dirty = true;
        info!(
            target: telemetry_target::EDITOR,
            count = sources.len(),
            folder = folder.as_str(),
            "[editor] files dropped"
        );
        self.push_toast(
            NoticeLevel::Note,
            format!("importing {} file(s) into {folder}", sources.len()),
        );
    }
}
