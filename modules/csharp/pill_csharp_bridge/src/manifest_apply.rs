//! The apply pipeline that every managed manifest kind runs through.
//!
//! # Responsibilities
//!
//! - Decide what each arriving declaration means for the live binding table:
//!   nothing changed, a new entry, a rename through an alias, or a reshape.
//! - Apply the entries in manifest order, recording what would undo each
//!   applied entry so a later refusal can unwind them.
//! - Retire the entries the manifest stopped naming, after everything else.
//!
//! # Design
//!
//! Every manifest kind needs the same stages in the same order. What one kind
//! contributes arrives as a [`ManifestSubject`]: a plain struct of function
//! pointers, in the same style as the `ColumnOps` and `StorageFactory` tables
//! that carry per-kind behaviour elsewhere in the engine. Everything else
//! lives here, once.
//!
//! - **Aliases are resolved before vanished entries are collected**, so when
//!   an alias reaches a predecessor that is still live, that predecessor counts
//!   as renamed instead of as gone.
//! - **Every entry is resolved before any of them is applied**, so the refusals
//!   that the manifest's own validation cannot find leave the world and the
//!   table as they were.
//! - **Retirement runs last**, because it drops bytes and cannot be undone.
//!
//! ## What rollback can and cannot promise
//!
//! The table is all-or-nothing: the pipeline works on a copy, and the caller
//! publishes whatever comes back. A component's reshape comes with an undo that
//! moves the rows back. A resource's reshape cannot do that, because reversing
//! it would have to invent the bytes it dropped. That reshape therefore stays
//! applied with its new binding: the table has to keep describing the layout
//! the engine holds, because the resource view callback checks the live byte
//! length against it.

// Standard library
use std::collections::{HashMap, HashSet};

// External crates
use pill_core::error::CSharpError;
use pill_core::telemetry::telemetry_target;
use pill_core::{error, info};
use pill_engine::Engine;

// Current crate
use super::components::StableComponentId;

// =============================================================================
// Types
// =============================================================================

/// The live table one manifest kind keeps, keyed by stable identity.
pub(super) type BindingTable<Binding> = HashMap<StableComponentId, Binding>;

/// What a subject action reports: the binding it produced, and how to reverse
/// it.
///
/// `None` in the second slot means the action's effect on the engine cannot be
/// reversed. That is a property of the kind, not of this call; see the
/// rollback note in this module's docs.
pub(super) type Applied<Binding, Undo> = Result<(Binding, Option<Undo>), CSharpError>;

/// Resolve one alias to the live table entry it names, if any.
type ResolveAlias<Binding> =
    fn(&Engine, &BindingTable<Binding>, &str) -> Result<Option<StableComponentId>, CSharpError>;

/// Register a declaration the table has no entry for.
type Register<Declaration, Binding, Undo> =
    fn(&mut Engine, StableComponentId, &Declaration) -> Applied<Binding, Undo>;

/// Migrate a live entry's stored data into the arriving shape.
type Reshape<Declaration, Binding, Undo> =
    fn(&mut Engine, StableComponentId, &Binding, &Declaration) -> Applied<Binding, Undo>;

/// Register a successor and move a predecessor's data onto it.
type Rename<Declaration, Binding, Undo> = fn(
    &mut Engine,
    StableComponentId,
    &RenamePredecessor<'_, Binding>,
    &Declaration,
) -> Applied<Binding, Undo>;

/// The live entry that an arriving declaration's alias reached.
///
/// The whole entry is passed because applying a rename retires the predecessor.
/// Once the data has moved, the registration is gone, so this is the only
/// description of what was there.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) struct RenamePredecessor<'binding, Binding> {
    /// The table key the predecessor is bound under.
    pub(super) identity: StableComponentId,
    /// The alias that reached it, which is the name it was registered under.
    pub(super) alias: &'binding str,
    /// Its live binding.
    pub(super) binding: &'binding Binding,
}

/// What an arriving declaration means for a live binding.
///
/// This is the one decision the pipeline cannot make for itself: whether the
/// stored bytes are still valid under the arriving shape depends on layouts,
/// and only the kind knows how its layouts compare.
pub(super) enum Settlement {
    /// The live binding already describes the arriving declaration.
    Unchanged,
    /// The shape moved, so the stored bytes have to migrate.
    Reshaped,
}

/// What one manifest kind contributes to the shared pipeline.
///
/// A plain struct of function pointers rather than a trait, matching the
/// `ColumnOps` and `StorageFactory` style this codebase already uses for
/// per-kind behaviour tables. The actions that write an entry - `register`,
/// `reshape` and `rename` - also receive the stable identity the pipeline
/// resolved, so none of them has to work it out again.
pub(super) struct ManifestSubject<Declaration, Binding, Undo> {
    /// What this kind is called in log lines - "component", "resource".
    pub(super) kind: &'static str,
    /// The stable identity an arriving declaration claims.
    pub(super) identity: fn(&Declaration) -> StableComponentId,
    /// The declared name, for reports and refusals.
    pub(super) name: fn(&Declaration) -> &str,
    /// The previous names this declaration may have been registered under. A
    /// live entry under one of them becomes its predecessor.
    pub(super) aliases: fn(&Declaration) -> &[String],
    /// The name a live binding was registered under, for the retirement report.
    pub(super) binding_name: fn(&Engine, &Binding) -> String,
    /// Whether a live entry is one this pipeline owns and may retire.
    ///
    /// A table can hold entries this manifest does not govern. The component
    /// table also holds module-native bindings - components an extension
    /// registered in Rust - and those are never retired just because the
    /// manifest stopped naming them.
    pub(super) is_governed: fn(&Binding) -> bool,
    /// Resolve one alias to the live table entry it names, if any.
    pub(super) resolve_alias: ResolveAlias<Binding>,
    /// Refuse a declaration that cannot be registered as it stands. Called only
    /// for a declaration the table has no entry for.
    pub(super) check_addable: fn(&Declaration) -> Result<(), CSharpError>,
    /// Decide what an arriving declaration means for a live binding.
    pub(super) settle: fn(&Engine, &Binding, &Declaration) -> Result<Settlement, CSharpError>,
    /// Register a declaration the table has no entry for.
    pub(super) register: Register<Declaration, Binding, Undo>,
    /// Migrate a live entry's stored bytes into the arriving shape.
    pub(super) reshape: Reshape<Declaration, Binding, Undo>,
    /// Register the successor and move the predecessor's data onto it.
    pub(super) rename: Rename<Declaration, Binding, Undo>,
    /// Retire every entry the manifest stopped naming, in one call.
    ///
    /// The pipeline calls it last, because a retirement cannot be journalled:
    /// no step that can fail may run after it. It reports how many entries it
    /// retired and cannot report an error, because nothing would be left to
    /// react to one.
    pub(super) retire: fn(&mut Engine, &[Binding]) -> usize,
    /// Undo one applied entry during a rollback, best effort.
    pub(super) undo: fn(&mut Engine, Undo),
}

/// What one applied manifest changed.
///
/// Every kind fills the same shape. The pipeline fills it in, and each caller
/// folds it into whatever report its own callers read.
#[derive(Debug, Default)]
pub(super) struct ManifestOutcome {
    /// Entries the arriving manifest declares for the first time.
    pub(super) added: Vec<String>,
    /// Entries whose layout changed and whose stored data was migrated.
    pub(super) migrated: Vec<String>,
    /// Entries the manifest renamed through an alias, shown as
    /// `old name -> new name`.
    pub(super) renamed: Vec<String>,
    /// Entries retired because the manifest stopped naming them.
    pub(super) retired: Vec<String>,
}

/// One arriving declaration, resolved against the live table.
enum Resolution<'declaration, Declaration> {
    /// The table already agrees with the declaration.
    Settled,
    /// The table has no entry for it.
    Add(&'declaration Declaration),
    /// An alias reached a live predecessor, whose data moves onto this entry.
    Rename {
        declaration: &'declaration Declaration,
        predecessor: StableComponentId,
        alias: String,
    },
    /// The shape moved, so the stored data has to migrate.
    Reshape(&'declaration Declaration),
}

/// One applied entry, recorded so a later refusal can undo it.
///
/// The table half belongs to the pipeline: it records exactly which entries the
/// step wrote, so a rollback can restore the map without the subject knowing a
/// table exists. The engine half belongs to the subject and is absent for a
/// step whose effect cannot be reversed.
struct JournalEntry<Binding, Undo> {
    /// The table entry the step wrote: the identity, and the binding that was
    /// there before, or `None` when the key was new.
    written_entry: (StableComponentId, Option<Binding>),
    /// A predecessor the step removed, and the binding it held.
    removed: Option<(StableComponentId, Binding)>,
    /// How to reverse the step's effect on the engine, when it can be.
    undo: Option<Undo>,
}

/// One kind's declarations, resolved against the live table and ready to
/// apply. [`prepare_manifest`] produces it, and nothing in it has changed the
/// engine yet.
pub(super) struct PreparedManifest<'declaration, Declaration, Binding> {
    /// The table the resolutions were made against. It is passed on so the
    /// execute half writes into it instead of reading the table again.
    previous: BindingTable<Binding>,
    /// The entries the manifest stopped naming, retired after everything else.
    vanished_identities: Vec<StableComponentId>,
    /// Each arriving entry and what has to happen to it.
    resolutions: Vec<(StableComponentId, Resolution<'declaration, Declaration>)>,
}

// =============================================================================
// The pipeline
// =============================================================================

/// The half of the pipeline that decides what a manifest means, without
/// changing anything.
///
/// It raises every refusal the pipeline can make about the manifest's own
/// shape. It is split from [`execute_manifest`] so one caller can resolve two
/// kinds before executing either. Resources and components arrive in one
/// manifest, and a shape refusal in one kind must not leave the other kind's
/// relayout already applied. Nothing here changes the engine or the table.
///
/// # Errors
///
/// Returns a [`CSharpError`] when an alias names more than one live
/// registration, when one declaration's aliases reach more than one
/// predecessor, when a declaration the table has no entry for cannot be
/// registered as it stands, when an entry the table already holds also declares
/// an alias, or when the subject refuses the arriving shape against the live
/// binding.
pub(super) fn prepare_manifest<'declaration, Declaration, Binding, Undo>(
    subject: &ManifestSubject<Declaration, Binding, Undo>,
    engine: &Engine,
    previous: BindingTable<Binding>,
    declarations: &'declaration [Declaration],
) -> Result<PreparedManifest<'declaration, Declaration, Binding>, CSharpError>
where
    Binding: Clone,
{
    // Step 1: Resolve aliases first, before anything looks at what vanished.
    // An entry whose alias reaches a live predecessor is a rename, so that
    // predecessor counts as renamed, not as gone.
    let renames = resolve_renames(subject, engine, &previous, declarations)?;

    // Step 2: Collect the entries the manifest stopped naming. They are retired
    // after everything else has applied, so a rename can still claim one of
    // them here, and its data moves instead of dropping.
    let arriving_identities: HashSet<StableComponentId> = declarations
        .iter()
        .map(|declaration| (subject.identity)(declaration))
        .collect();
    let renamed_predecessors: HashSet<StableComponentId> = renames
        .values()
        .map(|(predecessor, _)| *predecessor)
        .collect();
    let vanished_identities: Vec<StableComponentId> = previous
        .iter()
        .filter(|(identity, binding)| {
            !arriving_identities.contains(*identity)
                && !renamed_predecessors.contains(*identity)
                && (subject.is_governed)(binding)
        })
        .map(|(identity, _)| *identity)
        .collect();

    // Step 3: Resolve every entry before the first change. Everything here only
    // reads, so a refusal leaves the world and the table alone.
    let resolutions = resolve_entries(subject, engine, &previous, declarations, &renames)?;

    Ok(PreparedManifest {
        previous,
        vanished_identities,
        resolutions,
    })
}

/// The half of the pipeline that changes things: applies what
/// [`prepare_manifest`] resolved, unwinds on a refusal, and retires last.
///
/// # Errors
///
/// Returns the error that one of the subject's `register`, `reshape` or
/// `rename` actions reported, after every entry applied so far has been rolled
/// back. The table comes back in every case, so the caller can publish it
/// either way.
pub(super) fn execute_manifest<Declaration, Binding, Undo>(
    subject: &ManifestSubject<Declaration, Binding, Undo>,
    engine: &mut Engine,
    prepared: PreparedManifest<'_, Declaration, Binding>,
) -> (Result<ManifestOutcome, CSharpError>, BindingTable<Binding>)
where
    Binding: Clone,
{
    let PreparedManifest {
        previous,
        vanished_identities,
        resolutions,
    } = prepared;

    // Step 4: Apply each resolved entry, recording one journal entry per
    // applied resolution.
    let mut table = previous;
    let mut outcome = ManifestOutcome::default();
    let mut journal: Vec<JournalEntry<Binding, Undo>> = Vec::with_capacity(resolutions.len());
    for (identity, resolution) in resolutions {
        let applied = apply_resolution(
            subject,
            engine,
            &mut table,
            identity,
            resolution,
            &mut outcome,
        );
        match applied {
            Ok(Some(entry)) => journal.push(entry),
            Ok(None) => {}
            Err(error) => {
                roll_back(subject, engine, &mut table, journal);
                return (Err(error), table);
            }
        }
    }

    // Step 5: Retire the declarations the manifest stopped naming, once every
    // addition and migration has applied. Nothing else in the pipeline runs
    // after this, so no later refusal has to undo it.
    if !vanished_identities.is_empty() {
        let retired: Vec<Binding> = vanished_identities
            .iter()
            .filter_map(|identity| table.get(identity).cloned())
            .collect();
        // Read the names before the retirement, not after: retiring an entry
        // takes its registration with it, and the report needs the name it was
        // registered under.
        for binding in &retired {
            outcome
                .retired
                .push((subject.binding_name)(engine, binding));
        }
        let touched = (subject.retire)(engine, &retired);
        for identity in &vanished_identities {
            table.remove(identity);
        }
        info!(
            target: telemetry_target::HOT_RELOAD,
            kind = subject.kind,
            entries = retired.len(),
            touched,
            "retired managed declarations the manifest stopped naming"
        );
    }

    (Ok(outcome), table)
}

/// Resolve every declaration's aliases to the live entries they name.
///
/// Each alias is looked up once, and only among live entries: an entry that is
/// no longer registered has no data left to carry. The entry an alias names
/// becomes a rename instead of an add. An alias that matches nothing is left
/// alone: it names nothing this path owns, and a later registration under that
/// name is an ordinary add.
///
/// # Errors
///
/// Returns a [`CSharpError`] when an alias is ambiguous, or when one entry
/// collects predecessors through more than one alias. Two old registrations
/// cannot both be an entry's past, and picking one would silently drop the
/// other's data.
fn resolve_renames<Declaration, Binding, Undo>(
    subject: &ManifestSubject<Declaration, Binding, Undo>,
    engine: &Engine,
    previous: &BindingTable<Binding>,
    declarations: &[Declaration],
) -> Result<HashMap<StableComponentId, (StableComponentId, String)>, CSharpError> {
    let mut renames: HashMap<StableComponentId, (StableComponentId, String)> = HashMap::new();
    for declaration in declarations {
        let successor = (subject.identity)(declaration);
        for alias in (subject.aliases)(declaration) {
            let Some(predecessor) = (subject.resolve_alias)(engine, previous, alias)? else {
                continue;
            };
            if renames.contains_key(&successor) {
                return Err(CSharpError::ManifestInvalid {
                    message: format!(
                        "managed {} {} declares aliases for more than one previous registration",
                        subject.kind,
                        (subject.name)(declaration),
                    ),
                });
            }
            renames.insert(successor, (predecessor, alias.clone()));
        }
    }
    Ok(renames)
}

/// Resolve every declaration against the live table, refusing before anything is
/// applied.
///
/// # Errors
///
/// Returns a [`CSharpError`] when a declaration cannot be registered as it
/// stands, when a rename's successor name is already bound, or when the
/// subject refuses the arriving shape against the live one.
#[allow(clippy::type_complexity)]
fn resolve_entries<'declaration, Declaration, Binding, Undo>(
    subject: &ManifestSubject<Declaration, Binding, Undo>,
    engine: &Engine,
    previous: &BindingTable<Binding>,
    declarations: &'declaration [Declaration],
    renames: &HashMap<StableComponentId, (StableComponentId, String)>,
) -> Result<Vec<(StableComponentId, Resolution<'declaration, Declaration>)>, CSharpError> {
    let mut resolutions = Vec::with_capacity(declarations.len());
    for declaration in declarations {
        let identity = (subject.identity)(declaration);
        let Some(binding) = previous.get(&identity) else {
            (subject.check_addable)(declaration)?;
            // An alias that reached a predecessor turns this entry into a
            // rename; without one it is an ordinary addition.
            let resolution = match renames.get(&identity) {
                Some((predecessor, alias)) => Resolution::Rename {
                    declaration,
                    predecessor: *predecessor,
                    alias: alias.clone(),
                },
                None => Resolution::Add(declaration),
            };
            resolutions.push((identity, resolution));
            continue;
        };

        // A successor's identity is derived from its new name, so finding the
        // identity in the table means the name is already registered while its
        // aliases still name an earlier registration. This is refused rather
        // than settled: settling would leave the predecessor's data in a
        // binding that nothing tracks.
        if renames.contains_key(&identity) {
            return Err(CSharpError::ManifestInvalid {
                message: format!(
                    "managed {} {} is already registered and also declares an alias for an \
                     earlier registration; retire one of the two names",
                    subject.kind,
                    (subject.name)(declaration),
                ),
            });
        }

        let resolution = match (subject.settle)(engine, binding, declaration)? {
            Settlement::Unchanged => Resolution::Settled,
            Settlement::Reshaped => Resolution::Reshape(declaration),
        };
        resolutions.push((identity, resolution));
    }
    Ok(resolutions)
}

/// Apply one resolved entry, recording what it wrote.
///
/// # Errors
///
/// Whatever the subject's action reports. The table is only written after the
/// action succeeded, so a refusal here leaves the entry untouched and the
/// journal describes only the entries that did apply.
///
/// # Panics
///
/// Panics when a resolution names an entry the table does not hold, which the
/// resolve half makes impossible.
fn apply_resolution<Declaration, Binding, Undo>(
    subject: &ManifestSubject<Declaration, Binding, Undo>,
    engine: &mut Engine,
    table: &mut BindingTable<Binding>,
    identity: StableComponentId,
    resolution: Resolution<'_, Declaration>,
    outcome: &mut ManifestOutcome,
) -> Result<Option<JournalEntry<Binding, Undo>>, CSharpError>
where
    Binding: Clone,
{
    match resolution {
        Resolution::Settled => Ok(None),
        Resolution::Add(declaration) => {
            let (binding, undo) = (subject.register)(engine, identity, declaration)?;
            let written_entry = (identity, table.insert(identity, binding));
            outcome.added.push((subject.name)(declaration).to_string());
            Ok(Some(JournalEntry {
                written_entry,
                removed: None,
                undo,
            }))
        }
        Resolution::Reshape(declaration) => {
            let existing_binding = table
                .get(&identity)
                .cloned()
                .expect("a reshape is only resolved for an entry the table holds");
            let (binding, undo) =
                (subject.reshape)(engine, identity, &existing_binding, declaration)?;
            let written_entry = (identity, table.insert(identity, binding));
            outcome
                .migrated
                .push((subject.name)(declaration).to_string());
            Ok(Some(JournalEntry {
                written_entry,
                removed: None,
                undo,
            }))
        }
        Resolution::Rename {
            declaration,
            predecessor,
            alias,
        } => {
            let predecessor_binding = table
                .get(&predecessor)
                .cloned()
                .expect("the resolver only names predecessors the table holds");
            let (binding, undo) = (subject.rename)(
                engine,
                identity,
                &RenamePredecessor {
                    identity: predecessor,
                    alias: &alias,
                    binding: &predecessor_binding,
                },
                declaration,
            )?;
            let written_entry = (identity, table.insert(identity, binding));
            let removed = table
                .remove(&predecessor)
                .map(|removed| (predecessor, removed));
            outcome
                .renamed
                .push(format!("{alias} -> {}", (subject.name)(declaration)));
            Ok(Some(JournalEntry {
                written_entry,
                removed,
                undo,
            }))
        }
    }
}

/// Undo every applied entry, newest first.
///
/// The engine side is best effort by design. A rollback that fails leaves the
/// process in a mixed state. That failure is logged where it happened instead
/// of being returned in place of the original refusal, which is the error the
/// caller needs.
///
/// The table side is not best effort. When an entry's effect on the engine was
/// reversed, its table entry is put back exactly as it was. An entry whose
/// effect could not be reversed keeps the binding it was given, because the
/// table has to keep describing what the engine actually holds.
fn roll_back<Declaration, Binding, Undo>(
    subject: &ManifestSubject<Declaration, Binding, Undo>,
    engine: &mut Engine,
    table: &mut BindingTable<Binding>,
    journal: Vec<JournalEntry<Binding, Undo>>,
) {
    for entry in journal.into_iter().rev() {
        let Some(undo) = entry.undo else {
            // Nothing to reverse: the step's effect on the engine is permanent,
            // so its binding stays in the table describing that effect.
            continue;
        };
        (subject.undo)(engine, undo);
        let (identity, replaced_binding) = entry.written_entry;
        match replaced_binding {
            Some(binding) => table.insert(identity, binding),
            None => table.remove(&identity),
        };
        if let Some((predecessor, binding)) = entry.removed {
            table.insert(predecessor, binding);
        }
    }
    error!(
        target: telemetry_target::HOT_RELOAD,
        kind = subject.kind,
        "rolled back a refused manifest"
    );
}
