//! Periodic ECS state report, opt-in through
//! [`Engine::set_ecs_diagnostics_interval`](crate::Engine::set_ecs_diagnostics_interval).
//!
//! # Responsibilities
//!
//! - Gather a snapshot of world state: entities, archetypes, columns, the
//!   component registry and resources.
//! - Render it as a fixed-width block for the periodic system to log.
//! - Supply the system name and the default interval a project registers the
//!   report with.
//!
//! # Design
//!
//! Nothing is registered while the engine is built: the setter is the only
//! call that registers the report. It runs as a normal scheduler system, so it
//! appears in the system list, has an owner like every other system, and can
//! be switched off. The setter registers it through
//! [`register_system_with_access`](crate::Engine::register_system_with_access)
//! with an access that declares it uses commands. That declaration makes the
//! system conflict with every other one, so the scheduler never puts it in a
//! parallel batch. This matters because the report reads the whole world, and
//! no [`SystemParam`](crate::system::SystemParam) can describe that read.
//!
//! Its owner, [`SystemOwner::ENGINE`](crate::SystemOwner::ENGINE), is one no
//! reload path clears, so the report survives project and module reloads. It
//! cannot see `pill_development_host`'s reload graveyard, where retired images
//! stay mapped while their rows migrate, so the recycled entity ID count and
//! the empty archetype count are the signs of a reload it can show.

// Standard library
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

// Current crate
use crate::archetype::ColumnIdentity;
use crate::component::ComponentId;
use crate::world::World;

// =============================================================================
// Constants
// =============================================================================

/// How often the report prints when a project turns it on.
///
/// This is a wall-clock interval, not a frame count. A frame count would print
/// at whatever rate the project happens to run at, while a duration prints at
/// the same rate in every project. It is the interval
/// [`Engine::set_ecs_diagnostics_interval`] is normally given; the engine
/// itself registers nothing until that setter is called.
///
/// [`Engine::set_ecs_diagnostics_interval`]: crate::Engine::set_ecs_diagnostics_interval
pub const DEFAULT_REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// The name the report system is registered under.
pub const SYSTEM_NAME: &str = "engine_ecs_diagnostics";

// =============================================================================
// ColumnReport
// =============================================================================

/// One component column stored in one archetype.
struct ColumnReport {
    /// The registry bit this component occupies. It is also the bit the
    /// component sets in the owning archetype's mask.
    component_bit: Option<u8>,
    /// Registered name of the component.
    name: String,
    /// Bytes one row of this column occupies.
    row_bytes: usize,
    /// Whether this column holds rows defined by another language, reached as
    /// raw bytes rather than through a Rust type.
    is_descriptor: bool,
}

// =============================================================================
// ArchetypeReport
// =============================================================================

/// One archetype's contribution to the report.
struct ArchetypeReport {
    /// The archetype's packed component mask. The mask is also the archetype's
    /// identity, so this value names it uniquely.
    component_mask_bits: u128,
    /// How many entities the archetype currently holds.
    entity_count: usize,
    /// Its columns, ordered by mask bit so they read in the same order as the
    /// mask's set bits.
    columns: Vec<ColumnReport>,
    /// Bytes all its columns occupy together.
    total_bytes: usize,
}

// =============================================================================
// EcsSnapshot
// =============================================================================

/// Everything the report shows, gathered in one pass over the world.
///
/// The values are collected into an owned structure instead of being formatted
/// as they are gathered. That ends the borrow of the world before anything is
/// printed, and it lets a test check the gathered values without capturing
/// stdout.
pub struct EcsSnapshot {
    /// Frame the snapshot was taken on.
    pub frame: u64,
    /// Seconds since startup.
    pub elapsed_seconds: f32,
    /// Milliseconds the previous frame took.
    pub last_frame_milliseconds: f32,
    /// Live entities across every archetype.
    pub entities: usize,
    /// Entity IDs retired and available for reuse.
    pub recycled_entity_ids: usize,
    /// Archetypes the world holds, including empty ones.
    pub archetypes: usize,
    /// Archetypes still allocated but holding no entities.
    pub empty_archetypes: usize,
    /// Component types registered, of the 128 a `ComponentMask` can hold.
    pub registered_components: usize,
    /// Registrations still available before the 128-type ceiling.
    pub available_component_slots: usize,
    /// Registered components that declare a cross-binary shared identity.
    pub shared_components: Vec<String>,
    /// Resources whose type declares a cross-binary shared identity, listed by
    /// declared name.
    ///
    /// No `TypeId` names a shared resource in every binary, so the name its
    /// type declared is the only thing there is to print.
    pub shared_resources: Vec<String>,
    /// Registered components defined by another language at runtime.
    pub descriptor_components: Vec<String>,
    /// Registered component types no archetype currently stores.
    pub unused_components: Vec<String>,
    /// The world's current change tick.
    pub change_tick: u32,
    /// Resources held by the world.
    pub resources: usize,
    /// Per-archetype detail, largest first.
    archetype_reports: Vec<ArchetypeReport>,
}

impl EcsSnapshot {
    /// Gathers the report from a world.
    ///
    /// This only reads: it never touches the change tick, so taking a report
    /// cannot make a `Changed<T>` filter fire.
    pub fn gather(world: &World) -> Self {
        let registry = world.component_registry();

        // Step 1: Name every registered component once, and note which ones
        // have an identity other than a plain per-binary `TypeId`.
        let mut component_names: BTreeMap<ComponentId, String> = BTreeMap::new();
        let mut shared_components = Vec::new();
        let mut descriptor_components = Vec::new();
        for (component_id, _bit, name) in registry.registered_components() {
            component_names.insert(component_id, name.to_string());
            match component_id {
                ComponentId::Shared(_) => shared_components.push(name.to_string()),
                ComponentId::Descriptor(_) => descriptor_components.push(name.to_string()),
                ComponentId::Native(_) => {}
            }
        }
        shared_components.sort();
        descriptor_components.sort();

        // Step 2: Visit every archetype. Add up the totals, build the
        // per-archetype detail, and record which components are stored
        // somewhere.
        let mut stored_component_ids: Vec<ComponentId> = Vec::new();
        let mut archetype_reports = Vec::new();
        let mut entities = 0;
        let mut empty_archetypes = 0;

        for archetype in world.archetypes_iter() {
            let entity_count = archetype.entities.len();
            entities += entity_count;
            if entity_count == 0 {
                empty_archetypes += 1;
            }

            let mut columns = Vec::with_capacity(archetype.component_types.len());
            let mut total_bytes = 0;
            for &component_id in &archetype.component_types {
                if !stored_component_ids.contains(&component_id) {
                    stored_component_ids.push(component_id);
                }
                // Count the bytes the rows really occupy, not the column's
                // capacity, so the total follows the entities rather than the
                // allocation. The descriptor marker is read from the column's
                // own identity, not from how the column was created.
                let (row_bytes, row_count, is_descriptor) =
                    if let Some(column) = archetype.component_storages.get(component_id) {
                        (
                            column.element_size(),
                            column.len(),
                            matches!(column.identity(), ColumnIdentity::Descriptor),
                        )
                    } else {
                        (0, 0, false)
                    };
                total_bytes += row_bytes * row_count;
                columns.push(ColumnReport {
                    component_bit: registry.get_bit(&component_id),
                    name: component_names
                        .get(&component_id)
                        .cloned()
                        .unwrap_or_else(|| format!("{component_id:?}")),
                    row_bytes,
                    is_descriptor,
                });
            }
            // Sorted by mask bit, so the listing reads in the same order as the
            // set bits of the archetype's id.
            columns.sort_by_key(|column| (column.component_bit, column.name.clone()));

            archetype_reports.push(ArchetypeReport {
                component_mask_bits: archetype.id.0,
                entity_count,
                columns,
                total_bytes,
            });
        }

        // Largest first, so the archetypes holding the most entities are the
        // ones read at the top.
        archetype_reports.sort_by_key(|report| std::cmp::Reverse(report.entity_count));

        // Step 3: A registered component that no archetype stores is worth
        // showing: either it was just registered, or a reload orphaned it.
        let mut unused_components: Vec<String> = component_names
            .iter()
            .filter(|(component_id, _)| !stored_component_ids.contains(component_id))
            .map(|(_, name)| name.clone())
            .collect();
        unused_components.sort();

        let (frame, elapsed_seconds, last_frame_milliseconds) =
            match world.get_resource::<crate::time::Time>() {
                Some(time) => (
                    time.frame_count(),
                    time.elapsed_seconds(),
                    time.last_frame_milliseconds(),
                ),
                None => (0, 0.0, 0.0),
            };

        // Step 4: Resources have no registry entry, so the shared ones are
        // identified by the names their types declared.
        let shared_resources = world.shared_resource_names();

        Self {
            frame,
            elapsed_seconds,
            last_frame_milliseconds,
            entities,
            recycled_entity_ids: world.recycled_entity_id_count(),
            archetypes: archetype_reports.len(),
            empty_archetypes,
            registered_components: registry.len(),
            available_component_slots: registry.available_slots(),
            shared_components,
            shared_resources,
            descriptor_components,
            unused_components,
            change_tick: world.change_tick().get(),
            resources: world.resource_count(),
            archetype_reports,
        }
    }

    /// Renders the snapshot as the block the engine prints.
    ///
    /// Returns a `String` instead of printing, so a caller can send the block
    /// anywhere it likes and a test can assert on it.
    pub fn render(&self) -> String {
        /// Inner width of the block, in characters.
        const WIDTH: usize = 74;

        // Every content line is padded to the same width, so the borders line
        // up whatever the content is. Widths tuned per field would drift as
        // soon as a number grew a digit.
        fn write_row(out: &mut String, content: &str) {
            let length = content.chars().count();
            if length >= WIDTH {
                let clipped: String = content.chars().take(WIDTH - 1).collect();
                let _ = writeln!(out, "│{clipped}…│");
            } else {
                let _ = writeln!(out, "│{content}{}│", " ".repeat(WIDTH - length));
            }
        }

        // Writes a line in two parts: `left` starts at the margin, `right`
        // sits against the far edge, and the gap between them takes up the
        // slack. Only the left side is ever cut short, because the right side
        // holds a number, and losing digits is worse than losing the start of
        // a name.
        fn write_split_row(out: &mut String, left: &str, right: &str) {
            // One space before the border, matching the margin the left side
            // starts from, so the number is not pressed against the edge.
            const RIGHT_MARGIN: usize = 1;
            let right_length = right.chars().count() + RIGHT_MARGIN;
            let available = WIDTH.saturating_sub(right_length + 2);
            let left_length = left.chars().count();
            let margin = " ".repeat(RIGHT_MARGIN);
            if left_length > available {
                let clipped: String = left.chars().take(available.saturating_sub(1)).collect();
                let _ = writeln!(out, "│{clipped}…  {right}{margin}│");
            } else {
                let padding = WIDTH - left_length - right_length;
                let _ = writeln!(out, "│{left}{}{right}{margin}│", " ".repeat(padding));
            }
        }

        let mut out = String::with_capacity(1024);
        let rule = "─".repeat(WIDTH);

        let _ = writeln!(out, "┌{rule}┐");
        write_row(
            &mut out,
            &format!(
                " ECS state · frame {} · {:.1}s elapsed · last frame {:.2} ms",
                self.frame, self.elapsed_seconds, self.last_frame_milliseconds
            ),
        );
        let _ = writeln!(out, "├{rule}┤");

        // Totals, the numbers worth seeing even when nothing else is read.
        write_row(
            &mut out,
            &format!(
                "  entities {:<8} archetypes {:<7} components {:<6} resources {}",
                self.entities, self.archetypes, self.registered_components, self.resources
            ),
        );
        write_row(
            &mut out,
            &format!(
                "  recycled ids {:<4} empty arch {:<7} free slots {:<6} tick {}",
                self.recycled_entity_ids,
                self.empty_archetypes,
                self.available_component_slots,
                self.change_tick
            ),
        );

        // Archetypes, largest first. A line above the list explains the hex id
        // and the bit of each column, so the per-archetype lines need no
        // legend of their own.
        let _ = writeln!(out, "├{rule}┤");
        if self.archetype_reports.is_empty() {
            write_row(&mut out, "  no archetypes");
        } else {
            write_row(
                &mut out,
                "  an archetype's id is its component mask; each column shows its bit",
            );
            for archetype in &self.archetype_reports {
                let _ = writeln!(out, "├{rule}┤");
                write_row(
                    &mut out,
                    &format!(
                        "  archetype 0x{:x} · {} {} · {} in {} column{}",
                        archetype.component_mask_bits,
                        archetype.entity_count,
                        if archetype.entity_count == 1 {
                            "entity"
                        } else {
                            "entities"
                        },
                        format_bytes(archetype.total_bytes),
                        archetype.columns.len(),
                        if archetype.columns.len() == 1 {
                            ""
                        } else {
                            "s"
                        },
                    ),
                );
                for column in &archetype.columns {
                    // Shows `rows × row size` instead of the product, so a
                    // column whose element is unexpectedly large is visible on
                    // its line.
                    let size = format!(
                        "{} × {}{}",
                        archetype.entity_count,
                        format_bytes(column.row_bytes),
                        if column.is_descriptor { " (desc)" } else { "" },
                    );
                    let bit_label = match column.component_bit {
                        Some(bit) => format!("bit {bit:>3}"),
                        None => "bit   ?".to_string(),
                    };
                    write_split_row(
                        &mut out,
                        &format!("    {bit_label}  {}", column.name),
                        &size,
                    );
                }
            }
        }

        // Only lists that have entries are shown, because an empty heading is
        // noise rather than information.
        let mut sections: Vec<(&str, &Vec<String>)> = Vec::new();
        if !self.shared_components.is_empty() {
            sections.push(("shared identity", &self.shared_components));
        }
        if !self.shared_resources.is_empty() {
            sections.push(("shared resources", &self.shared_resources));
        }
        if !self.descriptor_components.is_empty() {
            sections.push(("runtime-defined", &self.descriptor_components));
        }
        if !self.unused_components.is_empty() {
            sections.push(("registered, unused", &self.unused_components));
        }
        for (label, entries) in sections {
            let _ = writeln!(out, "├{rule}┤");
            write_row(&mut out, &format!("  {label}"));
            for name in entries {
                write_row(&mut out, &format!("    · {name}"));
            }
        }

        let _ = write!(out, "└{rule}┘");
        out
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Formats a byte count with a unit, so a column total reads at a glance.
fn format_bytes(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = KIB * 1024;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Component;
    use crate::World;

    #[derive(Clone, Debug)]
    struct Position {
        _x: f32,
        _y: f32,
    }
    impl Component for Position {}

    #[derive(Clone, Debug)]
    struct Velocity {
        _x: f32,
    }
    impl Component for Velocity {}

    /// An empty world reports zeroes instead of failing, and the block says so
    /// rather than printing an empty archetype list.
    #[test]
    fn an_empty_world_renders() {
        let world = World::new();
        let snapshot = EcsSnapshot::gather(&world);

        assert_eq!(snapshot.entities, 0);
        assert_eq!(snapshot.archetypes, 0);
        assert_eq!(snapshot.registered_components, 0);
        assert!(snapshot.render().contains("no archetypes"));
    }

    /// Totals and per-archetype detail reflect what the world holds, and every
    /// component name reaches the rendered block.
    #[test]
    fn the_snapshot_counts_what_the_world_holds() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();

        for index in 0..5 {
            world
                .create_entity()
                .with(Position {
                    _x: index as f32,
                    _y: 0.0,
                })
                .build()
                .unwrap();
        }
        // A second archetype: these carry both components.
        for _ in 0..3 {
            world
                .create_entity()
                .with(Position { _x: 0.0, _y: 0.0 })
                .with(Velocity { _x: 1.0 })
                .build()
                .unwrap();
        }

        let snapshot = EcsSnapshot::gather(&world);
        assert_eq!(snapshot.entities, 8);
        assert_eq!(snapshot.archetypes, 2);
        assert_eq!(snapshot.registered_components, 2);
        assert_eq!(snapshot.empty_archetypes, 0);
        assert!(snapshot.unused_components.is_empty());

        let rendered = snapshot.render();
        assert!(rendered.contains("Position"), "{rendered}");
        assert!(rendered.contains("Velocity"), "{rendered}");
        // Largest archetype first.
        let position_only = rendered.find("5 entities").expect("5-entity archetype");
        let both = rendered.find("3 entities").expect("3-entity archetype");
        assert!(position_only < both, "archetypes must be ordered by size");
    }

    /// A component that is registered but stored nowhere is called out, which
    /// is what an orphaned type looks like after a reload.
    #[test]
    fn a_registered_but_unstored_component_is_reported() {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();
        world
            .create_entity()
            .with(Position { _x: 0.0, _y: 0.0 })
            .build()
            .unwrap();

        let snapshot = EcsSnapshot::gather(&world);
        assert_eq!(snapshot.unused_components.len(), 1);
        assert!(snapshot.unused_components[0].ends_with("Velocity"));
        assert!(snapshot.render().contains("registered, unused"));
    }

    /// Every rendered line is the same width, so the block's borders line up
    /// in a terminal regardless of what the world contains.
    #[test]
    fn every_rendered_line_is_the_same_width() {
        let mut world = World::new();
        world.register_component::<Position>();
        world
            .create_entity()
            .with(Position { _x: 0.0, _y: 0.0 })
            .build()
            .unwrap();

        let rendered = EcsSnapshot::gather(&world).render();
        let widths: Vec<usize> = rendered.lines().map(|line| line.chars().count()).collect();
        assert!(!widths.is_empty());
        assert!(
            widths.iter().all(|width| *width == widths[0]),
            "line widths differ: {widths:?}\n{rendered}"
        );
    }

    /// Gathering the report must leave the world's change tick alone. The
    /// report system runs during the frame, and every `Changed<T>` filter
    /// compares row ticks against that counter.
    #[test]
    fn gathering_does_not_touch_the_change_tick() {
        let mut world = World::new();
        world.register_component::<Position>();
        world
            .create_entity()
            .with(Position { _x: 0.0, _y: 0.0 })
            .build()
            .unwrap();

        let before = world.change_tick();
        let _ = EcsSnapshot::gather(&world);
        assert_eq!(world.change_tick(), before);
    }

    /// A resource that declares a cross-binary shared identity, plus an
    /// ordinary one so the test can show the listing keeps only the shared one.
    #[derive(Debug, Default)]
    struct SharedSettings {
        _value: u32,
    }
    impl crate::resource::Resource for SharedSettings {
        fn shared_name() -> Option<&'static str> {
            Some("diagnostics_test::SharedSettings")
        }
    }

    #[derive(Debug, Default)]
    struct PlainSettings {
        _value: u32,
    }
    impl crate::resource::Resource for PlainSettings {}

    /// Only a shared resource is listed by name. An ordinary resource is known
    /// by its `TypeId`, which has no stable name to print.
    #[test]
    fn only_shared_resources_are_listed_by_name() {
        let mut world = World::new();
        world.insert_resource(SharedSettings::default());
        world.insert_resource(PlainSettings::default());

        let snapshot = EcsSnapshot::gather(&world);
        assert_eq!(
            snapshot.shared_resources,
            vec!["diagnostics_test::SharedSettings".to_string()]
        );
        assert!(snapshot.render().contains("shared resources"));

        // Dropping the shared resource also drops its claim, so the section
        // disappears instead of naming a resource the world no longer holds.
        let _ = world.remove_resource::<SharedSettings>();
        assert!(EcsSnapshot::gather(&world).shared_resources.is_empty());
    }
}
