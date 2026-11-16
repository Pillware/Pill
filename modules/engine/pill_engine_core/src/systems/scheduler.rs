//! Works out which systems may run at the same time, and groups them into
//! parallel batches.
//!
//! # Responsibilities
//!
//! - Provides [`SystemAccess`] for declaring what a system reads and writes.
//! - Builds the conflict matrix and the parallel batches from the registered
//!   systems' access patterns.
//! - Tracks when those results need rebuilding, through
//!   [`SystemScheduler::invalidate`] and [`SystemScheduler::clear`].
//! - Defines [`TypeKey`], a [`TypeId`] wrapper for callers that key native
//!   types by identity.
//!
//! # Design
//!
//! Each system declares which components and resources it reads and writes.
//! The scheduler compares every pair of systems, so the cost grows with the
//! square of the system count (O(n²)). Two systems conflict when one writes
//! something the other reads or writes. Systems that do not conflict share a
//! parallel batch. A system that uses [`Commands`](crate::Commands) needs
//! exclusive access to the world, so it conflicts with every other system and
//! always runs alone.
//!
//! The scheduler keeps no list of systems of its own. The engine owns the
//! registration records and marks the scheduler stale whenever that list
//! changes; [`SystemScheduler::ensure_execution_graph`] then rebuilds the
//! matrix and the batches once from the access patterns it is given, and does
//! nothing until the next invalidation. So the engine is the only owner of the
//! system list, and one flag says whether the graph is current.

// Standard library
use std::any::TypeId;
use std::collections::HashSet;

// Current crate
use crate::component::ComponentId;
use crate::resource::ResourceId;

// =============================================================================
// TypeKey
// =============================================================================

/// Wraps a [`TypeId`] so a native type can be a map key that compares by
/// identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeKey(pub TypeId);

impl TypeKey {
    /// Builds the [`TypeKey`] for a given type `T`.
    pub fn of<T: 'static>() -> Self {
        TypeKey(TypeId::of::<T>())
    }

    /// Returns the underlying [`TypeId`].
    pub fn type_id(self) -> TypeId {
        self.0
    }
}

// =============================================================================
// SystemAccess
// =============================================================================

/// What a system reads and writes: its component and resource access.
///
/// Call [`add_read`](SystemAccess::add_read),
/// [`add_write`](SystemAccess::add_write),
/// [`add_resource_read`](SystemAccess::add_resource_read) and
/// [`add_resource_write`](SystemAccess::add_resource_write) before the system
/// is registered. [`conflicts_with`](SystemAccess::conflicts_with) then
/// compares the sets directly.
///
/// The sets are the only record of what a system touches, so no cached answer
/// can disagree with them. The engine keeps the copy it was given when the
/// system was registered, so an access added afterwards never reaches the
/// batches.
#[derive(Debug, Clone, Default)]
pub struct SystemAccess {
    /// Components read immutably (`&T`).
    component_reads: HashSet<ComponentId>,
    /// Components written mutably (`&mut T`).
    component_writes: HashSet<ComponentId>,
    /// Whether the system uses `Commands`, which needs exclusive access to the
    /// world.
    pub uses_commands: bool,
    /// Resources read immutably (`Res<T>`).
    resource_reads: HashSet<ResourceId>,
    /// Resources written mutably (`ResMut<T>`).
    resource_writes: HashSet<ResourceId>,
}

impl SystemAccess {
    /// Creates an empty [`SystemAccess`] with no components or resources.
    ///
    /// Equivalent to [`SystemAccess::default`]. Access patterns are built up
    /// with [`add_read`](SystemAccess::add_read) and
    /// [`add_write`](SystemAccess::add_write) before registration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `component_id` as read-only (`&T`) access for this system.
    #[inline]
    pub fn add_read(&mut self, component_id: ComponentId) {
        self.component_reads.insert(component_id);
    }

    /// Registers `component_id` as mutable (`&mut T`) access for this system.
    ///
    /// A write to a component conflicts with any other system reading or
    /// writing the same component.
    #[inline]
    pub fn add_write(&mut self, component_id: ComponentId) {
        self.component_writes.insert(component_id);
    }

    /// Marks whether this system uses [`Commands`](crate::Commands).
    ///
    /// A system that uses [`Commands`](crate::Commands) requires exclusive
    /// world access and therefore conflicts with every other system.
    pub fn set_uses_commands(&mut self, uses_commands: bool) {
        self.uses_commands = uses_commands;
    }

    /// Registers `resource_id` as immutable (`Res<T>`) access for this system.
    #[inline]
    pub fn add_resource_read(&mut self, resource_id: ResourceId) {
        self.resource_reads.insert(resource_id);
    }

    /// Registers `resource_id` as mutable (`ResMut<T>`) access for this system.
    #[inline]
    pub fn add_resource_write(&mut self, resource_id: ResourceId) {
        self.resource_writes.insert(resource_id);
    }

    /// Folds another access pattern into this one.
    ///
    /// The engine uses it once per registration to add a `run_if` condition's
    /// reads to the access its system declares, so the batches also respect
    /// what the conditions read. After it returns, the two patterns are
    /// indistinguishable: only the merged sets are kept.
    pub fn merge(&mut self, other: &SystemAccess) {
        self.component_reads
            .extend(other.component_reads.iter().copied());
        self.component_writes
            .extend(other.component_writes.iter().copied());
        self.uses_commands |= other.uses_commands;
        self.resource_reads
            .extend(other.resource_reads.iter().copied());
        self.resource_writes
            .extend(other.resource_writes.iter().copied());
    }

    /// How many components this system reads.
    ///
    /// Reporting only: the ids stay private, so nothing outside this type can
    /// change the set after registration.
    pub fn reads_count(&self) -> usize {
        self.component_reads.len()
    }

    /// How many components this system writes.
    pub fn writes_count(&self) -> usize {
        self.component_writes.len()
    }

    /// How many resources this system reads.
    pub fn resource_reads_count(&self) -> usize {
        self.resource_reads.len()
    }

    /// How many resources this system writes.
    pub fn resource_writes_count(&self) -> usize {
        self.resource_writes.len()
    }

    /// Checks whether this system conflicts with another.
    ///
    /// Two systems conflict when either one uses [`Commands`](crate::Commands),
    /// or when one writes a component or resource the other reads or writes.
    /// The sets answer directly, so the result always matches what the systems
    /// really touch. A cached answer could miss an access it never saw, and two
    /// writers of one component would then run on different threads.
    ///
    /// The sets hold only a handful of ids per system, so the comparisons are
    /// cheap. The scheduler stores the answer in its conflict matrix when it
    /// rebuilds, and the frame runs the batches built from that matrix.
    #[inline]
    pub fn conflicts_with(&self, other: &SystemAccess) -> bool {
        // Step 1: Commands require exclusive world access, so a system that
        // uses them conflicts with everything.
        if self.uses_commands || other.uses_commands {
            return true;
        }

        // Step 2: A component write shared with the other side's reads or
        // writes is a conflict, in either direction.
        if !self.component_writes.is_disjoint(&other.component_writes) {
            return true;
        }
        if !self.component_writes.is_disjoint(&other.component_reads) {
            return true;
        }
        if !self.component_reads.is_disjoint(&other.component_writes) {
            return true;
        }

        // Step 3: Resource conflicts follow the same rule: a write shared with
        // the other side's reads or writes is a conflict.
        if !self.resource_writes.is_disjoint(&other.resource_writes) {
            return true;
        }
        if !self.resource_writes.is_disjoint(&other.resource_reads) {
            return true;
        }
        if !self.resource_reads.is_disjoint(&other.resource_writes) {
            return true;
        }

        false
    }
}

// =============================================================================
// RunPhase
// =============================================================================

/// The phase of the frame a system runs in.
///
/// A frame runs the phases in this order: [`PreProject`](RunPhase::PreProject),
/// then the first deferred-command flush, then
/// [`Project`](RunPhase::Project) - the game's own frame work - then the second
/// flush, then [`PostProject`](RunPhase::PostProject). The first two phases
/// dispatch parallel batches; `PostProject` runs on the calling thread, because
/// it is where presentation systems draw the frame they have just finished
/// computing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunPhase {
    /// Runs before the first command flush, so the commands it queues are
    /// applied before the project phase reads the world.
    PreProject,
    /// The game's own frame work. What a declaration that names no phase gets.
    #[default]
    Project,
    /// Runs after the second command flush, sequentially.
    PostProject,
}

impl RunPhase {
    /// Every phase a schedule can hold, in frame order.
    pub const ALL: [Self; 3] = [Self::PreProject, Self::Project, Self::PostProject];

    /// The phases that dispatch parallel batches, in frame order.
    pub const BATCHED: [Self; 2] = [Self::PreProject, Self::Project];

    /// The spelling a `run_phase = "..."` declaration uses.
    pub fn label(self) -> &'static str {
        match self {
            Self::PreProject => "pre_project",
            Self::Project => "project",
            Self::PostProject => "post_project",
        }
    }

    /// Whether the phase's systems run in parallel batches.
    pub fn is_batched(self) -> bool {
        matches!(self, Self::PreProject | Self::Project)
    }
}

// =============================================================================
// SystemScheduler
// =============================================================================

/// The conflict matrix and the parallel batches of one batched phase.
///
/// The matrix depends only on the phase's access patterns, and the batches
/// depend only on the matrix. Both are rebuilt together, from scratch, by
/// [`SystemScheduler::ensure_execution_graph`].
#[derive(Default)]
struct PhaseGraph {
    /// `conflict_matrix[i][j]` is true when the phase's system `i` conflicts
    /// with its system `j`, both being positions in the phase's own list.
    /// Never edited in place.
    conflict_matrix: Vec<Vec<bool>>,
    /// Batches of indices into the engine's whole system list. No two systems
    /// in one batch conflict, so a batch can run in parallel.
    batches: Vec<Vec<usize>>,
}

impl PhaseGraph {
    /// Rebuilds the matrix and the batches from the phase's access patterns.
    ///
    /// `accesses` are the phase's systems in registration order and `indices`
    /// are their positions in the engine's whole system list, so a batch holds
    /// indices the engine can dispatch with directly.
    fn rebuild(&mut self, accesses: &[&SystemAccess], indices: &[usize]) {
        // Step 1: Build the conflict matrix from the access list. Each unordered
        // pair is compared once and the answer is written to both cells, so the
        // matrix stays symmetric.
        self.conflict_matrix = vec![vec![false; accesses.len()]; accesses.len()];
        for (first, access) in accesses.iter().enumerate() {
            for (second, other) in accesses.iter().enumerate().take(first) {
                let conflict = access.conflicts_with(other);
                self.conflict_matrix[first][second] = conflict;
                self.conflict_matrix[second][first] = conflict;
            }
        }

        // Step 2: Give each system the level one past the latest conflicting
        // system before it, then collect the systems of one level into one
        // batch. Levels are contiguous from zero - a level can only appear
        // after every lower one exists - so the batch vector only ever grows by
        // one at the end.
        let mut batches = std::mem::take(&mut self.batches);
        batches.clear();
        let mut levels: Vec<usize> = Vec::with_capacity(accesses.len());
        for (system_index, &global_index) in indices.iter().enumerate() {
            let level = (0..system_index)
                .filter(|&earlier| self.conflict_matrix[system_index][earlier])
                .map(|earlier| levels[earlier] + 1)
                .max()
                .unwrap_or(0);
            levels.push(level);
            if batches.len() == level {
                batches.push(Vec::new());
            }
            batches[level].push(global_index);
        }
        self.batches = batches;
    }
}

/// The conflict matrices and the parallel batches, one graph per batched phase.
///
/// The graphs depend only on the access patterns they were built from, and the
/// batches depend only on the matrices. Both are rebuilt together by
/// [`ensure_execution_graph`](SystemScheduler::ensure_execution_graph) after
/// [`invalidate`](SystemScheduler::invalidate), so a single flag says whether
/// the graphs are current and no caller has to work that out.
///
/// Each phase gets its own graph rather than one graph over every system: a
/// phase's systems never run beside another phase's, so a conflict across
/// phases must not cost a batch inside either of them.
pub struct SystemScheduler {
    /// The graph of systems that run before the first command flush.
    pre_project: PhaseGraph,
    /// The graph of the game's own frame work.
    project: PhaseGraph,
    /// Whether the matrices and the graphs are older than the current system
    /// set.
    ///
    /// Starts true: a fresh scheduler has built nothing yet.
    needs_rebuild: bool,
}

impl SystemScheduler {
    /// Creates an empty scheduler that builds its first graphs on demand.
    pub fn new() -> Self {
        Self {
            pre_project: PhaseGraph::default(),
            project: PhaseGraph::default(),
            needs_rebuild: true,
        }
    }

    /// Marks the conflict matrices and the batches stale.
    ///
    /// Registrations and removals call this. Removing every system calls
    /// [`clear`](SystemScheduler::clear) instead, which marks the graphs stale
    /// the same way. The rebuild itself happens once, in
    /// [`ensure_execution_graph`](SystemScheduler::ensure_execution_graph),
    /// so a module that registers many systems in its `init` pays for one
    /// rebuild, not one per registration.
    ///
    /// Enabled state is not conflict data: a disabled system stays in the batch
    /// it was given and is skipped at dispatch.
    pub fn invalidate(&mut self) {
        self.needs_rebuild = true;
    }

    /// Rebuilds the conflict matrices and the execution graphs when they are
    /// stale.
    ///
    /// `accesses` must yield every registered system's access pattern with its
    /// phase, in registration order, and its length must be the number of
    /// registered systems, because the graph's indices are positions in that
    /// list. The call does nothing when nothing was invalidated since the last
    /// build.
    ///
    /// Within a phase, a system's batch is one past the latest batch of the
    /// systems registered before it that it conflicts with, and zero when there
    /// is none. Because a conflicting predecessor always sits in a strictly
    /// earlier batch, no batch can hold two systems that conflict, and a later
    /// system can never run before an earlier one it conflicts with -
    /// registration order is the only ordering signal the engine offers (there
    /// is no `before`/`after`), and this keeps it. Filling batches first-fit
    /// instead would let a later system join an earlier batch than a
    /// conflicting predecessor whenever a third system happened to separate
    /// them.
    pub fn ensure_execution_graphs<'a>(
        &mut self,
        accesses: impl Iterator<Item = (&'a SystemAccess, RunPhase)>,
    ) {
        if !self.needs_rebuild {
            return;
        }
        let accesses: Vec<(&SystemAccess, RunPhase)> = accesses.collect();
        let _zone = crate::profile_scope!(
            "build execution graph",
            [("Number of systems to schedule: {}", accesses.len())]
        );

        for phase in RunPhase::BATCHED {
            // The phase's systems in registration order, with their positions
            // in the engine's whole list so a batch can dispatch them directly.
            let indices: Vec<usize> = accesses
                .iter()
                .enumerate()
                .filter(|(_, (_, entry_phase))| *entry_phase == phase)
                .map(|(index, _)| index)
                .collect();
            let phase_accesses: Vec<&SystemAccess> =
                indices.iter().map(|&index| accesses[index].0).collect();
            let graph = match phase {
                RunPhase::PreProject => &mut self.pre_project,
                _ => &mut self.project,
            };
            graph.rebuild(&phase_accesses, &indices);
        }
        self.needs_rebuild = false;
    }

    /// Rebuilds the graphs with every system in the project phase.
    ///
    /// The form for a caller whose systems all run in the phase a bare
    /// registration puts them in; [`ensure_execution_graphs`] is the
    /// phase-aware one.
    ///
    /// [`ensure_execution_graphs`]: SystemScheduler::ensure_execution_graphs
    pub fn ensure_execution_graph<'a>(
        &mut self,
        accesses: impl ExactSizeIterator<Item = &'a SystemAccess>,
    ) {
        self.ensure_execution_graphs(accesses.map(|access| (access, RunPhase::Project)));
    }

    /// Returns the batches of system indices that can run in parallel in one
    /// phase.
    ///
    /// The result is only current after
    /// [`ensure_execution_graphs`](SystemScheduler::ensure_execution_graphs);
    /// the engine calls that before it dispatches a frame. A phase that runs
    /// sequentially has no batches and answers an empty slice.
    pub fn execution_graph_of(&self, phase: RunPhase) -> &[Vec<usize>] {
        match phase {
            RunPhase::PreProject => &self.pre_project.batches,
            RunPhase::Project => &self.project.batches,
            RunPhase::PostProject => &[],
        }
    }

    /// Returns the project phase's batches, the phase a bare registration puts
    /// a system in.
    ///
    /// A caller that schedules other phases uses
    /// [`execution_graph_of`](SystemScheduler::execution_graph_of).
    pub fn execution_graph(&self) -> &[Vec<usize>] {
        &self.project.batches
    }

    /// Clears the conflict matrices and the graphs.
    ///
    /// The engine calls this when it removes every system at once. The next
    /// [`ensure_execution_graph`](SystemScheduler::ensure_execution_graph)
    /// rebuilds from whatever is registered then.
    pub fn clear(&mut self) {
        self.pre_project = PhaseGraph::default();
        self.project = PhaseGraph::default();
        self.needs_rebuild = true;
    }
}

impl Default for SystemScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Conflict-detection regression tests
// =============================================================================

/// Confirms that conflict detection depends only on what two systems touch.
///
/// The tests cover the ways a cached answer would go wrong: an access added
/// after a comparison, two resource sets of equal size that still share an id,
/// a read paired with a write in either direction, and systems that touch
/// nothing in common.
#[cfg(test)]
mod conflict_detection_tests {
    use super::SystemAccess;
    use crate::component::Component;
    use crate::ComponentId;

    struct ComponentA;
    impl Component for ComponentA {}
    struct ComponentB;
    impl Component for ComponentB {}
    struct ResourceX;
    impl crate::resource::Resource for ResourceX {}
    struct ResourceY;
    impl crate::resource::Resource for ResourceY {}
    struct ResourceZ;
    impl crate::resource::Resource for ResourceZ {}

    /// An access added after an earlier comparison is seen by the next one:
    /// the sets are read live, so no cached answer can go stale behind them.
    #[test]
    fn an_access_added_after_a_comparison_still_conflicts() {
        let mut writes_a = SystemAccess::new();
        writes_a.add_write(ComponentId::of::<ComponentA>());

        let mut writes_b = SystemAccess::new();
        writes_b.add_write(ComponentId::of::<ComponentB>());
        assert!(
            !writes_a.conflicts_with(&writes_b),
            "disjoint while neither shares a write"
        );

        // The set grows after the two were compared; the next comparison has
        // to see the shared write.
        writes_a.add_write(ComponentId::of::<ComponentB>());
        assert!(
            writes_a.conflicts_with(&writes_b),
            "the sets are the source of truth and report the shared write"
        );
    }

    /// Equal-length resource sets are compared by content, not by size: two
    /// systems whose resource sets differ while sharing one id still conflict.
    #[test]
    fn a_same_length_resource_edit_still_conflicts() {
        use crate::resource::ResourceId;

        let mut reader = SystemAccess::new();
        reader.add_read(ComponentId::of::<ComponentA>());
        reader.add_resource_read(ResourceId::of::<ResourceX>());
        reader.add_resource_read(ResourceId::of::<ResourceY>());

        let mut writer = SystemAccess::new();
        writer.add_write(ComponentId::of::<ComponentB>());
        writer.add_resource_write(ResourceId::of::<ResourceY>());
        writer.add_resource_write(ResourceId::of::<ResourceZ>());

        assert_eq!(
            reader.resource_reads.len(),
            writer.resource_writes.len(),
            "the two sets are the same size"
        );
        assert!(
            reader.conflicts_with(&writer),
            "the shared resource id is a conflict at any set size"
        );
        assert!(writer.conflicts_with(&reader));
    }

    /// Conflict detection is symmetric, and a read paired with a write is as
    /// much a conflict as two writes.
    #[test]
    fn a_read_paired_with_a_write_conflicts_in_both_directions() {
        let id = ComponentId::of::<ComponentA>();

        let mut reader = SystemAccess::new();
        reader.add_read(id);

        let mut writer = SystemAccess::new();
        writer.add_write(id);

        assert!(reader.conflicts_with(&writer));
        assert!(writer.conflicts_with(&reader));
    }

    /// The check must not over-report: genuinely disjoint systems stay
    /// parallel, and two readers of one component never conflict.
    #[test]
    fn disjoint_systems_do_not_conflict() {
        let mut writes_a = SystemAccess::new();
        writes_a.add_write(ComponentId::of::<ComponentA>());

        let mut writes_b = SystemAccess::new();
        writes_b.add_write(ComponentId::of::<ComponentB>());
        assert!(
            !writes_a.conflicts_with(&writes_b),
            "disjoint systems must still be allowed to run in parallel"
        );

        let mut reader = SystemAccess::new();
        reader.add_read(ComponentId::of::<ComponentA>());
        let mut other_reader = SystemAccess::new();
        other_reader.add_read(ComponentId::of::<ComponentA>());
        assert!(!reader.conflicts_with(&other_reader));
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::hash::BuildHasher;

    use super::*;
    use crate::component::{Component, ComponentId};

    // The scheduler only compares component ids for equality, so a test
    // component needs nothing but a distinct identity. `ComponentId::of` takes
    // `T: Component`, so each stand-in has to be declared here and implement
    // the trait.
    macro_rules! test_components {
        ($($name:ident),* $(,)?) => {$(
            #[derive(Debug)]
            struct $name;
            impl Component for $name {}
        )*};
    }
    test_components!(
        TestComponentA,
        TestComponentB,
        TestComponentC,
        TestComponentD,
        TestComponentE,
        TestComponentF,
        TestComponentG,
    );

    // The same for resources. These tests only ever compare resource ids for
    // equality, so they need nothing of a resource but a distinct identity.
    macro_rules! test_resources {
        ($($name:ident),* $(,)?) => {$(
            #[derive(Debug)]
            struct $name;
            impl crate::resource::Resource for $name {}
        )*};
    }
    test_resources!(TestResourceA, TestResourceB, TestResourceC, TestResourceD);

    /// Builds the batches for a list of access patterns, the way the engine
    /// does after its system set changed.
    fn scheduler_for(accesses: &[SystemAccess]) -> SystemScheduler {
        let mut scheduler = SystemScheduler::new();
        scheduler.ensure_execution_graph(accesses.iter());
        scheduler
    }

    /// Checks that no batch contains two conflicting systems.
    fn assert_no_batch_conflicts(scheduler: &SystemScheduler, accesses: &[SystemAccess]) {
        for (batch_index, batch) in scheduler.execution_graph().iter().enumerate() {
            for (i, &system_index_a) in batch.iter().enumerate() {
                let access_a = &accesses[system_index_a];
                for &system_index_b in &batch[i + 1..] {
                    let access_b = &accesses[system_index_b];
                    assert!(
                        !access_a.conflicts_with(access_b),
                        "Batch {} contains conflicting systems {} and {}!\n\
                         System {}: reads={:?}, writes={:?}, commands={}\n\
                         System {}: reads={:?}, writes={:?}, commands={}",
                        batch_index,
                        system_index_a,
                        system_index_b,
                        system_index_a,
                        access_a.component_reads,
                        access_a.component_writes,
                        access_a.uses_commands,
                        system_index_b,
                        access_b.component_reads,
                        access_b.component_writes,
                        access_b.uses_commands
                    );
                }
            }
        }
    }

    /// Checks that every system is scheduled exactly once.
    fn assert_all_systems_scheduled(scheduler: &SystemScheduler, system_count: usize) {
        let mut scheduled = vec![false; system_count];
        for batch in scheduler.execution_graph() {
            for &system_index in batch {
                assert!(
                    !scheduled[system_index],
                    "System {} scheduled multiple times",
                    system_index
                );
                scheduled[system_index] = true;
            }
        }
        for (system_index, &was_scheduled) in scheduled.iter().enumerate() {
            assert!(was_scheduled, "System {} was not scheduled", system_index);
        }
    }

    /// Checks that a system never runs before an earlier one it conflicts with.
    ///
    /// The batch of a conflicting pair must strictly increase with registration
    /// order, which is the ordering guarantee the engine's systems rely on.
    fn assert_batches_follow_registration_order(
        scheduler: &SystemScheduler,
        accesses: &[SystemAccess],
    ) {
        let mut batch_of = vec![usize::MAX; accesses.len()];
        for (batch_index, batch) in scheduler.execution_graph().iter().enumerate() {
            for &system_index in batch {
                batch_of[system_index] = batch_index;
            }
        }
        for later in 0..accesses.len() {
            for earlier in 0..later {
                if accesses[later].conflicts_with(&accesses[earlier]) {
                    assert!(
                        batch_of[later] > batch_of[earlier],
                        "System {} (batch {}) conflicts with earlier system {} (batch {}) \
                         but does not run after it",
                        later,
                        batch_of[later],
                        earlier,
                        batch_of[earlier]
                    );
                }
            }
        }
    }

    /// Two systems reading different components share one batch.
    #[test]
    fn two_readers_of_different_components_share_a_batch() {
        // The first system reads A, the second reads B.
        let mut reads_a = SystemAccess::new();
        reads_a.add_read(ComponentId::of::<TestComponentA>());
        let mut reads_b = SystemAccess::new();
        reads_b.add_read(ComponentId::of::<TestComponentB>());
        let accesses = vec![reads_a, reads_b];

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 2);

        // Reading different components is no conflict, so both share a batch.
        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 2);
    }

    /// Two systems writing the same component land in separate batches.
    #[test]
    fn two_writers_of_one_component_get_separate_batches() {
        // Both systems write A.
        let mut writes_a = SystemAccess::new();
        writes_a.add_write(ComponentId::of::<TestComponentA>());
        let mut also_writes_a = SystemAccess::new();
        also_writes_a.add_write(ComponentId::of::<TestComponentA>());
        let accesses = vec![writes_a, also_writes_a];

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 2);

        // Two writers of one component can never share a batch.
        assert_eq!(scheduler.execution_graph().len(), 2);
    }

    /// A reader and a writer of the same component land in separate batches.
    #[test]
    fn a_reader_and_a_writer_of_one_component_get_separate_batches() {
        // The first system reads A, the second writes it.
        let mut reads_a = SystemAccess::new();
        reads_a.add_read(ComponentId::of::<TestComponentA>());
        let mut writes_a = SystemAccess::new();
        writes_a.add_write(ComponentId::of::<TestComponentA>());
        let accesses = vec![reads_a, writes_a];

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 2);

        // A reader and a writer of one component can never share a batch.
        assert_eq!(scheduler.execution_graph().len(), 2);
    }

    /// A system registered earlier runs before the systems it conflicts with.
    ///
    /// Registration order is the only ordering signal the API offers (there is
    /// no `before`/`after`), so a producer registered first runs before every
    /// consumer of its output.
    #[test]
    fn conflicting_systems_keep_registration_order() {
        // System 0 writes A: it conflicts with both readers below.
        let mut producer = SystemAccess::new();
        producer.add_write(ComponentId::of::<TestComponentA>());
        let mut accesses = vec![producer];

        // Systems 1 and 2 read A: they conflict with the writer, not each other.
        for _ in 0..2 {
            let mut consumer = SystemAccess::new();
            consumer.add_read(ComponentId::of::<TestComponentA>());
            accesses.push(consumer);
        }

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 3);
        assert_eq!(
            scheduler.execution_graph().to_vec(),
            vec![vec![0], vec![1, 2]],
            "the producer runs in the first batch and the two readers together after it"
        );
    }

    /// A system using Commands conflicts with everything, including a
    /// read-only one.
    #[test]
    fn a_commands_system_shares_no_batch() {
        // The first system uses Commands, the second only reads A.
        let mut uses_commands = SystemAccess::new();
        uses_commands.set_uses_commands(true);
        let mut reads_a = SystemAccess::new();
        reads_a.add_read(ComponentId::of::<TestComponentA>());
        let accesses = vec![uses_commands, reads_a];

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 2);

        // Commands need exclusive access, so the two cannot share a batch.
        assert_eq!(scheduler.execution_graph().len(), 2);
    }

    /// Five readers of the same component share one batch.
    #[test]
    fn five_readers_of_one_component_share_a_batch() {
        // Five systems, each reading A.
        let accesses: Vec<SystemAccess> = (0..5)
            .map(|_| {
                let mut access = SystemAccess::new();
                access.add_read(ComponentId::of::<TestComponentA>());
                access
            })
            .collect();

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 5);

        // Readers never conflict with each other, so all five share one batch.
        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 5);
    }

    /// One writer keeps every reader of the same component out of its batch.
    #[test]
    fn a_writer_keeps_every_reader_of_its_component_out_of_its_batch() {
        // Four readers first, then one writer of the same component.
        let mut accesses: Vec<SystemAccess> = (0..4)
            .map(|_| {
                let mut access = SystemAccess::new();
                access.add_read(ComponentId::of::<TestComponentA>());
                access
            })
            .collect();
        let mut writer = SystemAccess::new();
        writer.add_write(ComponentId::of::<TestComponentA>());
        accesses.push(writer);

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 5);

        // The writer conflicts with every reader, so it gets a batch of its own.
        assert!(scheduler.execution_graph().len() >= 2);
    }

    /// A read-then-write chain (A→B→C) serializes the systems that disagree and
    /// batches the ones that do not.
    #[test]
    fn a_read_write_chain_serializes_only_the_systems_that_disagree() {
        // The first system reads A and writes B.
        let mut reads_a_writes_b = SystemAccess::new();
        reads_a_writes_b.add_read(ComponentId::of::<TestComponentA>());
        reads_a_writes_b.add_write(ComponentId::of::<TestComponentB>());

        // The second reads B and writes C.
        let mut reads_b_writes_c = SystemAccess::new();
        reads_b_writes_c.add_read(ComponentId::of::<TestComponentB>());
        reads_b_writes_c.add_write(ComponentId::of::<TestComponentC>());

        // The third only reads A, so it can share a batch with the first.
        let mut reads_a = SystemAccess::new();
        reads_a.add_read(ComponentId::of::<TestComponentA>());

        // The fourth only reads C, so it can share a batch with the first and
        // the third.
        let mut reads_c = SystemAccess::new();
        reads_c.add_read(ComponentId::of::<TestComponentC>());

        let accesses = vec![reads_a_writes_b, reads_b_writes_c, reads_a, reads_c];
        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 4);
    }

    /// Three writers of three different components share one batch.
    #[test]
    fn writers_of_three_different_components_share_a_batch() {
        let accesses: Vec<SystemAccess> = [
            ComponentId::of::<TestComponentA>(),
            ComponentId::of::<TestComponentB>(),
            ComponentId::of::<TestComponentC>(),
        ]
        .into_iter()
        .map(|component_id| {
            let mut access = SystemAccess::new();
            access.add_write(component_id);
            access
        })
        .collect();

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 3);

        // Writing different components is no conflict, so all three share a batch.
        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 3);
    }

    /// Three Commands systems get one batch each.
    #[test]
    fn three_commands_systems_get_one_batch_each() {
        // Three systems, each using Commands.
        let accesses: Vec<SystemAccess> = (0..3)
            .map(|_| {
                let mut access = SystemAccess::new();
                access.set_uses_commands(true);
                access
            })
            .collect();

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 3);

        // Commands need exclusive access, so each system gets its own batch.
        assert_eq!(scheduler.execution_graph().len(), 3);
    }

    /// Commands systems and read-only systems interleave without sharing a
    /// batch.
    #[test]
    fn commands_and_readers_interleave_without_sharing_a_batch() {
        // The systems read A, use Commands, read B and use Commands, in that
        // order.
        let mut read_a = SystemAccess::new();
        read_a.add_read(ComponentId::of::<TestComponentA>());
        let mut commands_first = SystemAccess::new();
        commands_first.set_uses_commands(true);
        let mut read_b = SystemAccess::new();
        read_b.add_read(ComponentId::of::<TestComponentB>());
        let mut commands_second = SystemAccess::new();
        commands_second.set_uses_commands(true);
        let accesses = vec![read_a, commands_first, read_b, commands_second];

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 4);
    }

    /// An empty system list produces an empty execution graph.
    #[test]
    fn an_empty_system_list_gives_an_empty_graph() {
        let scheduler = scheduler_for(&[]);

        assert_eq!(scheduler.execution_graph().len(), 0);
    }

    /// A single system produces one batch that holds it.
    #[test]
    fn a_single_system_forms_one_batch() {
        let mut writes_a = SystemAccess::new();
        writes_a.add_write(ComponentId::of::<TestComponentA>());
        let accesses = vec![writes_a];

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 1);

        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 1);
    }

    /// A chain of read-write dependencies (A → B → C) gets one batch per link.
    #[test]
    fn a_three_system_chain_gets_one_batch_per_link() {
        // The chain: write A, then read A and write B, then read B and write C.
        let mut writes_a = SystemAccess::new();
        writes_a.add_write(ComponentId::of::<TestComponentA>());

        let mut reads_a_writes_b = SystemAccess::new();
        reads_a_writes_b.add_read(ComponentId::of::<TestComponentA>());
        reads_a_writes_b.add_write(ComponentId::of::<TestComponentB>());

        let mut reads_b_writes_c = SystemAccess::new();
        reads_b_writes_c.add_read(ComponentId::of::<TestComponentB>());
        reads_b_writes_c.add_write(ComponentId::of::<TestComponentC>());

        let accesses = vec![writes_a, reads_a_writes_b, reads_b_writes_c];
        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 3);
        assert_batches_follow_registration_order(&scheduler, &accesses);

        // Each system conflicts with the one before it, so each link needs its
        // own batch. First-fit would have let the third system join the first
        // batch and run before the second, which reads B and so must come first.
        assert_eq!(
            scheduler.execution_graph().to_vec(),
            vec![vec![0], vec![1], vec![2]],
            "every link of the chain runs after the one it depends on"
        );
    }

    /// A later system never runs before an earlier one it conflicts with, even
    /// when the two touch no component in common and a third system sits
    /// between them.
    #[test]
    fn a_later_system_never_lands_before_an_earlier_conflicting_system() {
        // System 0 writes X and touches nothing else.
        let mut writes_x = SystemAccess::new();
        writes_x.add_write(ComponentId::of::<TestComponentA>());

        // System 1 reads X and writes Y: it conflicts with 0 through X.
        let mut reads_x_writes_y = SystemAccess::new();
        reads_x_writes_y.add_read(ComponentId::of::<TestComponentA>());
        reads_x_writes_y.add_write(ComponentId::of::<TestComponentB>());

        // System 2 reads Y: it conflicts with 1 through Y, and with 0 through
        // nothing, which is exactly the pair first-fit used to reorder.
        let mut reads_y = SystemAccess::new();
        reads_y.add_read(ComponentId::of::<TestComponentB>());

        let accesses = vec![writes_x, reads_x_writes_y, reads_y];
        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 3);
        assert_batches_follow_registration_order(&scheduler, &accesses);

        assert_eq!(
            scheduler.execution_graph().to_vec(),
            vec![vec![0], vec![1], vec![2]],
            "the Y reader runs after the Y writer, not beside the X writer"
        );
    }

    /// Each batched phase gets its own graph: a conflict across phases costs no
    /// batch inside either of them, because the two never run beside each other.
    #[test]
    fn a_conflict_across_phases_costs_no_batch() {
        // The pre-project system writes A; the project system reads it. They
        // conflict, but the phases run one after the other.
        let mut writes_a = SystemAccess::new();
        writes_a.add_write(ComponentId::of::<TestComponentA>());
        let mut reads_a = SystemAccess::new();
        reads_a.add_read(ComponentId::of::<TestComponentA>());

        let entries = [
            (&writes_a, RunPhase::PreProject),
            (&reads_a, RunPhase::Project),
        ];
        let mut scheduler = SystemScheduler::new();
        scheduler.ensure_execution_graphs(entries.into_iter());

        assert_eq!(
            scheduler.execution_graph_of(RunPhase::PreProject).to_vec(),
            vec![vec![0]],
            "the pre-project phase holds its own system in one batch"
        );
        assert_eq!(
            scheduler.execution_graph_of(RunPhase::Project).to_vec(),
            vec![vec![1]],
            "the project phase is not split by the earlier phase's conflict"
        );
        assert!(
            scheduler
                .execution_graph_of(RunPhase::PostProject)
                .is_empty(),
            "the post-project phase runs sequentially and has no batches"
        );
    }

    /// A condition's reads, merged into its system's access, conflict exactly
    /// like the system's own: a writer of what the condition reads must not
    /// share a batch with it.
    #[test]
    fn a_merged_condition_access_conflicts_like_the_system_own() {
        // The system writes B and reads nothing; its condition reads A.
        let mut system_access = SystemAccess::new();
        system_access.add_write(ComponentId::of::<TestComponentB>());
        let mut condition_access = SystemAccess::new();
        condition_access.add_read(ComponentId::of::<TestComponentA>());
        system_access.merge(&condition_access);

        // A writer of A is now a conflict through the condition's read.
        let mut writes_a = SystemAccess::new();
        writes_a.add_write(ComponentId::of::<TestComponentA>());
        assert!(
            system_access.conflicts_with(&writes_a),
            "the condition's read has to count for batching"
        );

        // A reader of A is not, and the system's own write still conflicts.
        let mut reads_a = SystemAccess::new();
        reads_a.add_read(ComponentId::of::<TestComponentA>());
        assert!(!system_access.conflicts_with(&reads_a));
        let mut writes_b = SystemAccess::new();
        writes_b.add_write(ComponentId::of::<TestComponentB>());
        assert!(system_access.conflicts_with(&writes_b));
    }

    /// Five systems writing five different components form one batch.
    #[test]
    fn five_writers_of_five_different_components_share_a_batch() {
        // Five systems, each writing a component no other one touches.
        let accesses: Vec<SystemAccess> = [
            ComponentId::of::<TestComponentD>(),
            ComponentId::of::<TestComponentE>(),
            ComponentId::of::<TestComponentC>(),
            ComponentId::of::<TestComponentF>(),
            ComponentId::of::<TestComponentG>(),
        ]
        .into_iter()
        .map(|component_id| {
            let mut access = SystemAccess::new();
            access.add_write(component_id);
            access
        })
        .collect();

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 5);

        // No two of them touch the same component, so all five share one batch.
        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 5);
    }

    // -------------------------------------------------------------------------
    // Resource Conflict Tests
    // -------------------------------------------------------------------------

    /// Two systems reading the same resource run in parallel.
    #[test]
    fn two_readers_of_one_resource_share_a_batch() {
        // Both systems read the same resource.
        let accesses: Vec<SystemAccess> = (0..2)
            .map(|_| {
                let mut access = SystemAccess::new();
                access.add_resource_read(ResourceId::of::<TestResourceA>());
                access
            })
            .collect();

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 2);

        // Two readers never conflict, so both share one batch.
        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 2);
    }

    /// Two systems writing the same resource run one after the other.
    #[test]
    fn two_writers_of_one_resource_get_separate_batches() {
        // Both systems write the same resource.
        let accesses: Vec<SystemAccess> = (0..2)
            .map(|_| {
                let mut access = SystemAccess::new();
                access.add_resource_write(ResourceId::of::<TestResourceA>());
                access
            })
            .collect();

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 2);

        // Two writers of one resource can never share a batch.
        assert_eq!(scheduler.execution_graph().len(), 2);
    }

    /// A reader and a writer of the same resource conflict.
    #[test]
    fn a_resource_reader_and_writer_get_separate_batches() {
        // The first system reads the resource, the second writes it.
        let mut reads = SystemAccess::new();
        reads.add_resource_read(ResourceId::of::<TestResourceA>());
        let mut writes = SystemAccess::new();
        writes.add_resource_write(ResourceId::of::<TestResourceA>());
        let accesses = vec![reads, writes];

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 2);

        // A reader and a writer of one resource can never share a batch.
        assert_eq!(scheduler.execution_graph().len(), 2);
    }

    /// Writers of different resources run in parallel.
    #[test]
    fn writers_of_different_resources_share_a_batch() {
        // The systems write different resources.
        let mut writes_a = SystemAccess::new();
        writes_a.add_resource_write(ResourceId::of::<TestResourceA>());
        let mut writes_d = SystemAccess::new();
        writes_d.add_resource_write(ResourceId::of::<TestResourceD>());
        let accesses = vec![writes_a, writes_d];

        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 2);

        // Writing different resources is no conflict, so both share one batch.
        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 2);
    }

    /// Component and resource conflicts are both found in one graph.
    #[test]
    fn component_and_resource_conflicts_are_detected_together() {
        // The first system reads component A and writes resource C.
        let mut reads_a_writes_resource_c = SystemAccess::new();
        reads_a_writes_resource_c.add_read(ComponentId::of::<TestComponentA>());
        reads_a_writes_resource_c.add_resource_write(ResourceId::of::<TestResourceC>());

        // The second reads component A and resource C.
        let mut reads_a_and_resource_c = SystemAccess::new();
        reads_a_and_resource_c.add_read(ComponentId::of::<TestComponentA>());
        reads_a_and_resource_c.add_resource_read(ResourceId::of::<TestResourceC>());

        // The third writes component B and reads resource B, so it has no
        // conflict with either of the others.
        let mut writes_b_reads_resource_b = SystemAccess::new();
        writes_b_reads_resource_b.add_write(ComponentId::of::<TestComponentB>());
        writes_b_reads_resource_b.add_resource_read(ResourceId::of::<TestResourceB>());

        let accesses = vec![
            reads_a_writes_resource_c,
            reads_a_and_resource_c,
            writes_b_reads_resource_b,
        ];
        let scheduler = scheduler_for(&accesses);

        assert_no_batch_conflicts(&scheduler, &accesses);
        assert_all_systems_scheduled(&scheduler, 3);

        // The first two conflict on resource C, one writing it and one reading
        // it. The third conflicts with neither, so at least two batches result.
        assert!(scheduler.execution_graph().len() >= 2);
    }

    // -------------------------------------------------------------------------
    // Empirical verification: exhaustive enumeration and random fuzzing
    // -------------------------------------------------------------------------
    //
    // These are empirical checks, not a mathematical proof. They run the
    // scheduler on a large but finite set of inputs that covers every conflict
    // category (components, resources, Commands and combinations of them). That
    // makes it very unlikely that the scheduler ever puts conflicting systems
    // in one batch.

    /// Models every conflict category the scheduler handles.
    ///
    /// Two components (A and B) and two resources (X and Y) cover read/write
    /// conflicts for components and resources alike, plus Commands and a no-op
    /// system.
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum AccessKind {
        None,      // accesses nothing - free to run in any batch
        ReadA,     // reads component A
        WriteA,    // writes component A - conflicts with any other ReadA or WriteA
        ReadB,     // reads component B
        WriteB,    // writes component B - conflicts with any other ReadB or WriteB
        Commands,  // uses Commands - conflicts with everything
        ResReadX,  // reads resource X
        ResWriteX, // writes resource X - conflicts with any other ResReadX or ResWriteX
        ResReadY,  // reads resource Y
        ResWriteY, // writes resource Y - conflicts with any other ResReadY or ResWriteY
    }

    /// Converts an `AccessKind` into a `SystemAccess` the scheduler can process.
    ///
    /// Component A, component B, resource X and resource Y each use their own
    /// type, so the conflict check sees four distinct ids.
    fn kind_to_access(kind: AccessKind) -> SystemAccess {
        let mut access = SystemAccess::new();
        match kind {
            AccessKind::None => {}
            AccessKind::ReadA => access.add_read(ComponentId::of::<TestComponentD>()),
            AccessKind::WriteA => access.add_write(ComponentId::of::<TestComponentD>()),
            AccessKind::ReadB => access.add_read(ComponentId::of::<TestComponentE>()),
            AccessKind::WriteB => access.add_write(ComponentId::of::<TestComponentE>()),
            AccessKind::Commands => access.set_uses_commands(true),
            AccessKind::ResReadX => access.add_resource_read(ResourceId::of::<TestResourceB>()),
            AccessKind::ResWriteX => access.add_resource_write(ResourceId::of::<TestResourceB>()),
            AccessKind::ResReadY => access.add_resource_read(ResourceId::of::<TestResourceC>()),
            AccessKind::ResWriteY => access.add_resource_write(ResourceId::of::<TestResourceC>()),
        }
        access
    }

    /// Runs every possible combination of accesses for one to six systems.
    ///
    /// With 10 access kinds, one to six systems give 1,111,110 inputs
    /// (10 + 100 + 1,000 + 10,000 + 100,000 + 1,000,000). Each one is scheduled
    /// and checked for three properties:
    ///
    /// 1. No batch holds two systems that conflict (the safety guarantee).
    /// 2. Every system appears in exactly one batch, so none is dropped or
    ///    scheduled twice.
    /// 3. A conflicting pair keeps registration order: the later system runs in
    ///    a strictly later batch than the earlier one.
    ///
    /// The combinations are counted in base 10: counter 0 is [None, None, ...],
    /// counter 1 is [ReadA, None, ...], and so on. Every combination is covered,
    /// including ones where several systems share an access kind.
    #[test]
    fn every_combination_of_one_to_six_systems_has_no_conflicting_batch() {
        let kinds: Vec<AccessKind> = vec![
            AccessKind::None,
            AccessKind::ReadA,
            AccessKind::WriteA,
            AccessKind::ReadB,
            AccessKind::WriteB,
            AccessKind::Commands,
            AccessKind::ResReadX,
            AccessKind::ResWriteX,
            AccessKind::ResReadY,
            AccessKind::ResWriteY,
        ];

        // Run every combination for 1 to 6 systems.
        for system_count in 1..=6u32 {
            let combination_count = (kinds.len() as u32).pow(system_count);
            let mut kind_indices = vec![0u8; system_count as usize];

            // Decode the counter into one access kind per system.
            for counter in 0..combination_count {
                let mut counter_value = counter;
                // `i` is the digit position being decoded out of `counter`, so
                // the loop is about the position itself, not about walking a
                // slice. That is why the lint is allowed here.
                #[allow(clippy::needless_range_loop)]
                for i in 0..system_count as usize {
                    kind_indices[i] = (counter_value % kinds.len() as u32) as u8;
                    counter_value /= kinds.len() as u32;
                }

                // Build the accesses this combination describes.
                let accesses: Vec<SystemAccess> = kind_indices
                    .iter()
                    .map(|&kind_index| kind_to_access(kinds[kind_index as usize]))
                    .collect();
                let scheduler = scheduler_for(&accesses);

                // Check the properties above.
                assert_no_batch_conflicts(&scheduler, &accesses);
                assert_all_systems_scheduled(&scheduler, system_count as usize);
                assert_batches_follow_registration_order(&scheduler, &accesses);
            }
        }
    }

    /// Randomised fuzz testing for larger system counts.
    ///
    /// Exhaustive enumeration grows exponentially (10^20 cases at 20 systems),
    /// so this test samples instead: 500 seeds, each with a random system count
    /// from 1 to 20. Every graph is checked for the same two properties as the
    /// exhaustive test.
    ///
    /// The generator is a simple LCG (linear congruential generator) whose
    /// starting value is the seed hashed by a freshly seeded `RandomState`.
    /// That makes the sequences varied but not reproducible: a failure cannot
    /// be replayed from its seed alone.
    #[test]
    fn random_combinations_of_up_to_twenty_systems_have_no_conflicting_batch() {
        let kinds: Vec<AccessKind> = vec![
            AccessKind::None,
            AccessKind::ReadA,
            AccessKind::WriteA,
            AccessKind::ReadB,
            AccessKind::WriteB,
            AccessKind::Commands,
            AccessKind::ResReadX,
            AccessKind::ResWriteX,
            AccessKind::ResReadY,
            AccessKind::ResWriteY,
        ];

        for seed in 0..500u64 {
            // Hash the seed to get a starting value, then run an LCG
            let hasher = std::hash::RandomState::new();
            let hash = hasher.hash_one(seed);
            let mut generator_state = hash.wrapping_mul(6364136223846793005).wrapping_add(1);

            // Pick a random system count between 1 and 20
            let system_count = 1 + (generator_state % 20) as usize;
            generator_state = generator_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1);

            // Build a scheduler from the chosen access kinds
            let accesses: Vec<SystemAccess> = (0..system_count)
                .map(|_| {
                    let kind_index = (generator_state % kinds.len() as u64) as usize;
                    generator_state = generator_state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1);
                    kind_to_access(kinds[kind_index])
                })
                .collect();
            let scheduler = scheduler_for(&accesses);

            // Check the same three properties as the exhaustive test.
            assert_no_batch_conflicts(&scheduler, &accesses);
            assert_all_systems_scheduled(&scheduler, system_count);
            assert_batches_follow_registration_order(&scheduler, &accesses);
        }
    }
} // mod tests
