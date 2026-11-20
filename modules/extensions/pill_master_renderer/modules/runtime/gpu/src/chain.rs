//! The chain's GPU objects, and the plan each frame is recorded from.
//!
//! # Responsibilities
//!
//! - Decide when a chain's pipelines, bind groups and offscreen targets must be
//!   rebuilt. A tuple of the frame's chain generation, resource epoch and
//!   surface size is checked every frame; the chain itself and the offscreen
//!   layout are compared only when that tuple changes.
//! - Build one pass object per pass of the chain, and remember the reason for
//!   each pass it could not build.
//! - Plan each frame: which draws each pass takes, and which pass opens each
//!   target. A pass that cannot be recorded is named once, not once per frame.
//! - Write the chain to the log once per change.
//!
//! # Design
//!
//! The frame hands this module a resolved chain every frame, but the GPU objects
//! are built once and reused: an unchanged frame costs one tuple comparison.
//!
//! A game writes its chain as a [`RenderingPipeline`] of [`RenderPass`]es. The
//! frame resolves that into [`ResolvedPass`]es, and this module turns each of
//! them into a GPU pass object. A [`ChainContext`] carries what the chain needs
//! from the renderer, and one pass build yields either that object or the reason
//! there is none.
//!
//! [`RenderingPipeline`]: crate::RenderingPipeline
//! [`RenderPass`]: crate::RenderPass

// Standard library
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

// External crates
use pill_core::telemetry::telemetry_target;
use pill_core::{debug, info, warn};
use pill_engine::AssetManager;

// Current crate
use crate::{
    assets::{asset_key, Shader},
    frame::{PassKind, PassTarget, ResolvedPass},
    render_queue::RenderQueueItem,
    renderer::{State, OFFSCREEN_FORMAT},
    rendering_resources_manager::GpuColumn,
    resources::{RendererPass, RendererShaderHandle, RendererTexture, RendererTextureHandle},
};

// =============================================================================
// Pass Output
// =============================================================================

/// Where a pass writes.
#[derive(Clone, Copy)]
pub(crate) enum PassOutput<'a> {
    /// The frame's own surface, which is the image the caller presents.
    Surface,
    /// An offscreen colour target, by the name the chain gave it.
    Offscreen(&'a str),
}

// =============================================================================
// Pass Slot
// =============================================================================

/// A pass's GPU object, or the reason there is none.
pub(crate) enum PassSlot {
    /// Built and ready to record: a pipeline of its own, for the target it
    /// writes.
    Drawable(Box<RendererPass>),
    /// A geometry pass with no shader of its own. Each instance draws through
    /// the pipeline its own material names, so nothing is built for the pass;
    /// this is what the built-in chain uses.
    Unshaded,
    /// Cannot be recorded, carrying the reason the log reports.
    Unsupported(String),
}

// =============================================================================
// Draw Range
// =============================================================================

/// One run of queue positions a geometry pass records, and which pipeline it
/// draws them through.
///
/// `own_pipeline` is true for the instances the pass's own shader shades. It is
/// false for the instances no pass in the chain claimed: those draw through the
/// pipeline their own material carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DrawRange {
    /// Positions in the frame's sorted queue (also instance indices).
    pub range: Range<u32>,
    /// Whether the pass's own pipeline draws this run.
    pub own_pipeline: bool,
}

// =============================================================================
// Pass Plan
// =============================================================================

/// One pass the renderer will record.
pub(crate) enum PassPlan<'a> {
    /// Draws instances, batched by material, into the pass's targets.
    ///
    /// The pass draws the queue items in `ranges`. A position there is a
    /// position in the frame's sorted queue, which is also the instance's
    /// position in the instance buffer, so nothing is copied. A pass that takes
    /// the whole queue - the built-in chain's shape - has one range; a pass
    /// filtered by shader has one range per run of that shader.
    ///
    /// The first pass that names a shader and writes only the surface also
    /// takes the runs no pass in the chain claimed, so an assigned material is
    /// never left off the screen.
    Geometry {
        label: &'a str,
        /// The frame's render queue, which `ranges` index into.
        queue: &'a [RenderQueueItem],
        ranges: Vec<DrawRange>,
        outputs: Vec<PassOutput<'a>>,
        /// Whether this pass opens the targets it writes.
        clear: bool,
        /// Index of the pass's own built object in the chain's pass list, when
        /// there is one. `None` leaves each instance to the pipeline its
        /// material's shader carries.
        pass_index: Option<usize>,
    },
    /// Three vertices and no vertex buffers, reading what earlier passes wrote.
    Fullscreen {
        label: &'a str,
        pass_index: usize,
        outputs: Vec<PassOutput<'a>>,
        /// Whether this pass opens the targets it writes.
        clear: bool,
        /// Whether the triangle is tested against the depth buffer: a skybox
        /// is, so it shows only where no mesh drew; a post-processing step is
        /// not.
        tests_depth: bool,
    },
}

impl PassPlan<'_> {
    /// Pass name, used as the wgpu render-pass label and in the chain log.
    pub(crate) fn label(&self) -> &str {
        match self {
            PassPlan::Geometry { label, .. } | PassPlan::Fullscreen { label, .. } => label,
        }
    }

    /// Where this pass writes, its own target first.
    pub(crate) fn outputs(&self) -> &[PassOutput<'_>] {
        match self {
            PassPlan::Geometry { outputs, .. } | PassPlan::Fullscreen { outputs, .. } => outputs,
        }
    }

    /// Whether this pass opens the targets it writes.
    pub(crate) fn clears(&self) -> bool {
        match self {
            PassPlan::Geometry { clear, .. } | PassPlan::Fullscreen { clear, .. } => *clear,
        }
    }

    /// How many instances this pass draws. A fullscreen pass reports one, for
    /// its triangle.
    pub(crate) fn draws(&self) -> usize {
        match self {
            PassPlan::Geometry { ranges, .. } => ranges
                .iter()
                .map(|draw| (draw.range.end - draw.range.start) as usize)
                .sum(),
            PassPlan::Fullscreen { .. } => 1,
        }
    }
}

// =============================================================================
// Chain Context
// =============================================================================

/// What the chain needs from the renderer to build its GPU objects.
///
/// The chain names its shaders and textures by asset key, and the renderer
/// caches their GPU objects under the same key, so the chain is handed the
/// caches instead of a way to look them up.
///
/// The fields travel as one struct because the chain is mutated while the
/// renderer's state is borrowed, and eight separate arguments at every call
/// site would stop the chain reading as one unit.
pub(crate) struct ChainContext<'a> {
    /// The device, caches and layouts the passes are built from.
    pub(crate) state: &'a mut State,
    /// The colour format a pass writing the surface declares.
    pub(crate) surface_format: wgpu::TextureFormat,
    /// The surface size, in pixels: every offscreen target is sized from it.
    pub(crate) size: (u32, u32),
    /// The depth buffer every geometry pass writes.
    pub(crate) depth_texture: &'a RendererTexture,
    /// The offscreen colour targets the chain declares, by name.
    pub(crate) offscreen: &'a mut HashMap<String, RendererTexture>,
    /// Shader objects by asset key: a pass names its shader by key.
    pub(crate) shader_handles: &'a GpuColumn<RendererShaderHandle>,
    /// Texture objects by asset key: a pass's committed textures.
    pub(crate) texture_handles: &'a GpuColumn<RendererTextureHandle>,
    /// Bumped whenever a shader or texture object is created, recreated, or
    /// dropped. It is part of both comparison keys, because a pass's bind
    /// groups reference those objects by handle.
    pub(crate) resource_epoch: u64,
}

// =============================================================================
// Offscreen Layout Key
// =============================================================================

/// What the offscreen targets were built for: the scale each pass asks for on
/// each target it names, and the surface size.
type OffscreenLayoutKey = (Vec<(String, u32)>, u32, u32);

// =============================================================================
// Gpu Chain
// =============================================================================

/// The GPU side of a chain: one pass object per pass, and the plan a frame is
/// recorded from.
///
/// The pass objects are rebuilt only when the chain, the resource epoch or the
/// surface size changes, or when [`invalidate`](Self::invalidate) is called. A
/// plan is asked for on every frame, and it borrows the frame it was made from,
/// so it never outlives that frame.
pub(crate) struct GpuChain {
    /// One entry per pass of the current chain, in the chain's order.
    passes: Vec<PassSlot>,
    /// The chain those passes were built from, with the resource epoch they
    /// were built at.
    ///
    /// Held only for comparison: [`ResolvedPass`] derives equality, so a field
    /// added to it is compared here without any change in this file, which a
    /// hand-built string could not promise.
    built_from: Option<(Vec<ResolvedPass>, u64)>,
    /// What `built_from` was last checked against: the frame's chain generation,
    /// the resource epoch and the surface size. One tuple comparison per frame,
    /// with no chain cloned or compared, until one of those three changes.
    frame_inputs: Option<(u64, u64, u32, u32)>,
    /// The offscreen layout the current targets were built for: each offscreen
    /// target a pass names, with that pass's scale, and the surface size.
    offscreen_key: Option<OffscreenLayoutKey>,
    /// Passes this chain cannot record yet, remembered so each one is named
    /// once in the log instead of once per frame.
    skipped_passes: HashSet<String>,
    /// The chain last written to the log, so a chain that does not change does
    /// not write a line per frame.
    chain_log: Option<String>,
}

impl GpuChain {
    /// A chain with nothing built and nothing planned.
    pub(crate) fn new() -> Self {
        Self {
            passes: Vec::new(),
            built_from: None,
            frame_inputs: None,
            offscreen_key: None,
            skipped_passes: HashSet::new(),
            chain_log: None,
        }
    }

    /// The built pass objects, in chain order, for the frame recorder to index.
    pub(crate) fn passes(&self) -> &[PassSlot] {
        &self.passes
    }

    /// Build the GPU objects the chain needs, when something they depend on
    /// changed.
    ///
    /// Two comparisons decide. The offscreen layout - every target and scale
    /// the chain declares, at the surface size - decides the render target
    /// textures. Everything else decides the pass pipelines and their bind
    /// groups: the chain itself, compared field by field, and the resource
    /// epoch.
    ///
    /// An unchanged frame costs one tuple comparison. The chain is cloned and
    /// compared only after the chain generation, the resource epoch or the
    /// surface size has changed.
    ///
    /// This never fails. A pass that would not build is stored as
    /// [`PassSlot::Unsupported`] and named in the log, so one bad shader does
    /// not cost the frame the passes that would have drawn.
    pub(crate) fn ensure(
        &mut self,
        chain: &[ResolvedPass],
        assets: &AssetManager,
        chain_generation: u64,
        context: ChainContext<'_>,
    ) {
        // Step 1: Stop here when nothing the chain depends on changed.
        let (width, height) = context.size;
        let frame_inputs = (chain_generation, context.resource_epoch, width, height);
        if self.frame_inputs == Some(frame_inputs) {
            return;
        }

        let offscreen_key = offscreen_layout(chain, width, height);
        let targets_changed = self.offscreen_key.as_ref() != Some(&offscreen_key);
        let built_from = (chain.to_vec(), context.resource_epoch);
        if !targets_changed && self.built_from.as_ref() == Some(&built_from) {
            self.frame_inputs = Some(frame_inputs);
            return;
        }

        // Step 2: Rebuild the offscreen targets when the layout or the surface
        // size changed.
        if targets_changed {
            ensure_offscreen_targets(
                &context.state.device,
                context.size,
                context.offscreen,
                chain,
            );
            self.offscreen_key = Some(offscreen_key);
        }

        // Step 3: Rebuild one pass object per pass. The slots the loop below
        // creates replace the old ones, so what was said about the old ones
        // goes with them.
        self.forget_passes();

        // A pass builds its own pipeline from the shader's sources, and the
        // chain names that shader by key while the asset store is walked by
        // handle. One pass over the shader column builds a map from key to
        // shader, so each pass looks its shader up without walking the store
        // again. Only a rebuild pays for it.
        let shaders_by_key: HashMap<u64, &Shader> = assets
            .iter_handles::<Shader>()
            .map(|(handle, shader)| (asset_key(handle), shader))
            .collect();

        // Passes are built in chain order and may only read targets an earlier
        // pass declares: the set grows as the chain is walked, so a pass that
        // reads its own output, or a later pass's, is refused instead of
        // binding whatever the map happens to hold.
        let mut defined_targets: HashSet<String> = HashSet::new();
        let mut passes: Vec<PassSlot> = Vec::with_capacity(chain.len());
        for pass in chain {
            passes.push(build_pass(
                pass,
                &shaders_by_key,
                &defined_targets,
                &context,
            ));
            for target in
                std::iter::once(&pass.settings.target).chain(pass.settings.extra_targets.iter())
            {
                if let PassTarget::Offscreen(name) = target {
                    defined_targets.insert(name.clone());
                }
            }
        }
        self.passes = passes;
        self.built_from = Some(built_from);
        self.frame_inputs = Some(frame_inputs);
    }

    /// Hand each pass the draws it was given.
    ///
    /// A geometry pass that names a shader draws the instances shaded by that
    /// shader and leaves the rest to the other passes. One that names no shader
    /// draws every instance, each with the pipeline its own material names,
    /// which is what the built-in chain does. The first pass that names a shader
    /// and writes only the surface also takes the instances no pass claimed.
    /// A fullscreen pass draws nothing but its triangle, and reads what earlier
    /// passes left in the targets it names.
    pub(crate) fn plan<'a>(
        &mut self,
        chain: &'a [ResolvedPass],
        render_queue: &'a [RenderQueueItem],
        shader_handles: &GpuColumn<RendererShaderHandle>,
    ) -> Vec<PassPlan<'a>> {
        let mut plan = Vec::with_capacity(chain.len());
        // Step 1: Collect the shaders this chain's geometry passes claim. A pass
        // that names a shader the renderer holds no handle for claims nothing.
        let mut claimed_shaders: std::collections::HashSet<RendererShaderHandle> =
            std::collections::HashSet::new();
        for pass in chain
            .iter()
            .filter(|pass| pass.settings.kind == PassKind::Geometry)
        {
            if let Some(key) = &pass.shader {
                if let Some(handle) = shader_handles.handle(key.0) {
                    claimed_shaders.insert(handle);
                }
            }
        }
        // Whether the runs no pass claimed have been handed to a pass yet. One
        // pass takes them, so nothing is drawn twice.
        let mut has_taken_unclaimed_runs = false;

        // Step 2: Build one plan entry per pass.
        for (index, pass) in chain.iter().enumerate() {
            // The pass's own target first, then the ones it writes beside it.
            let outputs: Vec<PassOutput<'_>> = std::iter::once(&pass.settings.target)
                .chain(pass.settings.extra_targets.iter())
                .map(|target| match target {
                    PassTarget::Surface => PassOutput::Surface,
                    PassTarget::Offscreen(name) => PassOutput::Offscreen(name.as_str()),
                })
                .collect();

            match pass.settings.kind {
                PassKind::Geometry => {
                    let reason = match self.passes.get(index) {
                        Some(PassSlot::Drawable(_)) => None,
                        Some(PassSlot::Unshaded) => None,
                        Some(PassSlot::Unsupported(reason)) => Some(reason.clone()),
                        _ => Some("it has no pipeline".to_owned()),
                    };
                    if let Some(reason) = reason {
                        self.report_skip(&pass.name, &reason);
                        continue;
                    }

                    let mut ranges: Vec<DrawRange> = match pass.shader {
                        // The pass draws the instances shaded by the shader it
                        // names, when that shader is still loaded. One that no
                        // longer is draws nothing: falling back to the default
                        // shader would hand this pass instances another pass
                        // already took.
                        Some(key) => match shader_handles.handle(key.0) {
                            Some(handle) => shader_ranges(render_queue, handle)
                                .into_iter()
                                .map(|range| DrawRange {
                                    range,
                                    own_pipeline: true,
                                })
                                .collect(),
                            None => Vec::new(),
                        },
                        // No shader named: the pass draws every instance, each
                        // with the pipeline its own material names. This is the
                        // built-in chain.
                        None => whole_queue(render_queue)
                            .into_iter()
                            .map(|range| DrawRange {
                                range,
                                own_pipeline: false,
                            })
                            .collect(),
                    };
                    // The instances no pass in the chain claimed are drawn
                    // here, through the pipeline of their own material, so an
                    // assigned material never disappears from the picture.
                    // Only a pass that writes the surface may take them: a
                    // material's pipeline is built for the surface's formats.
                    let writes_surface_only = outputs
                        .iter()
                        .all(|output| matches!(output, PassOutput::Surface));
                    if pass.shader.is_some() && !has_taken_unclaimed_runs && writes_surface_only {
                        let unclaimed = unclaimed_ranges(render_queue, &claimed_shaders);
                        if !unclaimed.is_empty() {
                            // Per frame, so it stays at debug: the chain's own
                            // line already reports the pass's draw count.
                            debug!(
                                target: telemetry_target::RENDERING,
                                pass = pass.name.as_str(),
                                instances = unclaimed
                                    .iter()
                                    .map(|range| (range.end - range.start) as usize)
                                    .sum::<usize>(),
                                "pass draws the instances no pass in the chain claimed"
                            );
                            ranges.extend(unclaimed.into_iter().map(|range| DrawRange {
                                range,
                                own_pipeline: false,
                            }));
                        }
                        has_taken_unclaimed_runs = true;
                    }
                    let pass_index = match self.passes.get(index) {
                        Some(PassSlot::Drawable(_)) => Some(index),
                        _ => None,
                    };
                    plan.push(PassPlan::Geometry {
                        label: pass.name.as_str(),
                        queue: render_queue,
                        ranges,
                        outputs,
                        clear: plan.is_empty(),
                        pass_index,
                    });
                }
                PassKind::Fullscreen | PassKind::Skybox => {
                    let reason = match self.passes.get(index) {
                        Some(PassSlot::Drawable(_)) => None,
                        Some(PassSlot::Unsupported(reason)) => Some(reason.clone()),
                        _ => Some("it has no fullscreen pipeline".to_owned()),
                    };
                    match reason {
                        None => plan.push(PassPlan::Fullscreen {
                            label: pass.name.as_str(),
                            pass_index: index,
                            outputs,
                            clear: plan.is_empty(),
                            tests_depth: pass.settings.kind == PassKind::Skybox,
                        }),
                        Some(reason) => self.report_skip(&pass.name, &reason),
                    }
                }
            }
        }

        // Step 3: A chain with nothing recordable in it still has to open the
        // frame's targets, because a swapchain image nobody cleared presents
        // whatever was in it.
        if plan.is_empty() {
            plan.push(PassPlan::Geometry {
                label: "frame.clear",
                queue: render_queue,
                ranges: Vec::new(),
                outputs: vec![PassOutput::Surface],
                clear: true,
                pass_index: None,
            });
        }
        plan
    }

    /// Forget the pass objects and what they were built from, so the next frame
    /// rebuilds them.
    ///
    /// The offscreen targets are kept on purpose: they depend on the chain's
    /// layout and the surface size, and an asset edit - which is what calls
    /// this - changes neither. Use [`invalidate_targets`](Self::invalidate_targets)
    /// when the surface size changed.
    pub(crate) fn invalidate(&mut self) {
        self.forget_passes();
    }

    /// Drop the built passes, the keys they were built from, and what was said
    /// about them.
    ///
    /// The skip set keeps the log quiet: a pass that stays broken is named once,
    /// not once per frame. It therefore has to be cleared together with the
    /// slots it describes, or a pass that breaks, is fixed and breaks again
    /// would stay silent. Every path that replaces the slots goes through here.
    fn forget_passes(&mut self) {
        self.passes.clear();
        self.built_from = None;
        self.frame_inputs = None;
        self.skipped_passes.clear();
    }

    /// Forget the offscreen layout as well, so the targets are rebuilt too.
    ///
    /// Use this when the surface size changed: every target is built at the
    /// surface size, so a new size makes all of them wrong.
    pub(crate) fn invalidate_targets(&mut self) {
        self.invalidate();
        self.offscreen_key = None;
    }

    /// Write the chain to the log, once per change.
    ///
    /// Each pass is named with the number of draws it was handed. That number
    /// is what shows whether the chain runs the passes the game asked for, or
    /// quietly falls back to the built-in pass. A line per frame would bury
    /// everything else, and an unchanged chain is the normal case.
    pub(crate) fn log(&mut self, plan: &[PassPlan]) {
        let mut chain_signature = String::new();
        for entry in plan {
            if !chain_signature.is_empty() {
                chain_signature.push(' ');
            }
            chain_signature.push_str(entry.label());
            chain_signature.push('(');
            chain_signature.push_str(&entry.draws().to_string());
            chain_signature.push(')');
        }
        if self.chain_log.as_deref() == Some(chain_signature.as_str()) {
            return;
        }

        // Logged once per change, like the renderer's other diagnostics: this
        // line is what a reader has when the window shows the wrong thing.
        info!(target: telemetry_target::RENDERING, "pass chain: {chain_signature}");
        self.chain_log = Some(chain_signature);
    }

    /// Name a pass the renderer cannot record, once rather than once per frame.
    fn report_skip(&mut self, pass_name: &str, reason: &str) {
        if !self.skipped_passes.insert(pass_name.to_owned()) {
            return;
        }

        // Logged once per change, like the renderer's other diagnostics.
        warn!(
            target: telemetry_target::RENDERING,
            pass = pass_name,
            error = reason,
            "pass not drawn; the chain runs without it"
        );
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Why a pass cannot write a target scaled down from the surface, or `None`
/// when it can.
///
/// A render pass's attachments must all be one size. The depth buffer is the
/// surface's, and a geometry pass writes it while a skybox reads it, so neither
/// can pair it with a smaller colour target. A fullscreen pass attaches no
/// depth buffer, so it may write a scaled target.
fn scaled_target_refusal(pass: &ResolvedPass) -> Option<String> {
    if pass.settings.target_scale.max(1) == 1 {
        return None;
    }
    let depth_use = match pass.settings.kind {
        PassKind::Geometry => "writes",
        PassKind::Skybox => "reads",
        PassKind::Fullscreen => return None,
    };
    Some(format!(
        "it {depth_use} the full-size depth buffer, so it cannot write a target scaled by {}",
        pass.settings.target_scale
    ))
}

/// Why a pass with no shader of its own cannot draw into `pass`'s targets, or
/// `None` when it can.
///
/// A geometry pass without a shader draws through each material's pipeline, and
/// those pipelines are built for the surface's formats. A target that is not
/// the surface would be recorded against the wrong format, so the pass is
/// refused while the chain is built, instead of every draw failing.
fn shaderless_target_refusal(pass: &ResolvedPass) -> Option<String> {
    if pass.settings.kind != PassKind::Geometry || pass.shader.is_some() {
        return None;
    }
    // The first target that is not the surface, if any: that one has no
    // material pipeline behind it.
    std::iter::once(&pass.settings.target)
        .chain(pass.settings.extra_targets.iter())
        .find_map(|target| match target {
            PassTarget::Surface => None,
            PassTarget::Offscreen(name) => Some(format!(
                "it names no shader, so it draws through the material pipelines built for the surface, and target '{name}' is not the surface"
            )),
        })
}

/// One pass's GPU object, or the reason there is none.
///
/// `defined_targets` is what earlier passes of the chain declared so far, and
/// the pass may only read those. `context` is what the renderer provides for
/// building GPU objects.
fn build_pass(
    pass: &ResolvedPass,
    shaders_by_key: &HashMap<u64, &Shader>,
    defined_targets: &HashSet<String>,
    context: &ChainContext<'_>,
) -> PassSlot {
    // Step 1: Refuse the pass when its targets cannot work.
    // The depth buffer is the surface's size, so a pass that uses it cannot
    // write a smaller target. Checked before the shader lookup: a geometry pass
    // with no shader of its own attaches the same buffer.
    if let Some(reason) = scaled_target_refusal(pass) {
        return PassSlot::Unsupported(reason);
    }
    // A geometry pass with no shader of its own draws through each material's
    // pipeline, and those are built for the surface's formats.
    if let Some(reason) = shaderless_target_refusal(pass) {
        return PassSlot::Unsupported(reason);
    }
    // Step 2: Resolve the shader the pass draws with. A geometry pass with no
    // shader of its own is the only kind that needs no object built here: it
    // draws through each material's pipeline.
    let Some(shader_key) = pass.shader else {
        return match pass.settings.kind {
            PassKind::Geometry => PassSlot::Unshaded,
            PassKind::Fullscreen => {
                PassSlot::Unsupported("it names no shader the renderer loaded".to_owned())
            }
            // A sky comes from a material; with none, there is nothing to draw.
            PassKind::Skybox => PassSlot::Unsupported(
                "it names no material (or shader) the renderer loaded".to_owned(),
            ),
        };
    };

    let Some(shader) = shaders_by_key.get(&shader_key.0).copied() else {
        return PassSlot::Unsupported("it names no shader the renderer loaded".to_owned());
    };
    let Some(renderer_shader) = context
        .shader_handles
        .handle(shader_key.0)
        .and_then(|handle| context.state.renderer_resource_storage.shaders.get(handle))
    else {
        return PassSlot::Unsupported("its shader has no pipeline".to_owned());
    };
    // Step 3: Collect the format of each target and the textures the pass
    // binds. One format per target the pass writes, in the order the shader's
    // `SV_TARGET` list names them. A target the pass writes beside its own is
    // what a geometry pass leaves a normal buffer in.
    let target_formats: Vec<wgpu::TextureFormat> = std::iter::once(&pass.settings.target)
        .chain(pass.settings.extra_targets.iter())
        .map(|target| match target {
            PassTarget::Surface => context.surface_format,
            PassTarget::Offscreen(name) => context
                .offscreen
                .get(name)
                .map(|texture| texture.texture.format())
                .unwrap_or(OFFSCREEN_FORMAT),
        })
        .collect();

    // The pass's own committed textures, by slot. They resolve to the same GPU
    // handles a material's textures do, so a pass and a material reach one
    // texture by one key. A slot whose texture is not loaded falls through to
    // the binder's default, which the log names rather than leaving a pass
    // quietly showing the wrong map.
    let mut textures: Vec<(String, RendererTextureHandle)> = Vec::new();
    for (slot, key) in &pass.textures {
        match context.texture_handles.handle(key.0) {
            Some(handle) => textures.push((slot.clone(), handle)),
            None => warn!(
                target: telemetry_target::RENDERING,
                pass = pass.name.as_str(),
                slot = %slot,
                "pass binds a texture that is not loaded; the shader's default is used"
            ),
        }
    }

    // Step 4: Build the pass object.
    let storage = &context.state.renderer_resource_storage;
    match RendererPass::new(
        &context.state.device,
        &context.state.queue,
        storage,
        &storage.engine_parameters.bind_group_layout,
        &context.state.camera_bind_group_layout,
        pass,
        shader,
        renderer_shader,
        &target_formats,
        context.state.depth_format,
        context.offscreen,
        context.depth_texture,
        &textures,
        defined_targets,
    ) {
        Ok(value) => PassSlot::Drawable(Box::new(value)),
        Err(error) => PassSlot::Unsupported(error.to_string()),
    }
}

/// The whole queue as one range, or none when the queue is empty.
fn whole_queue(render_queue: &[RenderQueueItem]) -> Vec<Range<u32>> {
    if render_queue.is_empty() {
        Vec::new()
    } else {
        std::iter::once(0..render_queue.len() as u32).collect()
    }
}

/// The runs of the queue whose shader no pass in `claimed_shaders` names.
///
/// These are the instances that a material queued and no geometry pass draws.
/// Neighbouring items join one run even when their shaders differ, so a run is a
/// stretch of the queue, not one shader's items.
fn unclaimed_ranges(
    render_queue: &[RenderQueueItem],
    claimed_shaders: &std::collections::HashSet<RendererShaderHandle>,
) -> Vec<Range<u32>> {
    let mut ranges: Vec<Range<u32>> = Vec::new();
    for (position, item) in render_queue.iter().enumerate() {
        if claimed_shaders.contains(&item.shader) {
            continue;
        }
        let position = position as u32;
        match ranges.last_mut() {
            Some(run) if run.end == position => run.end += 1,
            _ => ranges.push(position..position + 1),
        }
    }
    ranges
}

/// The runs of the queue drawn by `shader`.
///
/// The queue is sorted by draw order first and shader second, so one shader's
/// items are one run per draw order they appear in: usually a single run.
fn shader_ranges(
    render_queue: &[RenderQueueItem],
    shader: RendererShaderHandle,
) -> Vec<Range<u32>> {
    let mut ranges: Vec<Range<u32>> = Vec::new();
    for (position, item) in render_queue.iter().enumerate() {
        if item.shader != shader {
            continue;
        }
        let position = position as u32;
        match ranges.last_mut() {
            // Adjacent to the run before it: extend that run.
            Some(run) if run.end == position => run.end += 1,
            _ => ranges.push(position..position + 1),
        }
    }
    ranges
}

/// The layout of the offscreen targets a chain needs: one entry per pass and
/// target it names, with that pass's scale, plus the surface size.
///
/// Kept apart from the chain itself on purpose: editing an asset changes the
/// chain (a pass's parameters are part of it) but changes none of this, so the
/// offscreen textures survive an asset edit.
fn offscreen_layout(chain: &[ResolvedPass], width: u32, height: u32) -> OffscreenLayoutKey {
    let mut targets = Vec::new();
    for pass in chain {
        let scale = pass.settings.target_scale.max(1);
        for target in
            std::iter::once(&pass.settings.target).chain(pass.settings.extra_targets.iter())
        {
            if let PassTarget::Offscreen(name) = target {
                targets.push((name.clone(), scale));
            }
        }
    }
    (targets, width, height)
}

/// Create a target for every offscreen output the chain declares.
///
/// The targets are rebuilt when the offscreen layout or the surface size
/// changes, and every one is sized from the surface. A target that kept an old
/// size would be sampled at a different scale from the pass that writes it, so
/// the picture would shrink with the window instead of resizing with it.
fn ensure_offscreen_targets(
    device: &wgpu::Device,
    (width, height): (u32, u32),
    offscreen_targets: &mut HashMap<String, RendererTexture>,
    chain: &[ResolvedPass],
) {
    offscreen_targets.clear();
    for pass in chain {
        // The pass's own target comes first, then the ones it writes beside it.
        // The size is the first naming pass's: a target belongs to the chain,
        // so the pass that names it first decides for all of them.
        let scale = pass.settings.target_scale.max(1);
        let written =
            std::iter::once(&pass.settings.target).chain(pass.settings.extra_targets.iter());
        for target in written {
            let PassTarget::Offscreen(name) = target else {
                continue;
            };
            if offscreen_targets.contains_key(name) {
                continue;
            }
            match RendererTexture::new_render_target(
                device,
                name,
                (width / scale).max(1),
                (height / scale).max(1),
                OFFSCREEN_FORMAT,
            ) {
                Ok(texture) => {
                    offscreen_targets.insert(name.clone(), texture);
                }
                // Reported instead of fatal: the passes that read it each say
                // they are not drawn, which is the same outcome their own
                // failure path would produce.
                Err(error) => {
                    warn!(
                        target: telemetry_target::RENDERING,
                        "offscreen target `{name}` was not created: {error}"
                    );
                }
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{RendererMaterialHandle, RendererMeshHandle};
    use pill_renderer_api::frame::PassSettings;
    use pill_renderer_api::{AssetKey, MaterialParameter};

    fn pass(kind: PassKind, target: PassTarget) -> ResolvedPass {
        ResolvedPass {
            name: "p".to_owned(),
            shader: None,
            settings: PassSettings {
                kind,
                target,
                ..PassSettings::default()
            },
            parameters: std::collections::BTreeMap::new(),
            inputs: std::collections::BTreeMap::new(),
            textures: std::collections::BTreeMap::new(),
        }
    }

    /// A pass that names a shader takes the runs no pass in the chain claims,
    /// and draws them through their own materials rather than its pipeline.
    ///
    /// The shader this pass names is not loaded here (the handle map is empty),
    /// so the pass claims nothing and the queue's shader stays unclaimed. That
    /// is what happens to a material whose shader the chain never mentions.
    #[test]
    fn a_pass_that_names_a_shader_takes_the_runs_no_pass_claims() {
        let mut naming = pass(PassKind::Geometry, PassTarget::Surface);
        naming.shader = Some(AssetKey(7));
        let chain = vec![naming];
        let queue = vec![item_of_shader(3), item_of_shader(3), item_of_shader(3)];
        let mut pipeline = GpuChain::new();
        pipeline.passes.push(PassSlot::Unshaded);

        let plan = pipeline.plan(&chain, &queue, &no_shaders());

        assert_eq!(plan.len(), 1);
        match &plan[0] {
            PassPlan::Geometry { ranges, .. } => {
                assert_eq!(ranges.len(), 1, "the whole queue is one unclaimed run");
                assert_eq!(ranges[0].range, 0..3);
                assert!(
                    !ranges[0].own_pipeline,
                    "an unclaimed run draws through its material's pipeline"
                );
            }
            PassPlan::Fullscreen { .. } => panic!("a geometry pass planned as fullscreen"),
        }
    }

    /// A pass whose targets include an offscreen one takes no unclaimed runs: a
    /// material's pipeline is built for the surface's formats.
    #[test]
    fn an_offscreen_pass_leaves_the_unclaimed_runs_alone() {
        let mut naming = pass(PassKind::Geometry, PassTarget::Offscreen("hdr".to_owned()));
        naming.shader = Some(AssetKey(7));
        let chain = vec![naming];
        let queue = vec![item_of_shader(3)];
        let mut pipeline = GpuChain::new();
        pipeline.passes.push(PassSlot::Unshaded);

        let plan = pipeline.plan(&chain, &queue, &no_shaders());

        match &plan[0] {
            PassPlan::Geometry { ranges, .. } => assert!(ranges.is_empty()),
            PassPlan::Fullscreen { .. } => panic!("a geometry pass planned as fullscreen"),
        }
    }

    /// The rebuild check compares the stored chain as a whole, so a field added
    /// to [`ResolvedPass`] is compared without this file changing - including
    /// the ones a hand-built string could forget.
    #[test]
    fn the_stored_chain_covers_every_field_a_pass_has() {
        let surface = pass(PassKind::Geometry, PassTarget::Surface);
        let offscreen = pass(PassKind::Geometry, PassTarget::Offscreen("hdr".to_owned()));
        let mut reading = pass(PassKind::Geometry, PassTarget::Surface);
        reading.inputs.insert("hdr".to_owned(), "hdr".to_owned());
        let mut parameterised = pass(PassKind::Geometry, PassTarget::Surface);
        parameterised
            .parameters
            .insert("roughness".to_owned(), MaterialParameter::Scalar(0.5));

        let base = (vec![surface.clone()], 1u64);

        assert_eq!(base, (vec![surface.clone()], 1));
        assert_ne!(base, (vec![offscreen], 1), "the target is part of it");
        assert_ne!(base, (vec![reading], 1), "the inputs are part of it");
        assert_ne!(
            base,
            (vec![parameterised], 1),
            "the parameters are part of it"
        );
        assert_ne!(base, (vec![surface], 2), "the resource epoch is part of it");
    }

    /// The offscreen layout ignores what a pass reads and what it is set to,
    /// because neither changes a target's size.
    #[test]
    fn the_offscreen_layout_counts_targets_scales_and_the_surface_size() {
        let mut surface = pass(PassKind::Geometry, PassTarget::Surface);
        surface
            .parameters
            .insert("roughness".to_owned(), MaterialParameter::Scalar(0.5));
        let hdr = pass(PassKind::Geometry, PassTarget::Offscreen("hdr".to_owned()));

        assert_eq!(offscreen_layout(&[surface.clone()], 1920, 1080).0, vec![]);
        assert_eq!(
            offscreen_layout(std::slice::from_ref(&hdr), 1920, 1080).0,
            vec![("hdr".to_owned(), 1)]
        );
        assert_ne!(
            offscreen_layout(std::slice::from_ref(&hdr), 1920, 1080),
            offscreen_layout(&[hdr], 1280, 720),
            "a surface size moves every target"
        );
    }

    #[test]
    fn a_chain_with_nothing_recordable_still_clears_the_frame() {
        // No pass object was built for this chain, which is what happens on the
        // first frame after a chain arrives. Even then the frame's targets have
        // to be opened, so the plan holds a clear pass instead of nothing.
        let chain = vec![pass(PassKind::Geometry, PassTarget::Surface)];
        let mut pipeline = GpuChain::new();

        let plan = pipeline.plan(&chain, &[], &no_shaders());

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].label(), "frame.clear");
        assert!(plan[0].clears());
        assert_eq!(plan[0].draws(), 0);
        assert_eq!(plan[0].outputs().len(), 1);
    }

    #[test]
    fn a_geometry_pass_with_no_shader_takes_the_whole_queue() {
        let chain = vec![pass(PassKind::Geometry, PassTarget::Surface)];
        let queue = vec![
            RenderQueueItem {
                key: 0,
                frame_instance_index: 0,
                ..item_of_shader(0)
            },
            RenderQueueItem {
                key: 0,
                frame_instance_index: 1,
                ..item_of_shader(0)
            },
        ];
        let mut pipeline = GpuChain::new();
        pipeline.passes.push(PassSlot::Unshaded);

        let plan = pipeline.plan(&chain, &queue, &no_shaders());

        assert_eq!(plan.len(), 1);
        match &plan[0] {
            PassPlan::Geometry {
                ranges, pass_index, ..
            } => {
                // The whole queue as one range: nothing is copied.
                assert_eq!(ranges.len(), 1);
                assert_eq!(ranges[0].range, 0..2);
                assert!(!ranges[0].own_pipeline);
                // No pass pipeline of its own: each instance draws through the
                // pipeline its material names.
                assert!(pass_index.is_none());
            }
            PassPlan::Fullscreen { .. } => panic!("a geometry pass planned as fullscreen"),
        }
    }

    /// A queue item drawn by the shader in slot `shader_index`; only the
    /// shader handle matters to the ranges.
    fn item_of_shader(shader_index: u8) -> RenderQueueItem {
        RenderQueueItem {
            key: u64::from(shader_index) << 48,
            frame_instance_index: 0,
            shader: shader_handle(shader_index),
            material: RendererMaterialHandle::new(0, std::num::NonZeroU32::new(1).unwrap()),
            mesh: RendererMeshHandle::new(0, std::num::NonZeroU32::new(1).unwrap()),
        }
    }

    /// The handle of the shader in slot `shader_index`.
    fn shader_handle(shader_index: u8) -> RendererShaderHandle {
        RendererShaderHandle::new(
            u32::from(shader_index),
            std::num::NonZeroU32::new(1).unwrap(),
        )
    }

    /// No test registers a shader, so the planner gets an empty column to look
    /// shader keys up in.
    fn no_shaders() -> GpuColumn<RendererShaderHandle> {
        GpuColumn::new()
    }

    /// The runs no pass in the chain claims hold the queue items whose shader
    /// no pass names, so an assigned material is drawn by whichever pass takes
    /// those runs.
    #[test]
    fn unclaimed_ranges_are_the_shaders_no_pass_names() {
        let queue = vec![
            item_of_shader(0),
            item_of_shader(0),
            item_of_shader(1),
            item_of_shader(2),
            item_of_shader(1),
        ];
        let claimed: std::collections::HashSet<RendererShaderHandle> =
            [shader_handle(0), shader_handle(1)].into_iter().collect();
        assert_eq!(unclaimed_ranges(&queue, &claimed), vec![3..4]);
        let nothing: std::collections::HashSet<RendererShaderHandle> =
            std::collections::HashSet::new();
        // Adjacent runs of unclaimed shaders join into one range.
        assert_eq!(unclaimed_ranges(&queue, &nothing), vec![0..5]);
        let everything: std::collections::HashSet<RendererShaderHandle> =
            [shader_handle(0), shader_handle(1), shader_handle(2)]
                .into_iter()
                .collect();
        assert_eq!(
            unclaimed_ranges(&queue, &everything),
            Vec::<Range<u32>>::new()
        );
    }

    #[test]
    fn a_shader_draws_each_run_of_its_items_and_nothing_else() {
        // Shader 2's items in two runs, as two draw orders would leave them.
        let queue = [1, 2, 2, 3, 2, 2, 2, 1].map(item_of_shader);

        assert_eq!(shader_ranges(&queue, shader_handle(2)), vec![1..3, 4..7]);
        assert_eq!(shader_ranges(&queue, shader_handle(1)), vec![0..1, 7..8]);
        assert_eq!(
            shader_ranges(&queue, shader_handle(9)),
            Vec::<Range<u32>>::new()
        );
    }

    #[test]
    fn a_pass_that_uses_the_shared_depth_buffer_refuses_a_scaled_target() {
        let scaled = |kind| {
            let mut pass = pass(kind, PassTarget::Offscreen("half".to_owned()));
            pass.settings.target_scale = 2;
            pass
        };

        assert!(
            scaled_target_refusal(&scaled(PassKind::Geometry)).is_some(),
            "a geometry pass writes the surface-sized depth buffer"
        );
        assert!(
            scaled_target_refusal(&scaled(PassKind::Skybox)).is_some(),
            "a skybox reads the surface-sized depth buffer"
        );
        assert!(
            scaled_target_refusal(&scaled(PassKind::Fullscreen)).is_none(),
            "a post-processing pass reads no depth, so a scaled target is its own business"
        );
        assert!(
            scaled_target_refusal(&pass(PassKind::Geometry, PassTarget::Surface)).is_none(),
            "a full-size target matches the depth buffer"
        );
    }

    #[test]
    fn an_empty_queue_draws_no_range() {
        assert!(whole_queue(&[]).is_empty());
        let ranges = whole_queue(&[item_of_shader(0)]);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], 0..1);
    }

    #[test]
    fn a_shaderless_geometry_pass_that_writes_offscreen_is_refused() {
        let offscreen = pass(PassKind::Geometry, PassTarget::Offscreen("hdr".to_owned()));
        let reason = shaderless_target_refusal(&offscreen).expect(
            "a material pipeline is built for the surface, so an offscreen target has none",
        );
        assert!(
            reason.contains("hdr"),
            "the reason names the target that has no pipeline: {reason}"
        );

        let mut extra = pass(PassKind::Geometry, PassTarget::Surface);
        extra
            .settings
            .extra_targets
            .push(PassTarget::Offscreen("normals".to_owned()));
        assert!(
            shaderless_target_refusal(&extra).is_some(),
            "a second target is offscreen even when the first is the surface"
        );
    }

    /// A pass that breaks, is fixed and breaks again has to be named twice: the
    /// dedupe exists so a still-broken pass is not named every frame, not so a
    /// new breakage is silent.
    #[test]
    fn a_pass_that_breaks_again_after_a_rebuild_is_named_again() {
        let mut pipeline = GpuChain::new();
        pipeline.report_skip("p", "it has no pipeline");
        assert!(pipeline.skipped_passes.contains("p"));

        // The rebuild replaces the passes the report described, so the report
        // goes with them.
        pipeline.invalidate();
        assert!(
            pipeline.skipped_passes.is_empty(),
            "the report outlived the pass it was about"
        );
        pipeline.report_skip("p", "it has no pipeline");
        assert!(
            pipeline.skipped_passes.contains("p"),
            "the second breakage is named"
        );
    }

    #[test]
    fn a_shaderless_geometry_pass_that_writes_only_the_surface_is_allowed() {
        assert!(
            shaderless_target_refusal(&pass(PassKind::Geometry, PassTarget::Surface)).is_none(),
            "the material pipelines are built for the surface format"
        );

        let mut shaded = pass(PassKind::Geometry, PassTarget::Offscreen("hdr".to_owned()));
        shaded.shader = Some(AssetKey(0));
        assert!(
            shaderless_target_refusal(&shaded).is_none(),
            "a pass with a shader of its own gets a pipeline for its targets"
        );
    }
}
