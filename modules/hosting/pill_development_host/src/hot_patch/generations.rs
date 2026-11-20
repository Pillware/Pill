//! A patched function's history, and the routes back through it.
//!
//! # Responsibilities
//!
//! - Record every generation of every function this session replaced.
//! - Expose that history as a list the host can print and tooling can read.
//! - Reinstall an earlier generation, or the artifact's own original code.
//! - Forget prologue records a reload has invalidated.
//!
//! # Design
//!
//! Undoing a patch is not one operation; how a generation was delivered decides
//! how it is undone. A generation installed into a slot is undone by writing one
//! pointer. A generation applied by overwriting the function's first bytes is
//! undone by writing those bytes back. The case is recorded with the generation
//! rather than read from the function's kind, because an annotated and an
//! un-annotated plain function share one kind and are undone in different ways.
//!
//! A reload makes every recorded prologue address invalid at once, in every
//! session. Each address points into an image the graveyard (where the reload
//! keeps retired images until it releases them) will unmap, and one module's
//! records include addresses inside the project image, so the records cannot be
//! cleared selectively. Dropping the whole set is what stops a rollback from
//! writing into a retired image.

// External crates
use pill_core::info;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_engine::Engine;

// Current crate
use super::{
    install_everywhere, reset_everywhere, source, HotPatchSession, LoadedPatch, PrologueRestore,
};
use crate::native_library::NativeLibrary;

// =============================================================================
// Types
// =============================================================================

/// One recorded generation of one patched function, and the route back to it.
///
/// The patch library that delivered the generation is never unloaded, unlike a
/// module image, which the reload can retire and release. A slot holds an
/// address inside that library and nothing moves those addresses, which is why
/// a slot rollback is a single pointer write rather than a rebuild.
pub(crate) struct Generation {
    /// Qualified name of the function this generation replaced.
    pub(super) function: String,
    /// One-based position in this function's history. Generation zero is the
    /// code the running artifact was built with; it has no entry here because it
    /// needs no patch library.
    pub(super) number: u32,
    /// Address to install to make this generation active again.
    pub(super) address: usize,
    /// The hot-patching route that delivered this generation. When the
    /// generation lives in a slot, rollback uses it to choose between the
    /// engine's registry and a dispatch slot.
    pub(super) kind: source::HotFunctionKind,
    /// The signature identity a system's slot checks before it accepts the
    /// address.
    pub(super) signature_hash: u64,
    /// The name the delivering route looks this function up by. It differs from
    /// [`Self::function`] for a method's dispatch slot, whose name omits the
    /// type; see `slot_lookup_name`.
    pub(super) lookup_name: String,
    /// Signature text a plain function's slot checks before accepting it.
    pub(super) signature: String,
    /// The bytes this generation overwrote in each artifact, when it was
    /// delivered by prologue patching. Empty when the generation was installed
    /// into a slot.
    pub(super) prologue_restores: Vec<PrologueRestore>,
    /// Whether a reload emptied [`Self::prologue_restores`] rather than the
    /// generation never having filled it.
    ///
    /// Once the list is empty the two cases look the same, but they need
    /// opposite answers: a slot-delivered generation rolls back through its
    /// slot, while one whose prologue addresses a reload invalidated cannot be
    /// rolled back at all.
    pub(super) prologue_history_dropped: bool,
    /// When this generation was installed, so a listing can report its age.
    pub(super) installed_at: Instant,
}

impl Generation {
    /// Returns whether this generation was delivered by overwriting code rather
    /// than by installing into a slot.
    ///
    /// It reads the saved bytes because the function's kind does not answer the
    /// question: an annotated and an un-annotated plain function share one kind
    /// and are undone in completely different ways.
    fn overwrote_prologues(&self) -> bool {
        !self.prologue_restores.is_empty()
    }
}

/// One entry in a function's patch history, for hosts and tooling.
#[derive(Debug, Clone)]
pub struct PatchGeneration {
    /// Qualified name of the patched function.
    pub function: String,
    /// One-based position in the history; the code the artifact was built with
    /// is generation zero and has no entry.
    pub number: u32,
    /// The address that makes this generation active.
    pub address: usize,
    /// Seconds since this generation was installed.
    pub age_seconds: f64,
}

// =============================================================================
// HotPatchSession
// =============================================================================

impl HotPatchSession {
    /// The number of the generation that is running for one function.
    ///
    /// Zero means the code the artifact was built with, which is also what a
    /// function that was never patched reports.
    pub(super) fn active_generation(&self, qualified: &str) -> u32 {
        self.active_generations.get(qualified).copied().unwrap_or(0)
    }

    /// The address and signature a refused fan-out puts its already-redirected
    /// copies back on.
    ///
    /// `None` means the function runs generation zero, so each copy goes back to
    /// its own body. A generation delivered by prologue patching also answers
    /// `None`: its address is the body the patch compiled, written over the
    /// function's first bytes, so it cannot be installed into a redirect slot.
    pub(super) fn previous_install(&self, qualified: &str) -> Option<(usize, &str)> {
        let active_generation_number = self.active_generations.get(qualified)?;
        let generation = self.generations.iter().find(|candidate| {
            candidate.function == qualified && candidate.number == *active_generation_number
        })?;
        if generation.overwrote_prologues() {
            return None;
        }
        Some((generation.address, generation.signature.as_str()))
    }

    /// Every generation recorded for every function this session has patched.
    pub(crate) fn generations(&self) -> Vec<PatchGeneration> {
        self.generations
            .iter()
            .map(|generation| PatchGeneration {
                function: generation.function.clone(),
                number: generation.number,
                address: generation.address,
                age_seconds: generation.installed_at.elapsed().as_secs_f64(),
            })
            .collect()
    }

    /// Forget the patch history a reload has invalidated.
    ///
    /// A reload replaces an artifact's image, so a prologue address recorded
    /// before the reload no longer names its function, and the freshly loaded
    /// copy carries no patch anyway. Writing the saved bytes back to a stale
    /// address would corrupt whatever now occupies it, so the record is dropped
    /// instead of kept.
    ///
    /// Slot-delivered generations keep their records but lose their active
    /// entry. The rebuilt artifact re-creates its slot from the new sources, so
    /// the baseline is whatever the artifact now ships, and a rollback must not
    /// install a body from a previous revision into it. Patching the function
    /// again makes the history reachable from that new baseline.
    pub(crate) fn forget_prologue_patches(&mut self) {
        // Step 1: Drop the prologue bytes that a reload has invalidated.
        let mut function_names: Vec<String> = Vec::new();
        for generation in &mut self.generations {
            if generation.overwrote_prologues() {
                generation.prologue_restores.clear();
                generation.prologue_history_dropped = true;
                if !function_names.contains(&generation.function) {
                    function_names.push(generation.function.clone());
                }
            }
        }
        // Step 2: Clear every active entry, not only the prologue-patched
        // functions'. The reload re-creates the slot from the rebuilt
        // artifact, so what runs now is the artifact's own body even for a
        // slot-delivered generation. An entry left in place would make
        // `active_generation` name a generation that is not running, and let a
        // rollback install a body from a previous revision into the fresh
        // image. The `Generation` records stay, because the listing and their
        // `prologue_history_dropped` refusal still describe what happened.
        self.active_generations.clear();

        if function_names.is_empty() {
            return;
        }

        // Logged at INFO because the reload overrides any prologue patch
        // without a word of its own: without this line, a developer who had
        // just rolled a patch back would see the rollback come undone and
        // find no reason in the log.
        info!(
            target: telemetry_target::HOT_RELOAD,
            functions = function_names.join(", ").as_str(),
            "reload rebuilt from source; live-patch history reset (the reloaded \
             image supersedes the prologue patches)"
        );
    }

    /// Whether this function has an active generation recorded. A rollback
    /// needs that entry before it can reach the history.
    pub(crate) fn knows_function(&self, qualified: &str) -> bool {
        self.active_generations.contains_key(qualified)
    }

    /// Reinstall an earlier generation of one function.
    ///
    /// Generation zero is the code the running artifact was built with; one and
    /// up are patches, in the order they were applied. A slot generation is one
    /// pointer write per artifact - no rebuild, no reload, nothing unloaded -
    /// which is the payoff of dispatching through a slot rather than
    /// overwriting a function's first bytes.
    ///
    /// # Errors
    ///
    /// Returns a message when the function was never patched in this session,
    /// when the generation does not exist, when a reload has invalidated the
    /// generation's prologue addresses, when no baseline code is recorded to
    /// restore, or when an artifact refuses an address. The active-generation
    /// entry is updated only on success, and a prologue path that fails partway
    /// leaves the copies it already rewrote in the state it put them.
    pub(crate) fn rollback(
        &mut self,
        engine: &mut Engine,
        targets: &[(&str, &NativeLibrary)],
        patches: &[LoadedPatch],
        qualified: &str,
        generation_number: u32,
    ) -> Result<(), String> {
        if !self.knows_function(qualified) {
            return Err(format!(
                "`{qualified}` has not been patched in this session, so it has \
                 no history to roll back to"
            ));
        }

        // Step 1: Decide which route undoes this function's delivery. That is
        // not the same question as what kind of function it is: an un-annotated
        // function has no slot to install into, so its generations were written
        // over the code itself.
        let delivered_by_prologue = self
            .generations
            .iter()
            .any(|candidate| candidate.function == qualified && candidate.overwrote_prologues());

        // Step 2: Reinstall the requested generation through that route.
        if generation_number == 0 {
            if delivered_by_prologue {
                self.restore_prologue_baseline(qualified)?;
            } else {
                self.restore_baseline(engine, targets, patches, qualified)?;
            }
        } else {
            let generation = self
                .generations
                .iter()
                .find(|candidate| {
                    candidate.function == qualified && candidate.number == generation_number
                })
                .ok_or_else(|| format!("`{qualified}` has no generation {generation_number}"))?;

            // Its addresses pointed into an image that a reload has since
            // replaced, so there is nothing left to re-aim. Saying so is the
            // point: the slot route below would refuse too, but for the wrong
            // reason - the generation records no signature, so the refusal
            // would blame the function's shape.
            if generation.prologue_history_dropped {
                return Err(format!(
                    "generation {generation_number} of `{qualified}` was written into the \
                     artifact's own code, and a reload has replaced that code \
                     since; it can no longer be rolled back to. The reloaded \
                     artifact already carries the current source"
                ));
            }

            if generation.overwrote_prologues() {
                // Re-aim every copy at this generation's body. The bytes each
                // call returns are discarded on purpose: they are the jump the
                // previous generation wrote, not the artifact's own code, and
                // saving them over the stored originals would lose the only way
                // back to generation zero.
                for restore in &generation.prologue_restores {
                    // SAFETY: the address was recorded from this artifact's
                    // inventory when the generation was installed, and its
                    // declaration was checked against the artifact's before that
                    // write. `patch_prologue` validates the function's extent
                    // again before writing. No system is executing: rollback
                    // runs between frames.
                    unsafe {
                        pill_engine::hot_patch::patch_prologue(restore.address, generation.address)
                    }
                    .map_err(|detail| format!("{}: {detail}", restore.artifact))?;
                }
            } else {
                match generation.kind {
                    source::HotFunctionKind::System => engine
                        .hot_patch(qualified, generation.address, generation.signature_hash)
                        .map_err(|error| error.to_string())?,
                    // The copy count is only used by a fresh patch, whose
                    // install line reports it; a rollback has no line to
                    // report it on.
                    source::HotFunctionKind::PlainFunction => {
                        // A rollback is refused the same way a fresh install
                        // is, so the copies it already reached are put back on
                        // the generation it was leaving.
                        let previous = self.previous_install(qualified);
                        install_everywhere(
                            targets,
                            patches,
                            &generation.lookup_name,
                            generation.address,
                            &generation.signature,
                            previous,
                        )?;
                    }
                }
            }
        }

        // Step 3: Record what is running now, and report it.
        self.active_generations
            .insert(qualified.to_string(), generation_number);
        info!(
            target: telemetry_target::HOT_RELOAD,
            function = qualified,
            generation = generation_number,
            "patch generation rolled back"
        );
        Ok(())
    }

    /// Put a prologue-patched function back to the code its artifact was built
    /// with.
    ///
    /// The bytes to write come from the first generation. Every later generation
    /// saved the jump its predecessor wrote, so restoring from the newest would
    /// put a patch back rather than remove one.
    fn restore_prologue_baseline(&self, qualified: &str) -> Result<(), String> {
        let first_generation = self
            .generations
            .iter()
            .filter(|candidate| candidate.function == qualified && candidate.overwrote_prologues())
            .min_by_key(|candidate| candidate.number)
            .ok_or_else(|| {
                format!(
                    "`{qualified}` has no saved prologue bytes; the artifact was reloaded \
                     after it was patched, which already restored its own code"
                )
            })?;

        for restore in &first_generation.prologue_restores {
            // SAFETY: the bytes and the address were recorded together when this
            // generation was installed. `restore_prologue` re-reads the
            // function's extent and checks the installed jump before writing, so
            // a replaced image is refused rather than overwritten. No system is
            // executing.
            unsafe { pill_engine::hot_patch::restore_prologue(restore.address, &restore.original) }
                .map_err(|detail| format!("{}: {detail}", restore.artifact))?;
        }
        Ok(())
    }

    /// Return one function to the code its artifact was built with.
    ///
    /// A system has one baseline address, recorded in the engine's registry. A
    /// plain function has one per artifact, so each artifact is asked to empty
    /// its own slot instead of being handed an address that belongs to another.
    fn restore_baseline(
        &self,
        engine: &mut Engine,
        targets: &[(&str, &NativeLibrary)],
        patches: &[LoadedPatch],
        qualified: &str,
    ) -> Result<(), String> {
        // The route's own lookup name, not the qualified one: a method's slot
        // is registered without its type, so asking under the qualified name
        // would miss it.
        let (kind, lookup_name) = self
            .generations
            .iter()
            .find(|candidate| candidate.function == qualified)
            .map(|generation| (generation.kind, generation.lookup_name.clone()))
            .ok_or_else(|| format!("`{qualified}` has no recorded generation"))?;

        match kind {
            source::HotFunctionKind::System => {
                let (address, signature_hash) = engine
                    .hot_patch_baseline(qualified)
                    .ok_or_else(|| format!("`{qualified}` records no baseline implementation"))?;
                engine
                    .hot_patch(qualified, address, signature_hash)
                    .map_err(|error| error.to_string())
            }
            source::HotFunctionKind::PlainFunction => {
                reset_everywhere(targets, patches, &lookup_name)
            }
        }
    }
}
