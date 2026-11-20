//! Turns egui's tessellated output into `pill_ui_frame` draw data: meshes,
//! clips and texture ids.
//!
//! # Responsibilities
//!
//! - [`convert`]: append egui's clipped primitives to a [`DrawList`] and
//!   return how many draws they became.
//! - [`external_id`] and [`straight_color`]: the two conversions the rest of
//!   the draw data needs - one for texture identity, one for colour space.
//!
//! # Design
//!
//! egui hands over triangle lists whose colours are premultiplied sRGB, in
//! points. `pill_ui_frame::Color` stores straight alpha instead, and the
//! renderer's UI painter blends with `SrcAlpha`, so this module divides the
//! alpha back out of every vertex colour. [`textures`](crate::textures) does
//! the same for each texel of the images it mirrors.
//!
//! Vertex positions are copied unchanged. Each finished list records the scale
//! factor of the monitor it was built for, and the painter multiplies by it
//! when it draws, so no vertex is scaled here.

// External crates
use egui::{ClippedPrimitive, Color32, TextureId as EguiTextureId};
use pill_core::math::Vector2f;
use pill_ui_frame::{Color, DrawCommand, DrawList, Rect, TextureId, UiVertex};

// =============================================================================
// Constants
// =============================================================================

/// The top bit of a `pill_ui_frame::TextureId`, which keeps egui's two
/// texture id spaces apart.
///
/// egui numbers managed textures and user textures in separate spaces, so
/// both start at 0. Managed ids carry this bit and user ids have it cleared,
/// so one number never names a texture of each kind.
const MANAGED_TEXTURE_ID_BIT: u64 = 1 << 63;

// =============================================================================
// Free Functions
// =============================================================================

/// Appends `primitives` to `list` and returns how many draws they added.
///
/// Each mesh becomes one clipped draw: its vertices and indices are copied
/// in, then a clip and a draw command are recorded around them. An empty mesh
/// adds nothing, and neither does a paint callback, which is a GPU closure
/// that a plain draw list cannot carry.
pub fn convert(primitives: &[ClippedPrimitive], list: &mut DrawList) -> usize {
    let mut draw_count = 0;
    for primitive in primitives {
        let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else {
            continue;
        };
        if mesh.indices.is_empty() {
            continue;
        }
        // Step 1: copy the mesh's vertices and indices into the list. The
        // indices move up by the number of vertices already there.
        let first_vertex_index = list.vertices.len() as u32;
        for vertex in &mesh.vertices {
            list.vertices.push(UiVertex {
                position: Vector2f::new(vertex.pos.x, vertex.pos.y),
                uv: Vector2f::new(vertex.uv.x, vertex.uv.y),
                color: straight_color(vertex.color),
            });
        }
        let first_index = list.indices.len() as u32;
        list.indices
            .extend(mesh.indices.iter().map(|index| index + first_vertex_index));
        // Step 2: record the clip this primitive carries and the draw that
        // uses the vertices it just added.
        list.push_clip(Rect {
            min: Vector2f::new(primitive.clip_rect.min.x, primitive.clip_rect.min.y),
            max: Vector2f::new(primitive.clip_rect.max.x, primitive.clip_rect.max.y),
        });
        list.commands.push(DrawCommand::Draw {
            texture: TextureId(external_id(mesh.texture_id)),
            first_index,
            index_count: mesh.indices.len() as u32,
        });
        list.pop_clip();
        draw_count += 1;
    }
    draw_count
}

/// Converts egui's texture id into the number the painter registers the
/// texture under.
pub fn external_id(texture: EguiTextureId) -> u64 {
    match texture {
        EguiTextureId::Managed(id) => MANAGED_TEXTURE_ID_BIT | (id & !MANAGED_TEXTURE_ID_BIT),
        EguiTextureId::User(id) => id & !MANAGED_TEXTURE_ID_BIT,
    }
}

/// Converts egui's premultiplied sRGB colour into a `pill_ui_frame::Color`
/// with straight alpha.
pub fn straight_color(color: Color32) -> Color {
    Color::from_rgba8(color.to_srgba_unmultiplied())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::Mesh;

    /// Builds a one-triangle primitive in egui's terms: three vertices and
    /// one texture.
    fn one_triangle_primitive(texture_id: EguiTextureId, color: Color32) -> ClippedPrimitive {
        let mut mesh = Mesh::with_texture(texture_id);
        for (x, y) in [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui::pos2(x, y),
                uv: egui::pos2(0.0, 0.0),
                color,
            });
        }
        mesh.indices = vec![0, 1, 2];
        ClippedPrimitive {
            clip_rect: egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0)),
            primitive: egui::epaint::Primitive::Mesh(mesh),
        }
    }

    #[test]
    fn one_mesh_becomes_one_clipped_draw() {
        let mut list = DrawList::new();
        let draw_count = convert(
            &[one_triangle_primitive(
                EguiTextureId::Managed(0),
                Color32::WHITE,
            )],
            &mut list,
        );
        assert_eq!(draw_count, 1);
        assert_eq!(list.vertices.len(), 3);
        assert_eq!(list.indices, vec![0, 1, 2]);
        assert_eq!(
            list.commands,
            vec![
                DrawCommand::PushClip(Rect {
                    min: Vector2f::new(0.0, 0.0),
                    max: Vector2f::new(10.0, 10.0),
                }),
                DrawCommand::Draw {
                    texture: TextureId(MANAGED_TEXTURE_ID_BIT),
                    first_index: 0,
                    index_count: 3,
                },
                DrawCommand::PopClip,
            ]
        );
    }

    #[test]
    fn a_second_mesh_offsets_its_indices() {
        let mut list = DrawList::new();
        let primitives = [
            one_triangle_primitive(EguiTextureId::Managed(0), Color32::WHITE),
            one_triangle_primitive(EguiTextureId::User(5), Color32::WHITE),
        ];
        let draw_count = convert(&primitives, &mut list);
        assert_eq!(draw_count, 2);
        assert_eq!(list.vertices.len(), 6);
        assert_eq!(list.indices, vec![0, 1, 2, 3, 4, 5]);
        match &list.commands[4] {
            DrawCommand::Draw {
                texture,
                first_index,
                index_count,
            } => {
                assert_eq!(*texture, TextureId(5));
                assert_eq!(*first_index, 3);
                assert_eq!(*index_count, 3);
            }
            other => panic!("expected the second draw, got {other:?}"),
        }
    }

    #[test]
    fn premultiplied_vertex_colour_is_unpremultiplied() {
        // Red at half alpha, in egui's premultiplied space.
        let premultiplied_color = Color32::from_rgba_premultiplied(128, 0, 0, 128);
        let straight_alpha_color = straight_color(premultiplied_color);
        assert!(
            (straight_alpha_color.r - 1.0).abs() < 0.01,
            "red comes back at full"
        );
        assert!((straight_alpha_color.a - 128.0 / 255.0).abs() < 0.01);
    }

    #[test]
    fn the_two_id_spaces_do_not_collide() {
        assert_eq!(external_id(EguiTextureId::Managed(0)), 1 << 63);
        assert_eq!(external_id(EguiTextureId::User(0)), 0);
        assert_eq!(external_id(EguiTextureId::User(7)), 7);
        assert_ne!(
            external_id(EguiTextureId::Managed(7)),
            external_id(EguiTextureId::User(7))
        );
    }

    #[test]
    fn an_empty_mesh_and_a_callback_add_nothing() {
        let mut empty_mesh = Mesh::with_texture(EguiTextureId::Managed(0));
        empty_mesh.vertices.clear();
        let primitives = [
            ClippedPrimitive {
                clip_rect: egui::Rect::NOTHING,
                primitive: egui::epaint::Primitive::Mesh(empty_mesh),
            },
            ClippedPrimitive {
                clip_rect: egui::Rect::NOTHING,
                primitive: egui::epaint::Primitive::Callback(egui::PaintCallback {
                    rect: egui::Rect::NOTHING,
                    callback: std::sync::Arc::new(()),
                }),
            },
        ];
        let mut list = DrawList::new();
        assert_eq!(convert(&primitives, &mut list), 0);
        assert!(list.is_empty());
    }
}
