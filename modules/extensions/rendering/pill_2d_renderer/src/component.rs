//! Renderer data contract: the sprite components and the viewport types.
//!
//! # Responsibilities
//!
//! - Defines [`Position`], [`Color`] and [`Sprite`], the components describing
//!   where and how to draw an entity as a colored rectangle.
//! - Publishes their editor field layouts through [`register_components`], so a
//!   world that registers them exposes their fields to the inspector.
//! - Defines [`RenderViewport`] and [`VirtualResolution`], the pure-data
//!   description of the area sprites are drawn into and the space they are
//!   placed in.
//! - Collects a world's drawable entities into [`SpriteInstance`] records
//!   through [`sprite_instances`].
//!
//! # Design
//!
//! No `wgpu` type appears here: this is the data half of the renderer, and
//! [`crate::sprite`] is the pipeline that consumes it. `sprite.rs` keeps its
//! `bytemuck` upload record separate and asserts the two layouts agree.
//!
//! The components live here, with the renderer that draws them, because a
//! sprite is a renderer concept. They are also a contract between binaries: a
//! hot-loaded project and the host give the same type different `TypeId`s, so
//! collection finds their columns by registered type name and size instead of
//! by a host-typed query.

// External crates
use pill_engine::component::Component;
use pill_engine::world::World;
use pill_engine::PillLayout;

// =============================================================================
// Components
// =============================================================================

/// Position of an entity's draw origin, in pixels, measured from the viewport's
/// top-left corner.
///
/// A plain `repr(C)` pair of floats. A sprite's placement is the renderer's own
/// concern, so the type lives beside the pipeline that draws it rather than in
/// the ECS core. Another binary binds against its byte layout, because a
/// `TypeId` is computed per compilation unit.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, PillLayout)]
pub struct Position {
    /// Horizontal pixel coordinate of the draw origin.
    pub x: f32,
    /// Vertical pixel coordinate of the draw origin.
    pub y: f32,
}
impl Component for Position {}

/// Plain RGBA color, with no backend type in it. Each channel is a float,
/// normally between 0.0 and 1.0.
///
/// `#[repr(C)]` with float channels, so a color a project writes and the
/// pipeline reads agrees byte for byte.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, PillLayout)]
pub struct Color {
    /// Red channel.
    #[pill(color)]
    pub r: f32,
    /// Green channel.
    #[pill(color)]
    pub g: f32,
    /// Blue channel.
    #[pill(color)]
    pub b: f32,
    /// Alpha channel.
    #[pill(color)]
    pub a: f32,
}
impl Component for Color {}

impl Color {
    /// Opaque white. This is also the value [`Default`] returns.
    pub const WHITE: Color = Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

    /// Constructs a color from its red, green, blue and alpha channels.
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::WHITE
    }
}

/// Axis-aligned colored rectangle drawn at an entity's [`Position`].
///
/// The quad spans `width` by `height` pixels starting at the entity's draw
/// origin and is filled with `color`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PillLayout)]
pub struct Sprite {
    /// Quad width in pixels.
    pub width: f32,
    /// Quad height in pixels.
    pub height: f32,
    /// Fill color of the quad.
    pub color: Color,
}
impl Component for Sprite {}

impl Default for Sprite {
    fn default() -> Self {
        Self {
            width: 16.0,
            height: 16.0,
            color: Color::WHITE,
        }
    }
}

/// Registers [`Position`], [`Color`] and [`Sprite`] with their editor layouts.
///
/// This is the one call a project makes to opt into sprite rendering. Running
/// it twice is safe, which is what a hot reload does when it re-runs `init`. It
/// uses `register_component_with_layout`, so the inspector shows each field as
/// an editable row instead of one opaque value.
///
/// `Sprite`'s layout comes from `#[derive(PillLayout)]`: one `color` row, which
/// the world expands into one scalar row per channel and the inspector draws as
/// a color group. The engine's transform is registered first, so an entity can
/// carry both the pixel placement this renderer reads and the scene placement a
/// 3D renderer would.
pub fn register_components(world: &mut World) {
    pill_engine::register_transform_component(world);
    world.register_component_with_layout::<Position>(Position::FIELD_LAYOUT);
    world.register_component_with_layout::<Color>(Color::FIELD_LAYOUT);
    world.register_component_with_layout::<Sprite>(Sprite::FIELD_LAYOUT);
}

// =============================================================================
// Viewport
// =============================================================================

/// A rectangle inside a render target, measured in physical pixels.
///
/// Sprite positions are relative to this rectangle's top-left corner. The GPU
/// viewport scales that local space into the rectangle, and a matching scissor
/// rectangle stops any pixel from being drawn outside it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderViewport {
    /// Left edge of the rectangle, in physical pixels.
    pub x: u32,
    /// Top edge of the rectangle, in physical pixels.
    pub y: u32,
    /// Horizontal extent of the rectangle, in physical pixels.
    pub width: u32,
    /// Vertical extent of the rectangle, in physical pixels.
    pub height: u32,
}

impl RenderViewport {
    /// Constructs a physical-pixel viewport rectangle.
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Constructs a viewport covering an entire render target.
    pub const fn full(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    /// Clamps this rectangle to a render target and returns `None` when the
    /// result would be empty.
    pub fn clamped_to(self, target_width: u32, target_height: u32) -> Option<Self> {
        let x = self.x.min(target_width);
        let y = self.y.min(target_height);
        let width = self.width.min(target_width.saturating_sub(x));
        let height = self.height.min(target_height.saturating_sub(y));

        (width > 0 && height > 0).then_some(Self::new(x, y, width, height))
    }
}

/// A logical coordinate space that is scaled into a physical [`RenderViewport`].
///
/// Keeping it separate from the swapchain size lets a caller keep one
/// coordinate system while the rectangle it fills is resized. The GPU viewport
/// does the final scaling into the rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VirtualResolution {
    /// Horizontal extent of the logical space.
    pub width: f32,
    /// Vertical extent of the logical space.
    pub height: f32,
}

impl VirtualResolution {
    /// Constructs a logical resolution.
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// Returns whether both dimensions can safely be used by the projection.
    pub fn is_valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

// =============================================================================
// Instance data
// =============================================================================

/// One drawable entity, flattened into the form the renderer uploads.
///
/// Plain `repr(C)` data with no GPU dependency. [`crate::sprite`] keeps a
/// matching `bytemuck` mirror of this record for the upload and checks that the
/// two layouts agree, so this type needs no `bytemuck` derive.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteInstance {
    /// Top-left position in pixels.
    pub position: [f32; 2],
    /// Width/height in pixels.
    pub size: [f32; 2],
    /// RGBA color.
    pub color: [f32; 4],
}

// =============================================================================
// Collection across binaries
// =============================================================================

/// Every drawable entity in `world`, flattened for a renderer.
///
/// A hot-loaded Rust project and the host executable can give the same renderer
/// type different `TypeId` values. These components are a deliberate shared
/// contract, so the columns are found by registered type name and size instead
/// of by a host-typed ECS query.
pub fn sprite_instances(world: &World) -> Vec<SpriteInstance> {
    sprite_instances_by_type_name(
        world,
        std::any::type_name::<Position>(),
        std::any::type_name::<Sprite>(),
    )
}

/// The type-erased implementation behind [`sprite_instances`].
///
/// It takes the component type names as arguments, so a unit test can pass the
/// names of stand-in types and exercise the cross-`TypeId` path.
fn sprite_instances_by_type_name(
    world: &World,
    position_type_name: &str,
    sprite_type_name: &str,
) -> Vec<SpriteInstance> {
    let component_registry = world.component_registry();
    let mut instances = Vec::new();

    for archetype in world.archetypes_iter() {
        // Step 1: Find the position and sprite columns in this archetype by
        // registered type name and size. This also covers entities created by
        // an older DLL generation, whose ids point at that image's copy of the
        // types.
        let position_id = archetype.component_types.iter().copied().find(|id| {
            component_registry.get_name(id) == Some(position_type_name)
                && component_registry.get_size(id) == Some(std::mem::size_of::<Position>())
        });
        let sprite_id = archetype.component_types.iter().copied().find(|id| {
            component_registry.get_name(id) == Some(sprite_type_name)
                && component_registry.get_size(id) == Some(std::mem::size_of::<Sprite>())
        });
        let (Some(position_id), Some(sprite_id)) = (position_id, sprite_id) else {
            continue;
        };

        // Step 2: Fetch the type-erased column behind each component, so rows
        // can be read without a host-typed query. Columns are keyed by
        // component id, so a column filled by another binary is reached exactly
        // like one of this crate's own.
        if !position_id.is_native_storage() || !sprite_id.is_native_storage() {
            continue;
        }
        let Some(position_storage) = archetype.component_storages.get(position_id) else {
            continue;
        };
        let Some(sprite_storage) = archetype.component_storages.get(sprite_id) else {
            continue;
        };

        // Step 3: Copy each row's component data into an instance record for
        // the GPU.
        //
        // SAFETY: `Position` and `Sprite` are `#[repr(C)]` and `Copy`, their
        // sizes were checked against this crate's types above, and every field
        // of both is an `f32`, so any bytes of that size form a valid value
        // even when the row was written by another DLL's copy of the type.
        // `row_count` caps the loop at every storage length, which keeps the
        // `row_ptr` accesses in bounds.
        let row_count = archetype
            .entity_count()
            .min(position_storage.len())
            .min(sprite_storage.len());
        for row_index in 0..row_count {
            let (Some(position_row), Some(sprite_row)) = (
                position_storage.row_ptr(row_index),
                sprite_storage.row_ptr(row_index),
            ) else {
                continue;
            };
            // SAFETY: `row_ptr` returned a row inside the checked column, so
            // `position_row` points at `size_of::<Position>()` readable bytes,
            // and any bytes of that size form a valid `Position` because all
            // its fields are `f32`.
            let position = unsafe { read_shared_component::<Position>(position_row) };
            // SAFETY: the same argument as the `Position` read above, for the
            // sprite row.
            let sprite = unsafe { read_shared_component::<Sprite>(sprite_row) };
            instances.push(SpriteInstance {
                position: [position.x, position.y],
                size: [sprite.width, sprite.height],
                color: [
                    sprite.color.r,
                    sprite.color.g,
                    sprite.color.b,
                    sprite.color.a,
                ],
            });
        }
    }

    instances
}

/// Copies one component row out of type-erased storage.
///
/// # Safety
///
/// `row_bytes` must point to at least `size_of::<T>()` readable bytes that form
/// a valid `T`. Callers establish this from the row's registered type name and
/// size, which is also why the row arrives as bytes: the same component carries
/// a different `TypeId` in each binary, so no typed accessor can check it.
unsafe fn read_shared_component<T: Copy>(row_bytes: *const u8) -> T {
    let typed_row = row_bytes.cast::<T>();
    // SAFETY: The caller guarantees the bytes at `row_bytes`. `read_unaligned`
    // also means the registry does not have to record the row's alignment.
    unsafe { typed_row.read_unaligned() }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::component::ComponentId;
    use pill_engine::component_registry::ComponentFieldDescriptor;

    /// Layout-compatible stand-in with a different `TypeId` than `Position`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ForeignPosition {
        x: f32,
        y: f32,
    }

    impl Component for ForeignPosition {}

    /// Layout-compatible stand-in with a different `TypeId` than `Sprite`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ForeignSprite {
        width: f32,
        height: f32,
        color: Color,
    }

    impl Component for ForeignSprite {}

    /// Renderer layouts can be read even when their native `TypeId`s come from
    /// a different compilation unit.
    #[test]
    fn collects_layout_compatible_components_with_foreign_type_ids() {
        let mut world = World::new();
        world.register_component::<ForeignPosition>();
        world.register_component::<ForeignSprite>();
        world
            .create_entity()
            .with(ForeignPosition { x: 12.0, y: 34.0 })
            .with(ForeignSprite {
                width: 56.0,
                height: 78.0,
                color: Color::new(0.1, 0.2, 0.3, 0.4),
            })
            .build()
            .unwrap();

        let instances = sprite_instances_by_type_name(
            &world,
            std::any::type_name::<ForeignPosition>(),
            std::any::type_name::<ForeignSprite>(),
        );

        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].position, [12.0, 34.0]);
        assert_eq!(instances[0].size, [56.0, 78.0]);
        assert_eq!(instances[0].color, [0.1, 0.2, 0.3, 0.4]);
    }

    /// Projection dimensions must be finite and strictly positive.
    #[test]
    fn virtual_resolution_rejects_invalid_projection_dimensions() {
        assert!(VirtualResolution::new(800.0, 600.0).is_valid());
        assert!(!VirtualResolution::new(0.0, 600.0).is_valid());
        assert!(!VirtualResolution::new(800.0, f32::NAN).is_valid());
        assert!(!VirtualResolution::new(f32::INFINITY, 600.0).is_valid());
    }

    /// The derive generates one row per field. `register_components` attaches
    /// that layout to the world, with `color` expanded into channel rows, so the
    /// inspector can edit the fields without any hand-written descriptors.
    #[test]
    fn the_sprite_layout_matches_its_struct_and_attaches_on_registration() {
        // Field offsets and byte sizes come from the compiler.
        assert_eq!(std::mem::size_of::<Position>(), 8);
        assert_eq!(std::mem::size_of::<Color>(), 16);
        assert_eq!(std::mem::size_of::<Sprite>(), 24);

        // The derive's own layout has one row per declared field. `color` is
        // still the nested struct here; the world flattens it further down.
        assert_eq!(Sprite::FIELD_LAYOUT.len(), 3);
        let color_row = &Sprite::FIELD_LAYOUT[2];
        assert_eq!(color_row.name, "color");
        assert_eq!(color_row.type_tag, "struct:Color");
        assert_eq!(color_row.offset, std::mem::offset_of!(Sprite, color));
        assert_eq!(color_row.size, std::mem::size_of::<Color>());

        let assert_field_descriptors =
            |fields: &[ComponentFieldDescriptor], expected: &[(&str, usize, usize)]| {
                assert_eq!(fields.len(), expected.len());
                for (field, (name, offset, size)) in fields.iter().zip(expected) {
                    assert_eq!(field.name, *name, "descriptor name for {name}");
                    assert_eq!(field.type_tag, "f32", "descriptor tag for {name}");
                    assert_eq!(field.offset, *offset, "descriptor offset for {name}");
                    assert_eq!(field.size, *size, "descriptor size for {name}");
                    assert_eq!(field.align, 4, "descriptor align for {name}");
                    assert_eq!(field.element_count, 0);
                }
            };

        // `register_components` is what makes the fields editable. It uses
        // `register_component_with_layout`, not the plain registration path.
        let mut world = World::new();
        register_components(&mut world);
        let position = world
            .component_field_layout(ComponentId::of::<Position>())
            .expect("Position registered with a layout");
        assert_eq!(position.len(), 2);
        let color = world
            .component_field_layout(ComponentId::of::<Color>())
            .expect("Color registered with a layout");
        assert_eq!(color.len(), 4);

        // The world's copy of the Sprite layout has `color` expanded into one
        // scalar row per channel, at offsets added up from the struct's own.
        // The inspector draws those rows as one color group.
        let sprite = world
            .component_field_layout(ComponentId::of::<Sprite>())
            .expect("Sprite registered with a layout");
        let color_offset = std::mem::offset_of!(Sprite, color);
        assert_field_descriptors(
            sprite,
            &[
                ("width", std::mem::offset_of!(Sprite, width), 4),
                ("height", std::mem::offset_of!(Sprite, height), 4),
                ("color.r", color_offset + std::mem::offset_of!(Color, r), 4),
                ("color.g", color_offset + std::mem::offset_of!(Color, g), 4),
                ("color.b", color_offset + std::mem::offset_of!(Color, b), 4),
                ("color.a", color_offset + std::mem::offset_of!(Color, a), 4),
            ],
        );
        assert_eq!(sprite.len(), 6);
    }

    /// Registering twice is safe, which is what a hot reload does when it
    /// re-runs `init`.
    #[test]
    fn register_components_is_idempotent() {
        let mut world = World::new();
        register_components(&mut world);
        register_components(&mut world);

        assert_eq!(
            world
                .component_field_layout(ComponentId::of::<Sprite>())
                .map(<[ComponentFieldDescriptor]>::len),
            Some(6)
        );
    }
}
