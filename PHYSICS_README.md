## Rapier Physics Integration

This engine now includes Rapier physics integration with the following components:

### Components

1. **PhysicsWorldComponent** (Global Component)
   - Manages the entire physics world
   - Contains rigid bodies, colliders, and physics pipeline
   - Automatically handles physics simulation stepping

2. **RigidBodyComponent**
   - Wraps a Rapier rigid body
   - Stores RigidBodyBuilder for configuration
   - Supports dynamic, kinematic, and static bodies

3. **ColliderComponent**
   - Wraps a Rapier collider
   - Stores ColliderBuilder for configuration
   - Supports various shapes and physics properties

### Systems

- **physics_system**: Main physics simulation system that:
  - Syncs transforms to physics bodies
  - Creates new physics objects
  - Steps the physics world
  - Syncs physics results back to transforms

### Usage Example

```rust
// 1. Register physics components
engine.register_global_component::<PhysicsWorldComponent>(PhysicsWorldComponent::new())?;
engine.register_component::<RigidBodyComponent>(scene)?;
engine.register_component::<ColliderComponent>(scene)?;

// 2. Add physics system to your update loop
engine.system_manager.add_system(UpdatePhase::Update, "physics", physics_system)?;

// 3. Create entities with physics
let entity = engine.create_entity(scene)?;

// Add transform (required for physics positioning)
engine.add_component_to_entity(scene, entity, TransformComponent::new())?;

// Add a dynamic rigid body
engine.add_component_to_entity(scene, entity, RigidBodyComponent::dynamic())?;

// Add a sphere collider
engine.add_component_to_entity(scene, entity, ColliderComponent::ball(1.0))?;

// Or create more complex physics objects:
let kinematic_body = RigidBodyComponent::builder()
    .body_type(RigidBodyType::KinematicPositionBased)
    .linear_damping(0.5)
    .build();

let custom_collider = ColliderComponent::builder(SharedShape::cuboid(0.5, 0.5, 0.5))
    .friction(0.7)
    .restitution(0.3)
    .density(2.0)
    .build();

engine.add_component_to_entity(scene, entity2, kinematic_body)?;
engine.add_component_to_entity(scene, entity2, custom_collider)?;
```

### Key Features

- **Simplified Design**: Components only store the Rapier builder and handle, letting Rapier manage the actual physics data
- **Transform Integration**: Automatically syncs with engine's TransformComponent
- **Flexible Configuration**: Use builders for complex physics setup or convenience methods for common cases
- **Performance**: Minimal overhead by storing only essential data in components

### Convenience Methods

**RigidBodyComponent**:
- `RigidBodyComponent::dynamic()` - Dynamic body
- `RigidBodyComponent::kinematic_position_based()` - Kinematic body
- `RigidBodyComponent::fixed()` - Static body

**ColliderComponent**:
- `ColliderComponent::cuboid(hx, hy, hz)` - Box collider
- `ColliderComponent::ball(radius)` - Sphere collider
- `ColliderComponent::capsule_y(half_height, radius)` - Capsule collider
- `ColliderComponent::cylinder(half_height, radius)` - Cylinder collider
