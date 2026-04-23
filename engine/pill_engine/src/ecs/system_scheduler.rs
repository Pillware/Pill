// ============================================================================
// Parallel System Scheduler
// ============================================================================
//! Automatic dependency analysis and parallel system execution.
//!
//! This module analyzes component access patterns to build a dependency graph
//! and executes systems in parallel batches when safe to do so.
//!
//! ## How it works:
//! - When a system is registered, it reports its access pattern (which components it reads/writes, whether it uses Commands).
//! - The scheduler builds an execution graph that groups systems into batches that can run in
//!   parallel without conflicts (no read-write or write-write conflicts, and Commands require exclusive access).
//! - During frame processing and if parallel execution is enabled, the scheduler executes each batch in parallel using Rayon.
//! - Systems that use Commands are executed sequentially to ensure safe access to the World.
//! - The scheduler dependency analysis ensures that no two systems that access the same component in a conflicting way
//!   are run in parallel, preventing data races and ensuring thread safety.

use crate::ecs::component::ComponentId;
use std::collections::HashSet;

/// Component access information for a system
#[derive(Debug, Clone, Default)]
pub struct SystemAccess {
    /// Components read immutably (&T)
    pub reads: HashSet<ComponentId>,
    /// Components written mutably (&mut T)
    pub writes: HashSet<ComponentId>,
    /// Whether the system uses Commands (requires exclusive World access)
    pub uses_commands: bool,
}

impl SystemAccess {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_read(&mut self, component_id: ComponentId) {
        self.reads.insert(component_id);
    }

    pub fn add_write(&mut self, component_id: ComponentId) {
        self.writes.insert(component_id);
    }

    pub fn set_uses_commands(&mut self, uses: bool) {
        self.uses_commands = uses;
    }

    /// Check if this system conflicts with another
    ///
    /// Two systems conflict if:
    /// - Either uses Commands (Commands require exclusive access)
    /// - Both write to the same component (write-write conflict)
    /// - One writes and the other reads the same component (read-write conflict)
    ///
    /// Multiple systems can read the same component simultaneously (read-read is OK)
    pub fn conflicts_with(&self, other: &SystemAccess) -> bool {
        // Commands require exclusive World access
        if self.uses_commands || other.uses_commands {
            return true;
        }

        // Check for write-write conflicts
        if !self.writes.is_disjoint(&other.writes) {
            return true;
        }

        // Check for read-write conflicts (write on one side, read on the other)
        if !self.writes.is_disjoint(&other.reads) {
            return true;
        }

        if !self.reads.is_disjoint(&other.writes) {
            return true;
        }

        // No conflicts - systems can run in parallel
        false
    }
}

/// Execution scheduler that builds parallel batches from system dependencies
pub struct SystemScheduler {
    /// System count
    system_count: usize,
    /// Access patterns for each system
    access_patterns: Vec<SystemAccess>,
    /// Computed execution graph: Vec of batches, each batch contains system indices that can run in parallel
    execution_graph: Vec<Vec<usize>>,
}

impl SystemScheduler {
    /// Create a new scheduler
    pub fn new() -> Self {
        Self {
            system_count: 0,
            access_patterns: Vec::new(),
            execution_graph: Vec::new(),
        }
    }
}

impl Default for SystemScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemScheduler {
    /// Register a system with its access pattern
    pub fn register_system(&mut self, access: SystemAccess) -> usize {
        let index = self.system_count;
        self.access_patterns.push(access);
        self.system_count += 1;
        index
    }

    /// Build the execution graph based on dependencies
    ///
    /// # Algorithm
    ///
    /// Build the parallel execution graph using greedy batching.
    ///
    /// # Algorithm
    ///
    /// Uses a greedy approach that iterates through systems in registration order:
    /// 1. Start with an empty batch
    /// 2. For each unscheduled system:
    ///    - If it doesn't conflict with any system in the current batch, add it
    ///    - Otherwise, skip it for now
    /// 3. When no more systems can be added, finalize the batch
    /// 4. Repeat until all systems are scheduled
    ///
    /// # Limitations
    ///
    /// This greedy algorithm is O(n²) in the number of systems and may not produce
    /// the optimal (minimum) number of batches. For example, with systems A, B, C where:
    /// - A conflicts with B
    /// - B conflicts with C  
    /// - A does NOT conflict with C
    ///
    /// Registration order [A, B, C] produces: [[A, C], [B]] (2 batches, optimal)
    /// Registration order [B, A, C] produces: [[B], [A, C]] (2 batches, optimal)
    ///
    /// However, pathological orderings could produce suboptimal results. For most
    /// real-world system counts (<100), this is not a concern. For very large system
    /// counts, consider using topological sort with graph coloring.
    ///
    /// # Correctness
    ///
    /// Despite potential suboptimality, the algorithm is **always correct**: systems
    /// in the same batch are guaranteed to have non-conflicting access patterns.
    pub fn build_execution_graph(&mut self) {
        self.execution_graph.clear();

        let mut scheduled = vec![false; self.system_count];
        let mut scheduled_count = 0;

        while scheduled_count < self.system_count {
            let mut batch = Vec::new();

            // Try to add each unscheduled system to the current batch
            for (i, is_scheduled) in scheduled.iter_mut().enumerate() {
                if *is_scheduled {
                    continue;
                }

                // Check if this system conflicts with any system already in the batch
                let conflicts = batch
                    .iter()
                    .any(|&j| self.access_patterns[i].conflicts_with(&self.access_patterns[j]));

                if !conflicts {
                    batch.push(i);
                    *is_scheduled = true;
                    scheduled_count += 1;
                }
            }

            if !batch.is_empty() {
                self.execution_graph.push(batch);
            }
        }
    }

    /// Get the execution graph (batches of system indices)
    pub fn execution_graph(&self) -> &[Vec<usize>] {
        &self.execution_graph
    }

    /// Get access pattern for a system
    pub fn get_access(&self, index: usize) -> Option<&SystemAccess> {
        self.access_patterns.get(index)
    }

    /// Print execution graph for debugging
    pub fn print_execution_graph(&self, system_names: &[&str]) {
        println!("\n=== System Execution Graph ===");
        for (batch_idx, batch) in self.execution_graph.iter().enumerate() {
            println!("Batch {}: {} systems (parallel)", batch_idx, batch.len());
            for &sys_idx in batch {
                let name = system_names.get(sys_idx).unwrap_or(&"<unknown>");
                let access = &self.access_patterns[sys_idx];
                println!(
                    "  - {} (reads: {}, writes: {}, commands: {})",
                    name,
                    access.reads.len(),
                    access.writes.len(),
                    access.uses_commands
                );
            }
        }
        println!("==============================\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::component::ComponentId;
    use std::any::TypeId;

    // Helper: Verify that no batch contains conflicting systems
    fn assert_no_batch_conflicts(scheduler: &SystemScheduler) {
        for (batch_idx, batch) in scheduler.execution_graph().iter().enumerate() {
            for (i, &idx_a) in batch.iter().enumerate() {
                let access_a = scheduler.get_access(idx_a).unwrap();
                for &idx_b in &batch[i + 1..] {
                    let access_b = scheduler.get_access(idx_b).unwrap();
                    assert!(
                        !access_a.conflicts_with(access_b),
                        "Batch {} contains conflicting systems {} and {}!\n\
                         System {}: reads={:?}, writes={:?}, commands={}\n\
                         System {}: reads={:?}, writes={:?}, commands={}",
                        batch_idx,
                        idx_a,
                        idx_b,
                        idx_a,
                        access_a.reads,
                        access_a.writes,
                        access_a.uses_commands,
                        idx_b,
                        access_b.reads,
                        access_b.writes,
                        access_b.uses_commands
                    );
                }
            }
        }
    }

    // Helper: Verify all systems are scheduled exactly once
    fn assert_all_systems_scheduled(scheduler: &SystemScheduler, system_count: usize) {
        let mut scheduled = vec![false; system_count];
        for batch in scheduler.execution_graph() {
            for &idx in batch {
                assert!(!scheduled[idx], "System {} scheduled multiple times", idx);
                scheduled[idx] = true;
            }
        }
        for (idx, &was_scheduled) in scheduled.iter().enumerate() {
            assert!(was_scheduled, "System {} was not scheduled", idx);
        }
    }

    #[test]
    fn test_no_conflicts() {
        let mut scheduler = SystemScheduler::new();

        // System 1: reads A
        let mut access1 = SystemAccess::new();
        access1.add_read(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access1);

        // System 2: reads B
        let mut access2 = SystemAccess::new();
        access2.add_read(ComponentId(TypeId::of::<f32>()));
        scheduler.register_system(access2);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 2);

        // Both systems should be in the same batch (no conflicts)
        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 2);
    }

    #[test]
    fn test_write_conflict() {
        let mut scheduler = SystemScheduler::new();

        // System 1: writes A
        let mut access1 = SystemAccess::new();
        access1.add_write(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access1);

        // System 2: writes A
        let mut access2 = SystemAccess::new();
        access2.add_write(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access2);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 2);

        // Systems must be in different batches (write-write conflict)
        assert_eq!(scheduler.execution_graph().len(), 2);
    }

    #[test]
    fn test_read_write_conflict() {
        let mut scheduler = SystemScheduler::new();

        // System 1: reads A
        let mut access1 = SystemAccess::new();
        access1.add_read(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access1);

        // System 2: writes A
        let mut access2 = SystemAccess::new();
        access2.add_write(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access2);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 2);

        // Systems must be in different batches (read-write conflict)
        assert_eq!(scheduler.execution_graph().len(), 2);
    }

    #[test]
    fn test_commands_exclusive() {
        let mut scheduler = SystemScheduler::new();

        // System 1: uses commands
        let mut access1 = SystemAccess::new();
        access1.set_uses_commands(true);
        scheduler.register_system(access1);

        // System 2: reads A
        let mut access2 = SystemAccess::new();
        access2.add_read(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access2);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 2);

        // Systems must be in different batches (Commands require exclusive access)
        assert_eq!(scheduler.execution_graph().len(), 2);
    }

    #[test]
    fn test_multiple_readers_parallel() {
        let mut scheduler = SystemScheduler::new();

        // 5 systems all reading the same component
        for _ in 0..5 {
            let mut access = SystemAccess::new();
            access.add_read(ComponentId(TypeId::of::<i32>()));
            scheduler.register_system(access);
        }

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 5);

        // All readers can run in parallel (read-read is OK)
        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 5);
    }

    #[test]
    fn test_single_writer_blocks_all() {
        let mut scheduler = SystemScheduler::new();

        // 4 readers
        for _ in 0..4 {
            let mut access = SystemAccess::new();
            access.add_read(ComponentId(TypeId::of::<i32>()));
            scheduler.register_system(access);
        }

        // 1 writer of the same component
        let mut access = SystemAccess::new();
        access.add_write(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 5);

        // Writer must be in a separate batch from all readers
        assert!(scheduler.execution_graph().len() >= 2);
    }

    #[test]
    fn test_complex_dependency_graph() {
        let mut scheduler = SystemScheduler::new();

        // System 0: reads A, writes B
        let mut access0 = SystemAccess::new();
        access0.add_read(ComponentId(TypeId::of::<i32>()));
        access0.add_write(ComponentId(TypeId::of::<f32>()));
        scheduler.register_system(access0);

        // System 1: reads B, writes C
        let mut access1 = SystemAccess::new();
        access1.add_read(ComponentId(TypeId::of::<f32>()));
        access1.add_write(ComponentId(TypeId::of::<u32>()));
        scheduler.register_system(access1);

        // System 2: reads A (can run with system 1)
        let mut access2 = SystemAccess::new();
        access2.add_read(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access2);

        // System 3: reads C (can run with systems 0 and 2)
        let mut access3 = SystemAccess::new();
        access3.add_read(ComponentId(TypeId::of::<u32>()));
        scheduler.register_system(access3);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 4);
    }

    #[test]
    fn test_disjoint_component_sets() {
        let mut scheduler = SystemScheduler::new();

        // System 0: writes A
        let mut access0 = SystemAccess::new();
        access0.add_write(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access0);

        // System 1: writes B
        let mut access1 = SystemAccess::new();
        access1.add_write(ComponentId(TypeId::of::<f32>()));
        scheduler.register_system(access1);

        // System 2: writes C
        let mut access2 = SystemAccess::new();
        access2.add_write(ComponentId(TypeId::of::<u32>()));
        scheduler.register_system(access2);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 3);

        // All can run in parallel (disjoint writes)
        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 3);
    }

    #[test]
    fn test_multiple_commands_sequential() {
        let mut scheduler = SystemScheduler::new();

        // 3 systems all using commands
        for _ in 0..3 {
            let mut access = SystemAccess::new();
            access.set_uses_commands(true);
            scheduler.register_system(access);
        }

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 3);

        // Each command system must be in its own batch
        assert_eq!(scheduler.execution_graph().len(), 3);
    }

    #[test]
    fn test_mixed_commands_and_queries() {
        let mut scheduler = SystemScheduler::new();

        // System 0: reads A
        let mut access0 = SystemAccess::new();
        access0.add_read(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access0);

        // System 1: uses commands
        let mut access1 = SystemAccess::new();
        access1.set_uses_commands(true);
        scheduler.register_system(access1);

        // System 2: reads B
        let mut access2 = SystemAccess::new();
        access2.add_read(ComponentId(TypeId::of::<f32>()));
        scheduler.register_system(access2);

        // System 3: uses commands
        let mut access3 = SystemAccess::new();
        access3.set_uses_commands(true);
        scheduler.register_system(access3);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 4);
    }

    #[test]
    fn test_empty_scheduler() {
        let mut scheduler = SystemScheduler::new();
        scheduler.build_execution_graph();

        assert_eq!(scheduler.execution_graph().len(), 0);
    }

    #[test]
    fn test_single_system() {
        let mut scheduler = SystemScheduler::new();

        let mut access = SystemAccess::new();
        access.add_write(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 1);

        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 1);
    }

    #[test]
    fn test_chain_dependencies() {
        let mut scheduler = SystemScheduler::new();

        // Chain: System0 writes A -> System1 reads A, writes B -> System2 reads B, writes C
        let mut access0 = SystemAccess::new();
        access0.add_write(ComponentId(TypeId::of::<i32>()));
        scheduler.register_system(access0);

        let mut access1 = SystemAccess::new();
        access1.add_read(ComponentId(TypeId::of::<i32>()));
        access1.add_write(ComponentId(TypeId::of::<f32>()));
        scheduler.register_system(access1);

        let mut access2 = SystemAccess::new();
        access2.add_read(ComponentId(TypeId::of::<f32>()));
        access2.add_write(ComponentId(TypeId::of::<u32>()));
        scheduler.register_system(access2);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 3);

        // The greedy scheduler groups systems by conflict:
        // - System0 writes A, System2 writes C - no conflict, could be batched
        // - System1 reads A, writes B - conflicts with both
        // The exact number of batches depends on scheduling order,
        // but no conflicts should exist within any batch.
        // The greedy algorithm places them as: [0], [1], [2] = 3 batches
        // or could optimize to [0,2], [1] = 2 batches
        assert!(scheduler.execution_graph().len() >= 2);
        assert!(scheduler.execution_graph().len() <= 3);
    }

    #[test]
    fn test_large_parallel_batch() {
        // Test with 5 systems all accessing different components
        let mut scheduler = SystemScheduler::new();

        // System 0: writes type A
        let mut access = SystemAccess::new();
        access.add_write(ComponentId(TypeId::of::<u8>()));
        scheduler.register_system(access);

        // System 1: writes type B
        let mut access = SystemAccess::new();
        access.add_write(ComponentId(TypeId::of::<u16>()));
        scheduler.register_system(access);

        // System 2: writes type C
        let mut access = SystemAccess::new();
        access.add_write(ComponentId(TypeId::of::<u32>()));
        scheduler.register_system(access);

        // System 3: writes type D
        let mut access = SystemAccess::new();
        access.add_write(ComponentId(TypeId::of::<u64>()));
        scheduler.register_system(access);

        // System 4: writes type E
        let mut access = SystemAccess::new();
        access.add_write(ComponentId(TypeId::of::<i8>()));
        scheduler.register_system(access);

        scheduler.build_execution_graph();

        assert_no_batch_conflicts(&scheduler);
        assert_all_systems_scheduled(&scheduler, 5);

        // All can run in parallel
        assert_eq!(scheduler.execution_graph().len(), 1);
        assert_eq!(scheduler.execution_graph()[0].len(), 5);
    }
}
