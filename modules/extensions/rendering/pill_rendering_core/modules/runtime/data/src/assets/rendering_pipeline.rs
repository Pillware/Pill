//! An ordered list of passes, as one asset.
//!
//! # Responsibilities
//!
//! - Hold the passes a frame runs, in the order the game wrote them, each with
//!   this chain's own switch for it ([`PipelinePass`]).
//! - Stay a list of handles rather than a copy of the passes, so one pass can
//!   appear in several pipelines and editing a pass does not duplicate it.
//!
//! # Design
//!
//! This asset is only the declaration. The renderer builds the chain from it -
//! ordering the passes, resolving the targets they name, creating the GPU
//! pipelines - because it owns the device. The game picks the pipeline by
//! setting it on `RenderingManager`, and the renderer builds the chain again
//! whenever that pipeline or the assets change.
//!
//! The asset is also a file of its own (`.pipeline`), so the editor can create
//! one and a project can load one by path. Its body names the passes it runs,
//! in the order they run, and each entry has an optional `enabled` key. That
//! lets one chain leave a pass out while another chain naming the same pass
//! still runs it. The pass's own `enabled` key cannot do that, because it
//! applies to every chain.
//!
//! Nothing else belongs here. A pass already carries the shader it draws with
//! and the values it reads, so a pipeline that also listed shaders and textures
//! would repeat them - and would be out of date as soon as a pass was edited.
//! What the built-in pipelines' `install` functions put in the store is their
//! own business; `AssetManager::handle_by_name` finds an asset by name.

// External crates
use pill_engine::{
    asset, pill_mirror_object, AssetLoadResult, AssetManager, AssetReference, Handle, PillMirror,
    StandaloneAsset,
};
use serde::{Deserialize, Serialize};

// Current crate
use crate::RenderPass;

// =============================================================================
// PipelinePass
// =============================================================================

/// One pass in a pipeline, with this chain's own on/off switch for it.
///
/// The switch belongs to the entry, not to the pass asset: the same
/// `.render_pass` can appear in two chains, and a project may want bloom in one
/// and not the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PillMirror)]
pub struct PipelinePass {
    /// The pass this entry runs.
    pub pass: Handle<RenderPass>,
    /// Whether this chain runs the pass. A `.pipeline` file writes the switch
    /// for each entry; a chain built in code runs every pass it adds.
    pub enabled: bool,
}

impl PipelinePass {
    /// Builds an entry that runs `pass`, switched on.
    pub fn new(pass: Handle<RenderPass>) -> Self {
        Self {
            pass,
            enabled: true,
        }
    }
}

// =============================================================================
// RenderingPipeline
// =============================================================================

/// The passes a frame runs, in the order they were added.
#[asset(standalone)]
#[pill_mirror_object(asset, shared = "pill_master_renderer::RenderingPipeline")]
#[derive(Clone, Debug)]
pub struct RenderingPipeline {
    /// The passes, in the order they run. The renderer sorts the chain by each
    /// pass's own `order` value; the order here breaks ties.
    pub passes: Vec<PipelinePass>,
}

impl Default for RenderingPipeline {
    /// An empty chain, the same value `new` builds.
    fn default() -> Self {
        Self { passes: Vec::new() }
    }
}

impl RenderingPipeline {
    /// An empty pipeline.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one pass to the chain, switched on.
    pub fn add(&mut self, pass: Handle<RenderPass>) {
        self.passes.push(PipelinePass::new(pass));
    }

    /// Appends one pass and returns the chain, so calls can be chained.
    pub fn with_pass(mut self, pass: Handle<RenderPass>) -> Self {
        self.passes.push(PipelinePass::new(pass));
        self
    }
}

// =============================================================================
// PipelinePassDocument
// =============================================================================

/// One entry in a `.pipeline` file: the pass it names, and whether this chain
/// runs it.
///
/// The pass is stored as a reference, so a chain survives a pass being moved or
/// renamed, and one pass can appear in several chains. The reference's fields
/// are written beside `enabled` instead of under a key of their own, so an
/// entry reads `{ "guid": …, "enabled": false }`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PipelinePassDocument {
    /// The pass this entry runs, by guid.
    #[serde(flatten)]
    pub pass: AssetReference<RenderPass>,
    /// Whether this chain runs the pass. A file that leaves the key out runs
    /// it, so a file written before the key existed still draws what it always
    /// did.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

impl Default for PipelinePassDocument {
    /// An entry that names no pass, switched on.
    fn default() -> Self {
        Self {
            pass: AssetReference::unset(),
            enabled: enabled_by_default(),
        }
    }
}

// =============================================================================
// RenderingPipelineDocument
// =============================================================================

/// The pipeline as its file holds it. The passes are listed in the order they
/// run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderingPipelineDocument {
    /// The passes, in the order they run.
    pub passes: Vec<PipelinePassDocument>,
}

// The metadata type name defaults to the shared name above, so every binary's
// copy of `RenderingPipeline` reads the same files.
impl StandaloneAsset for RenderingPipeline {
    type Document = RenderingPipelineDocument;
    const FILE_EXTENSION: &'static str = "pipeline";

    fn from_document(
        _name: &str,
        document: RenderingPipelineDocument,
        assets: &AssetManager,
    ) -> AssetLoadResult<Self> {
        // A reference that resolves to nothing stays an invalid handle, the
        // same as in a material. The chain the renderer builds then skips that
        // pass instead of refusing the whole file.
        Ok(Self {
            passes: document
                .passes
                .iter()
                .map(|entry| PipelinePass {
                    pass: entry.pass.resolve(assets),
                    enabled: entry.enabled,
                })
                .collect(),
        })
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The default for `enabled`, which is on. The serde attribute and [`Default`]
/// both name this function, so the two cannot disagree.
fn enabled_by_default() -> bool {
    true
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_keep_the_order_they_were_added() {
        let first = Handle::from_raw(0, 1);
        let second = Handle::from_raw(1, 1);
        let mut pipeline = RenderingPipeline::new().with_pass(first);

        pipeline.add(second);

        assert_eq!(
            pipeline
                .passes
                .iter()
                .map(|entry| entry.pass)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
    }

    #[test]
    fn an_empty_pipeline_has_no_passes() {
        assert!(RenderingPipeline::new().passes.is_empty());
    }

    /// A chain built in code runs every pass it adds. Only a file can switch an
    /// entry off.
    #[test]
    fn an_added_pass_is_on_unless_the_file_says_otherwise() {
        let pass = Handle::from_raw(0, 1);

        let pipeline = RenderingPipeline::new().with_pass(pass);

        assert!(pipeline.passes[0].enabled);
    }

    /// The entry's key is optional. Leaving it out runs the pass, so a
    /// pipeline file written before the key existed still draws what it always
    /// did.
    #[test]
    fn a_document_entry_reads_the_enabled_key_and_defaults_it_on() {
        let omitted: PipelinePassDocument =
            serde_json::from_str(r#"{ "guid": "0f5e2c7a9b1d34c8a6e40b52d7c19f83" }"#)
                .expect("an entry without the key parses");
        let off: PipelinePassDocument = serde_json::from_str(
            r#"{ "guid": "0f5e2c7a9b1d34c8a6e40b52d7c19f83", "enabled": false }"#,
        )
        .expect("an entry with the key parses");

        assert!(omitted.enabled);
        assert!(!off.enabled);
    }

    /// A chain's file names its passes by guid. Loading the document turns them
    /// back into handles, in the order the file lists them.
    #[test]
    fn a_document_resolves_its_passes_in_order() {
        let mut assets = AssetManager::new();
        let first = assets
            .add_named_with_guid(
                "first",
                pill_engine::AssetGuid::from_name("first"),
                RenderPass::new("first"),
            )
            .expect("a free name");
        let second = assets
            .add_named_with_guid(
                "second",
                pill_engine::AssetGuid::from_name("second"),
                RenderPass::new("second"),
            )
            .expect("a free name");

        let document = RenderingPipelineDocument {
            passes: vec![
                PipelinePassDocument {
                    pass: AssetReference::from_handle(&assets, second)
                        .expect("a guid to name it by"),
                    enabled: true,
                },
                PipelinePassDocument {
                    pass: AssetReference::from_handle(&assets, first)
                        .expect("a guid to name it by"),
                    enabled: false,
                },
            ],
        };
        let pipeline =
            RenderingPipeline::from_document("pipelines/main.pipeline", document, &assets)
                .expect("the passes resolve");

        assert_eq!(
            pipeline
                .passes
                .iter()
                .map(|entry| entry.pass)
                .collect::<Vec<_>>(),
            vec![second, first]
        );
        assert!(pipeline.passes[0].enabled);
        assert!(
            !pipeline.passes[1].enabled,
            "the entry's own switch reaches the loaded chain"
        );
    }
}
