//! Renderer data contract: sprite components, viewports and instance records.
//!
//! # Responsibilities
//!
//! - Defines [`Position`], [`Color`] and [`Sprite`]: the draw origin, the
//!   RGBA colour type, and the rectangle drawn at that origin.
//! - Registers their editor field layouts through [`register_components`], so
//!   the inspector can edit the fields of a world that registers them.
//! - Defines [`RenderViewport`] and [`VirtualResolution`], the plain-data
//!   description of where sprites are drawn and in what coordinate space.
//! - Collects a world's drawable entities into [`SpriteInstance`] records
//!   through [`sprite_instances`].
//!
//! # Design
//!
//! No `wgpu` type appears in this file. It is the data half of the renderer,
//! and [`crate::rasterizer`] is the pipeline that reads it: the rasterizer
//! consumes whole [`SpriteInstance`] records, so neither side needs a
//! `bytemuck` derive.
//!
//! This module is a deliberate copy of `pill_2d_renderer`'s, not a shared
//! dependency, so a display target never links wgpu just to name `Sprite`.
//! The two copies are one shared ABI: a hot-loaded project and the host give
//! the same type different `TypeId`s, so collection finds the columns by
//! stable type name and checked `repr(C)` size instead of using a query typed
//! by the host.

// External crates
use pill_engine::component::Component;
use pill_engine::world::World;
use pill_engine::PillLayout;

// =============================================================================
// Components
// =============================================================================

/// Position of an entity's draw origin in the project coordinate space.
///
/// One unit is one pixel until a [`VirtualResolution`] is set; the rasterizer
/// then scales the whole space into the physical viewport.
///
/// Plain `repr(C)` float pairs: a sprite's placement is the renderer's own
/// concern, so the type lives beside the pipeline that draws it rather than in
/// the ECS core. Its byte layout is what another binary binds against, since a
/// `TypeId` is computed per compilation unit.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, PillLayout)]
pub struct Position {
    /// Horizontal coordinate of the draw origin, in project units.
    pub x: f32,
    /// Vertical coordinate of the draw origin, in project units.
    pub y: f32,
}
impl Component for Position {}

/// An RGBA color with one float per channel, each in the 0.0-1.0 range.
///
/// The `#[repr(C)]` layout keeps the bytes identical when a project writes a
/// colour and the rasterizer reads it.
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
    /// Opaque white, and what [`Default`] returns for a colour.
    pub const WHITE: Color = Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

    /// Construct a color from its red, green, blue, and alpha channels.
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::WHITE
    }
}

/// Axis-aligned coloured rectangle drawn at an entity's [`Position`].
///
/// The quad spans `width` by `height` project units starting at the entity's
/// draw origin and is filled with `color`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PillLayout)]
pub struct Sprite {
    /// Quad width in project units.
    pub width: f32,
    /// Quad height in project units.
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

/// Register [`Position`], [`Color`] and [`Sprite`] with their editor layouts.
///
/// This is the one call a project makes to opt into sprite rendering.
/// Registration is idempotent (repeating it changes nothing), so a hot reload
/// that re-runs `init` is safe. Each type goes through
/// `register_component_with_layout`, which is what lets the inspector edit its
/// fields instead of showing one opaque blob.
///
/// `Sprite`'s layout comes from `#[derive(PillLayout)]`: the derive writes one
/// row for the nested `color` field, and the world expands that row into one
/// scalar row per channel, which the inspector draws as a colour group. The
/// engine's transform component is registered first, so the world also knows
/// the scene placement a 3D renderer reads.
pub fn register_components(world: &mut World) {
    pill_engine::register_transform_component(world);
    world.register_component_with_layout::<Position>(Position::FIELD_LAYOUT);
    world.register_component_with_layout::<Color>(Color::FIELD_LAYOUT);
    world.register_component_with_layout::<Sprite>(Sprite::FIELD_LAYOUT);
}

// =============================================================================
// Viewport
// =============================================================================

/// Physical-pixel rectangle within a render target.
///
/// Sprite positions are relative to this rectangle's top-left corner. The
/// rasterizer maps them into the rectangle and clips every span it draws to it.
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
    /// Construct a physical-pixel viewport rectangle.
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Construct a viewport covering an entire render target.
    pub const fn full(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    /// Clamp this rectangle to a render target, returning `None` when empty.
    pub fn clamped_to(self, target_width: u32, target_height: u32) -> Option<Self> {
        let x = self.x.min(target_width);
        let y = self.y.min(target_height);
        let width = self.width.min(target_width.saturating_sub(x));
        let height = self.height.min(target_height.saturating_sub(y));

        (width > 0 && height > 0).then_some(Self::new(x, y, width, height))
    }
}

/// Logical coordinate space mapped into a physical [`RenderViewport`].
///
/// Keeping this separate from the framebuffer's dimensions lets a caller keep a
/// stable coordinate system while the viewport it fills is resized. The
/// rasterizer performs the final scaling into the rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VirtualResolution {
    /// Horizontal extent of the project coordinate space.
    pub width: f32,
    /// Vertical extent of the project coordinate space.
    pub height: f32,
}

impl VirtualResolution {
    /// Construct a logical scene resolution.
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// Return whether both dimensions are finite and greater than zero, so
    /// the projection can use them.
    pub fn is_valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

// =============================================================================
// Instance data
// =============================================================================

/// One drawable entity, flattened for the renderer.
///
/// Plain `repr(C)` data with no GPU dependency: [`crate::rasterizer`] reads
/// the fields directly, so no `bytemuck` derive is needed.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteInstance {
    /// Top-left position in the project coordinate space.
    pub position: [f32; 2],
    /// Width and height in project units.
    pub size: [f32; 2],
    /// RGBA color.
    pub color: [f32; 4],
}

// =============================================================================
// Shared-ABI collection
// =============================================================================

/// Every drawable entity in `world`, flattened for a renderer.
///
/// A hot-loaded Rust project and the host executable can give the same
/// renderer type different `TypeId`s. The components are a deliberately
/// shared ABI, so their columns are found by stable type name and checked
/// `repr(C)` size, not by an ECS query typed by the host.
pub fn sprite_instances(world: &World) -> Vec<SpriteInstance> {
    sprite_instances_by_type_name(
        world,
        std::any::type_name::<Position>(),
        std::any::type_name::<Sprite>(),
    )
}

/// Implementation behind [`sprite_instances`], with the type names passed in.
///
/// Keeping the names as arguments lets a unit test use types from another
/// compilation unit, which is the case the shared-ABI lookup has to handle.
fn sprite_instances_by_type_name(
    world: &World,
    position_name: &str,
    sprite_name: &str,
) -> Vec<SpriteInstance> {
    let registry = world.component_registry();
    let mut instances = Vec::new();

    for archetype in world.archetypes_iter() {
        // Step 1: Find each column by shared type name and size among the
        // components present in this archetype. This also finds entities kept
        // from older DLL generations, whose component IDs differ from the IDs
        // this build assigns.
        let position_id = archetype.component_types.iter().copied().find(|id| {
            registry.get_name(id) == Some(position_name)
                && registry.get_size(id) == Some(std::mem::size_of::<Position>())
        });
        let sprite_id = archetype.component_types.iter().copied().find(|id| {
            registry.get_name(id) == Some(sprite_name)
                && registry.get_size(id) == Some(std::mem::size_of::<Sprite>())
        });
        let (Some(position_id), Some(sprite_id)) = (position_id, sprite_id) else {
            continue;
        };

        // Step 2: Get the type-erased column behind each component found above,
        // so rows can be read without a query typed by the host. Columns are
        // looked up by component id, so a column whose rows were created by
        // another binary is reached the same way as this crate's own.
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
        // the rasterizer. Both shared types are `#[repr(C)]` and `Copy`, and
        // their sizes were checked against this crate's `Position`/`Sprite`
        // above, so a row of either column holds a valid value of the target
        // type even when its `TypeId` came from another DLL. `row_count` stops
        // the loop at the shortest column.
        let row_count = archetype
            .entity_count()
            .min(position_storage.len())
            .min(sprite_storage.len());
        for row in 0..row_count {
            // Both pointers are in range: `row_count` was capped at each
            // column's length above.
            let (Some(position_row), Some(sprite_row)) =
                (position_storage.row_ptr(row), sprite_storage.row_ptr(row))
            else {
                continue;
            };
            // SAFETY: `row_ptr` returned a row inside the validated shared
            // column, and the name and size checks above make its bytes a
            // `Position`.
            let position = unsafe { read_shared_component::<Position>(position_row) };
            // SAFETY: as for the `Position` read directly above, for `Sprite`.
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

/// Copy one shared component row out of type-erased storage.
///
/// # Safety
///
/// `row_bytes` must point to a value with the same `repr(C)` layout and size
/// as `T`. Callers establish that by checking the shared component's name and
/// size. That is also why the row arrives as bytes: a shared component is
/// identified by its layout, so there is no `TypeId` for a typed accessor to
/// check.
unsafe fn read_shared_component<T: Copy>(row_bytes: *const u8) -> T {
    let typed_row = row_bytes.cast::<T>();
    // SAFETY: `row_bytes` points to a value of type `T`, as the caller
    // promised. `read_unaligned` is used because a shared row's alignment is
    // not checked, only its type name and size.
    unsafe { typed_row.read_unaligned() }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod shared_component_tests {
    use super::*;
    use pill_engine::component::ComponentId;
    use pill_engine::component_registry::ComponentFieldDescriptor;

    /// Layout-compatible stand-in with a different TypeId than Position.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ForeignPosition {
        x: f32,
        y: f32,
    }

    impl Component for ForeignPosition {}

    /// Layout-compatible stand-in with a different TypeId than Sprite.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ForeignSprite {
        width: f32,
        height: f32,
        color: Color,
    }

    impl Component for ForeignSprite {}

    /// Shared renderer layouts can be read even when their native TypeIds
    /// originate from a different compilation unit.
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

    /// The derive writes one row per field, and `register_components` attaches
    /// that layout with `color` expanded into channel rows. Together they make
    /// the renderer components editable in the inspector without any
    /// hand-written descriptors.
    #[test]
    fn the_sprite_layout_matches_its_struct_and_attaches_on_registration() {
        // Byte sizes and field offsets come straight from the compiler.
        assert_eq!(std::mem::size_of::<Position>(), 8);
        assert_eq!(std::mem::size_of::<Color>(), 16);
        assert_eq!(std::mem::size_of::<Sprite>(), 24);

        // The derive's own layout has one row per declared field. Here `color`
        // is still the nested struct; the world flattens it in the
        // registration below.
        assert_eq!(Sprite::FIELD_LAYOUT.len(), 3);
        let color_row = &Sprite::FIELD_LAYOUT[2];
        assert_eq!(color_row.name, "color");
        assert_eq!(color_row.type_tag, "struct:Color");
        assert_eq!(color_row.offset, std::mem::offset_of!(Sprite, color));
        assert_eq!(color_row.size, std::mem::size_of::<Color>());

        let assert_field_descriptors_match =
            |fields: &[ComponentFieldDescriptor], expected_fields: &[(&str, usize, usize)]| {
                assert_eq!(fields.len(), expected_fields.len());
                for (field, (name, offset, size)) in fields.iter().zip(expected_fields) {
                    assert_eq!(field.name, *name, "descriptor name for {name}");
                    assert_eq!(field.type_tag, "f32", "descriptor tag for {name}");
                    assert_eq!(field.offset, *offset, "descriptor offset for {name}");
                    assert_eq!(field.size, *size, "descriptor size for {name}");
                    assert_eq!(field.align, 4, "descriptor align for {name}");
                    assert_eq!(field.element_count, 0);
                }
            };

        // `register_components` is what makes them editable, because it calls
        // `register_component_with_layout` instead of the plain registration
        // path.
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

        // The world's copy of the `Sprite` layout has `color` expanded into one
        // scalar row per channel, at offsets that include the field's own
        // offset. That is the shape the inspector draws as one colour group.
        let sprite = world
            .component_field_layout(ComponentId::of::<Sprite>())
            .expect("Sprite registered with a layout");
        let color_offset = std::mem::offset_of!(Sprite, color);
        assert_field_descriptors_match(
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

    /// Registering twice changes nothing, which matters because a hot reload
    /// re-runs every `init`.
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
