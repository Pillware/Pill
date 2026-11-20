# Rust Hybrid ECS

An archetype-based Entity Component System (ECS) for Rust with Bevy-style
system parameters, automatic parallel scheduling, change detection, and
scriptable components.

## Features

| Feature                 | Description                                                                                                                                                    |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Archetype storage       | Components of the same type stored contiguously (Structure-of-Arrays) for cache-friendly bulk iteration                                                        |
| Bevy-style systems      | `fn my_system(q: Query<(&mut Transform, &Velocity)>, time: Res<ProjectTime>)` - parameters resolved automatically                                              |
| Automatic parallelism   | Scheduler builds a dependency graph from component/resource access patterns; systems with disjoint access run concurrently via Rayon                           |
| Change detection        | `Changed<T>` and `Added<T>` filters skip entities whose data hasn't changed since the system last ran                                                          |
| Deferred commands       | Structural changes (create/destroy entities, add/remove components) are queued during system execution and applied at the frame boundary - no mid-iteration UB |
| Script components       | Components with an `update()` method called every frame with safe, deferred-command-only World access                                                          |
| Resources               | Global singleton data (`ProjectTime`, `InputState`, `AssetStore`) accessed via `Res<T>` / `ResMut<T>` with scheduler-tracked access                            |
| Deterministic iteration | Queries and scripts iterate entities in a stable, sorted order regardless of HashMap layout                                                                    |

## Quick Start

```rust
use pill_engine::*;

// 1. Define components
#[derive(Debug, Clone)]
struct Position { x: f32, y: f32 }
impl Component for Position {}

#[derive(Debug, Clone)]
struct Velocity { vx: f32, vy: f32 }
impl Component for Velocity {}

// 2. Create the engine and register types
let mut engine = Engine::new();
engine.world_mut().register_component::<Position>();
engine.world_mut().register_component::<Velocity>();

// 3. Spawn entities
engine.world_mut()
    .create_entity()
    .with(Position { x: 0.0, y: 0.0 })
    .with(Velocity { vx: 1.0, vy: 0.5 })
    .build()
    .unwrap();

// 4. Define systems
fn movement(mut q: Query<(&mut Position, &Velocity)>) {
    for (mut pos, vel) in q.iter_mut() {
        pos.x += vel.vx;
        pos.y += vel.vy;
    }
}

fn debug_positions(q: Query<(Entity, &Position)>) {
    for (entity, pos) in q.iter_mut() {
        println!("Entity {} at ({}, {})", entity.id(), pos.x, pos.y);
    }
}

// 5. Register and run
engine.register_system("movement", movement);
engine.register_system("debug", debug_positions);

// Each frame:
engine.process_frame();
```

## System Parameters

Systems declare what they need as function parameters - the engine resolves them automatically:

```rust
fn example(
    mut q: Query<(&mut Transform, &Velocity), Changed<Transform>>,  // filtered query
    time: Res<ProjectTime>,          // immutable resource
    mut score: ResMut<Score>,     // mutable resource (change-tracked)
    mut commands: Commands,       // deferred entity operations
) {
    for (mut transform, vel) in q.iter_mut() {
        commands.create_entity().with(*transform).build();
    }
}
```

### Built-in filters

| Filter         | Effect                                                         |
| -------------- | -------------------------------------------------------------- |
| `()` (default) | All entities matching the data shape                           |
| `With<T>`      | Only entities that have component `T`                          |
| `Without<T>`   | Exclude entities that have component `T`                       |
| `Changed<T>`   | Only entities whose `T` was mutated since this system last ran |
| `Added<T>`     | Only entities whose `T` was added since this system last ran   |
| `Or<(A, B)>`   | Row matches if ANY inner filter matches                        |

## Resources

Global singleton data stored in the World, not attached to entities:

```rust
#[derive(Debug)]
struct ProjectTime { delta: f32, elapsed: f32 }
impl Resource for ProjectTime {}

// Insert
engine.world_mut().insert_resource(ProjectTime { delta: 0.016, elapsed: 0.0 });

// Access in systems
fn time_system(mut time: ResMut<ProjectTime>) {
    if let Some(mut t) = time.get_mut() {
        t.elapsed += t.delta;  // bumps changed tick via DerefMut
    }
}
```

Lightweight handles (`ResHandle<T>`) allow storing typed resource references
without borrowing the World:

```rust
let handle = ResHandle::<ProjectTime>::new();
let time = handle.get(&world).unwrap();
```

## Script Components

Components that self-update each frame with safe, deferred-command-only access:

```rust
#[derive(Debug, Clone)]
struct Rotator { speed: f32 }
impl Component for Rotator {}
impl ScriptComponent for Rotator {
    fn update(&mut self, ctx: &mut ScriptContext) {
        // Safe: reads from other components
        if let Some(transform) = ctx.get_component::<Transform>(ctx.get_owning_entity()) {
            // ...
        }
        // Safe: deferred commands
        if self.speed > 10.0 {
            ctx.destroy_entity(ctx.get_owning_entity());
        }
    }
}
```

## Parallel Execution

The scheduler automatically groups systems into parallel batches. Systems
with disjoint component/resource access run concurrently:

```
Batch 0: [movement,  scoring   ]   ← movement writes Position, scoring reads Position ✓
Batch 1: [physics              ]   ← writes Velocity (conflicts with nothing in batch 0)
Batch 2: [render               ]   ← uses Commands (always runs alone)
```

Enable/disable parallelism at runtime:
```rust
engine.set_parallel_execution(false);  // sequential for debugging
engine.set_parallel_execution(true);   // parallel (default)
```

## Change Detection

`Changed<T>` and `Added<T>` filters skip work on unchanged data without
manual dirty-flag bookkeeping:

```rust
fn render_changed(
    q: Query<(Entity, &Transform), Changed<Transform>>,
) {
    // Only entities whose Transform was mutated since last frame
    for (entity, transform) in q.iter_mut() {
        // ...
    }
}
```

Works for both components (via `Mut<T>` from `&mut T` queries) and
resources (via `ResMut<T>::get_mut()` returning `Mut<T>`).

## Project Structure

```
src/
├── archetype.rs       - Archetype storage, ArchetypeId
├── commands.rs        - Deferred CommandQueue, Commands system param
├── component.rs       - Component trait, ComponentId, ComponentMask, Tick
├── engine.rs          - Engine: system registration, frame loop, parallel runner
├── entity.rs          - Entity handle with generation-based recycling
├── lib.rs             - Public re-exports
├── main.rs            - Interactive example launcher
├── resource.rs        - Resource trait, ResourceId, ResHandle
├── scheduler.rs       - SystemScheduler: dependency analysis, batch building
├── scripting.rs       - ScriptComponent trait, ScriptContext
├── system.rs          - System trait, SystemParam, IntoSystem
├── world.rs           - World: central ECS state, entity/archetype/resource mgmt
├── query/
│   ├── mod.rs         - Module re-exports
│   ├── change_detection.rs - Mut<T> smart pointer with tick bumping
│   ├── filter.rs      - QueryFilter: With, Without, Changed, Added, Or
│   ├── iter.rs        - Sequential + parallel iterators, BatchStats
│   ├── ptr.rs         - SendPtr / SendPtrMut (thread-safe raw pointers)
│   ├── query.rs       - Query struct: iter_mut, par_iter_mut, first
│   ├── resource.rs    - Res / ResMut system parameters
│   ├── target.rs      - QueryTarget: &T, &mut T, Entity, tuple impls
│   └── tests.rs       - Query + change detection tests
└── examples/
    ├── change_detection_demo.rs
    ├── iterators_stress_test.rs
    ├── parallel_systems_demo.rs
    ├── resources_demo.rs
    └── scripting_demo.rs
```

## Frame Lifecycle

```
process_frame()
│
├─ increment_change_tick()          // advance global tick for change detection
├─ rebuild execution graph          // if systems were enabled/disabled
│
├─ Phase 1: Run systems
│   └─ parallel batches (or sequential if disabled)
│       └─ each system:
│           ├─ fetch system params (Query, Res, Commands, etc.)
│           ├─ user code executes
│           └─ queue deferred commands
│
├─ update_scripts()                 // call update() on all ScriptComponents
│
└─ Phase 2: execute_queued_commands() // apply all deferred structural changes
```

### Run mode

`Engine::run_mode()` says whether the frame is being edited or played. Every
build but the editor's is always `Playing`, so the gate below changes nothing
for a game. The editor starts in `Editing` and switches to `Playing` when the
person presses Play; while editing, only the systems declared
`#[system(..., run_in_edit_mode = true)]` run, and no script does.

`Engine::enter_play()` copies the world - entity storage, component rows and the
game-state resources - into a held slot, and `Engine::exit_play()` puts it back,
bumps `World::restore_epoch()` and drops everything the game did. A resource
says which side of that it is on with `Resource::play_state()`, and `AGENTS.md`
("Play mode") documents the three classes.

## Running

```bash
# Run the interactive demo
cargo run

# Run tests
cargo test

# Run a specific example
cargo run --example resources_demo
```

