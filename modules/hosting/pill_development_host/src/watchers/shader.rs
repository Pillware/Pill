//! Development shader reload, host half: watch a shader source tree, re-cook
//! edits, and queue the results.
//!
//! # Responsibilities
//!
//! - Watch one tree for HLSL edits, wait for a burst of events to stop
//!   (debounce), and then cook the whole tree with
//!   [`pill_assets::cook_shader_tree`]. That is the same routine the data
//!   crate's build script runs, so both produce the same outputs.
//! - Queue every output whose text changed as a [`CookedShader`] for the main
//!   thread, with both the output's and its source's path.
//!
//! # Design
//!
//! Two trees are watched with this worker: the renderer data crate's
//! `shader_sources/`, whose outputs the data crate's own export applies to the
//! shaders built from that tree, and the project's `res`, whose outputs are
//! read by the shader asset's import and therefore only need the asset to be
//! reimported.
//!
//! The worker thread runs only host code and `slangc`, never inside a module
//! image, so a data crate reload can swap the image at any time. The main
//! thread hands the queue over at the frame boundary, after the reloads, using
//! the export of the data generation that is current at that moment.
//!
//! Cooking writes only stale outputs, so editing a header makes every source
//! stale again. A cook that fails is logged and queues nothing, so the shaders
//! already loaded keep drawing. What gets queued is every output whose text
//! differs from the last text the worker saw for it, not only the outputs this
//! cook rebuilt. The data crate's build script runs the same cook, so it can
//! reach an edited source first; the worker then finds nothing stale, yet the
//! edit still has to reach the running world. Only `.hlsl` events count. The
//! cooked `.wgsl` files sit beside the sources and would otherwise feed back
//! in as edits.

// Standard library
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

// External crates
use pill_core::error::WatcherError;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::{info, warn};

// Current crate
use super::shared::{is_relevant_event, spawn_settled_watcher};

// =============================================================================
// Constants
// =============================================================================

/// Extension of the source files whose edits start a cook.
const SOURCE_EXTENSION: &str = "hlsl";

// =============================================================================
// Types
// =============================================================================

/// One re-cooked WGSL file, waiting for the main thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CookedShader {
    /// The source relative to the watched tree, using `/` as the separator.
    ///
    /// The shader asset's import reads the file beside this one, so a project
    /// shader is reimported by this path.
    pub(crate) source_relative_path: String,
    /// The file relative to the watched `shader_sources/` tree, using `/` as
    /// the separator.
    pub(crate) relative_path: String,
    /// The file's new text.
    pub(crate) wgsl: String,
}

/// A running shader watcher. Dropping it stops the worker the next time it
/// has a shader to queue.
pub(crate) struct ShaderWatcher {
    /// The re-cooked files the worker has queued, oldest first.
    cooked_shaders: Receiver<CookedShader>,
}

impl ShaderWatcher {
    /// Starts a worker thread that watches `shaders_directory` and cooks the
    /// tree when an edit settles. `tree_label` names the tree in the log line
    /// that announces the watch - the data crate's name, or the project.
    ///
    /// Returns `Ok(None)` when the directory does not exist: a tree with no
    /// shaders has nothing to reload.
    ///
    /// # Errors
    ///
    /// Returns a [`WatcherError`] when the watcher or its worker thread cannot
    /// be created, or when the directory cannot be registered with the
    /// watcher.
    pub(crate) fn spawn(
        tree_label: &str,
        shaders_directory: PathBuf,
    ) -> Result<Option<Self>, WatcherError> {
        if !shaders_directory.is_dir() {
            return Ok(None);
        }

        // Step 1: Open the queue that carries cooked files to the main
        // thread. It starts with the text the outputs have on disk now, so a
        // file is queued only once its text differs from that.
        let (cooked_sender, cooked_shaders) = channel::<CookedShader>();
        info!(
            target: telemetry_target::HOT_RELOAD,
            tree = tree_label,
            directory = %shaders_directory.display(),
            "watching shader sources for edits"
        );
        let mut known_outputs = read_output_texts(&shaders_directory);

        // Step 2: Start the shared watcher on the tree. It accepts only
        // `.hlsl` files, and the last closure cooks the tree once per batch of
        // edits and queues the outputs whose text changed.
        spawn_settled_watcher(
            shaders_directory,
            "pill shader watcher",
            "shader watcher error",
            is_relevant_event,
            is_shader_source,
            move |root, edited_paths| {
                for shader in cook_edited_shaders(root, &edited_paths, &mut known_outputs) {
                    if cooked_sender.send(shader).is_err() {
                        // The host dropped the watcher, so stop the worker.
                        return false;
                    }
                }
                true
            },
        )?;

        Ok(Some(Self { cooked_shaders }))
    }

    /// Returns every re-cooked file queued since the last call, oldest first.
    pub(crate) fn drain(&self) -> Vec<CookedShader> {
        self.cooked_shaders.try_iter().collect()
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Whether an edited file is one a cook reads.
///
/// Headers count as sources here: a header under an `include/` directory
/// changes what every source that includes it cooks to, so an edit to one has
/// to start a cook just as a stage source's edit does. The cooked `.wgsl`
/// files are excluded, or the cook's own output would feed back in as an edit.
fn is_shader_source(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == SOURCE_EXTENSION)
}

/// Returns the text of every cooked `.wgsl` file under `shaders_directory`,
/// keyed by path.///
/// An output that cannot be read is left out, so the next cook queues it as
/// changed.
fn read_output_texts(shaders_directory: &Path) -> HashMap<PathBuf, String> {
    let Ok(layout) = pill_assets::shader_tree_layout(shaders_directory) else {
        return HashMap::new();
    };
    let mut output_texts_by_path = HashMap::new();
    for source_directory in &layout.source_directories {
        let Ok(entries) = std::fs::read_dir(source_directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "wgsl")
            {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    output_texts_by_path.insert(path, text);
                }
            }
        }
    }
    output_texts_by_path
}

/// Cooks the tree after the edits in `edited_paths` settled, and returns every
/// output whose text differs from `known_outputs`.
///
/// The texts that are returned are also recorded in `known_outputs`, so the
/// same text is not queued twice. `edited_paths` is the batch of files whose
/// events started the cook; it is only used for the count in the log line.
///
/// Every discovered source is compared, not only the outputs this cook
/// rebuilt. The data crate's build script cooks the same tree, so it can reach
/// an edited source first; the edit still has to reach the running world.
///
/// A cook that fails is logged and returns nothing, so the running shaders
/// keep the text they have.
fn cook_edited_shaders(
    shaders_directory: &Path,
    edited_paths: &[PathBuf],
    known_outputs: &mut HashMap<PathBuf, String>,
) -> Vec<CookedShader> {
    // Step 1: Cook the tree. A failed cook queues nothing, so the shaders
    // already loaded keep drawing.
    let cook_started_at = Instant::now();
    let report = match pill_assets::cook_shader_tree(shaders_directory) {
        Ok(report) => report,
        Err(error) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "shader cook failed; keeping the previous shaders: {error}"
            );
            return Vec::new();
        }
    };

    // Step 2: Queue every output whose text changed, in discovery order.
    let mut changed_shaders = Vec::new();
    for source in &report.discovered_sources {
        let output_path = source.with_extension("wgsl");
        let Some(relative_path) = relative_path_from_root(shaders_directory, &output_path) else {
            continue;
        };
        let Some(source_relative_path) = relative_path_from_root(shaders_directory, source) else {
            continue;
        };
        let wgsl_text = match std::fs::read_to_string(&output_path) {
            Ok(wgsl_text) => wgsl_text,
            Err(error) => {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    path = %output_path.display(),
                    "cooked shader could not be read: {error}"
                );
                continue;
            }
        };
        if known_outputs.get(&output_path) == Some(&wgsl_text) {
            continue;
        }
        known_outputs.insert(output_path, wgsl_text.clone());
        changed_shaders.push(CookedShader {
            source_relative_path,
            relative_path,
            wgsl: wgsl_text,
        });
    }
    info!(
        target: telemetry_target::HOT_RELOAD,
        edited = edited_paths.len(),
        recooked = report.rebuilt_outputs.len(),
        changed = changed_shaders.len(),
        cook_ms = cook_started_at.elapsed().as_millis() as u64,
        "shader sources re-cooked"
    );
    changed_shaders
}

/// Returns `path` relative to `root`, using `/` as the separator, in the form
/// the data crate's shader records use.
///
/// Returns `None` when `path` is outside `root`.
fn relative_path_from_root(root: &Path, path: &Path) -> Option<String> {
    let relative_path = path.strip_prefix(root).ok()?;
    let path_components: Vec<String> = relative_path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(path_components.join("/"))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_uses_forward_slashes() {
        let root = Path::new("crate").join("shaders");
        let output = root
            .join("pbr_pipeline")
            .join("shaders")
            .join("pbr_fragment.wgsl");

        assert_eq!(
            relative_path_from_root(&root, &output).as_deref(),
            Some("pbr_pipeline/shaders/pbr_fragment.wgsl")
        );
        assert_eq!(
            relative_path_from_root(&root, Path::new("elsewhere.wgsl")),
            None
        );
    }

    #[test]
    fn a_header_edit_counts_as_a_source_edit() {
        let root = Path::new("res").join("shaders");

        assert!(is_shader_source(&root.join("unlit_fragment.hlsl")));
        assert!(is_shader_source(&root.join("include").join("common.hlsl")));
        assert!(!is_shader_source(&root.join("unlit_fragment.wgsl")));
        assert!(!is_shader_source(&root.join("unlit_fragment.hlsl.meta")));
    }

    #[test]
    fn a_missing_directory_starts_no_watcher() {
        let missing = std::env::temp_dir().join("pill_host_shader_watcher_missing_directory");

        let watcher = ShaderWatcher::spawn("test", missing).expect("not an error");

        assert!(watcher.is_none());
    }
}
