//! The embedded renderer's proof project: a textured quad, a light, a UI panel
//! and debug lines, drawn on the processor at panel resolution.
//!
//! # Responsibilities
//!
//! - Build a textured quad, its material and its light in code, with a camera
//!   that looks straight at them.
//! - Publish a small UI panel into the world's UI slot and draw two debug lines
//!   every update.
//! - Report `[embedded_panel] ready` once the scene exists, so a headless run
//!   can be checked without a window.
//!
//! # Design
//!
//! Nothing here knows which renderer draws it. The project fills the world and
//! the UI slot the same way for either renderer; `renderer:` in
//! `project_settings.yaml` asks for the processor one, and the preview shows the
//! frame it produces. That is what makes this the proof that the two renderers
//! are interchangeable.
//!
//! The scene is built in `register`, and each part is only added when the world
//! does not have it yet, so a reload finds the running scene instead of a second
//! copy. Every log line starts with the project's folder name, which is the
//! convention the extension-example suite reads.

// Standard library
use std::sync::Arc;

// External crates
use pill_engine::{
    pill_project, system, DebugDraw, DebugStyle, Engine, Query, ResMut, SystemError, World,
};
use pill_rendering::{
    CameraComponent, DirectionalLightComponent, Material, Mesh, MeshRendererComponent, MeshVertex,
    Texture, TextureType, TransformComponent,
};
use pill_ui_frame::{
    Color, DrawCommand, DrawList, Rect, TextureId, UiDraw, UiFrame, UiFrameSlot, UiTextureFilter,
    UiTextureSet, UiVertex,
};

// =============================================================================
// Constants
// =============================================================================

/// Where the camera stands: on `+Z`, looking down `-Z` at the quad.
const CAMERA_TRANSLATION: [f32; 3] = [0.0, 0.0, 3.0];

/// The panel's UI: a translucent green bar across the top.
const PANEL_BAR: ([f32; 2], [f32; 2]) = ([20.0, 16.0], [220.0, 40.0]);

// =============================================================================
// Entry points
// =============================================================================

/// Builds the example's scene once, and registers its systems.
#[pill_project(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

/// Fills the world with the example's scene if it has none.
///
/// This is the half a reload repeats, so it must be idempotent.
pub fn register(engine: &mut Engine) -> u32 {
    match build_scene(engine.world_mut()) {
        Ok(()) => {
            pill_engine::tracing::info!("[embedded_panel] ready panel=240x240");
            0
        }
        Err(error) => {
            pill_engine::tracing::error!("[embedded_panel] scene not built: {error}");
            1
        }
    }
}

// =============================================================================
// Systems
// =============================================================================

/// Publishes the UI panel the renderer draws over the scene.
///
/// The frame is rebuilt each update because the renderer drains the slot every
/// frame. It is two quads: the bar and the glyph-sized text stand-in beside it.
#[system(name = "embedded_panel_ui")]
fn publish_ui(mut slot: ResMut<UiFrameSlot>) -> Result<(), SystemError> {
    let Some(mut slot) = slot.get_mut() else {
        return Ok(());
    };
    slot.publish(panel_ui_frame());
    Ok(())
}

/// Draws the example's debug lines again every update.
#[system(name = "embedded_panel_debug")]
fn draw_debug(mut debug_draw: ResMut<DebugDraw>) -> Result<(), SystemError> {
    if let Some(mut debug_draw) = debug_draw.get_mut() {
        debug_draw.line(
            [-1.0, -1.0, 0.0],
            [1.0, 1.0, 0.0],
            DebugStyle::colored([0.2, 1.0, 0.2, 1.0]),
        );
        debug_draw.line(
            [-1.0, 1.0, 0.0],
            [1.0, -1.0, 0.0],
            DebugStyle::colored([0.2, 0.4, 1.0, 1.0]),
        );
    }
    Ok(())
}

// =============================================================================
// Scene
// =============================================================================

/// Adds the quad, its material, the light, the camera and the UI slot.
///
/// # Errors
///
/// Returns the engine's message when an entity cannot be created.
fn build_scene(world: &mut World) -> Result<(), String> {
    // Step 1: The UI slot and the debug drawing resource, unless the engine
    // already holds them.
    if world.get_resource::<UiFrameSlot>().is_none() {
        world.insert_resource(UiFrameSlot::new());
    }
    if world.get_resource::<DebugDraw>().is_none() {
        world.insert_resource(DebugDraw::new(true));
    }

    // Step 2: The camera, unless the world already has one.
    if Query::<&CameraComponent>::new(world)
        .iter_mut()
        .next()
        .is_none()
    {
        world
            .create_entity()
            .with(CameraComponent::default())
            .with(TransformComponent {
                translation: CAMERA_TRANSLATION,
                ..Default::default()
            })
            .build()
            .map_err(|error| error.to_string())?;
    }

    // Step 3: The light. The processor renderer lights a face from its own key
    // light, so this component records the scene's light for the tools rather
    // than changing what is drawn.
    if Query::<&DirectionalLightComponent>::new(world)
        .iter_mut()
        .next()
        .is_none()
    {
        world
            .create_entity()
            .with(DirectionalLightComponent::default())
            .build()
            .map_err(|error| error.to_string())?;
    }

    // Step 4: The textured quad, unless something is already drawn.
    if Query::<&MeshRendererComponent>::new(world)
        .iter_mut()
        .next()
        .is_none()
    {
        let (mesh, material) = build_quad_assets(world)?;
        world
            .create_entity()
            .with(TransformComponent::default())
            .with(
                MeshRendererComponent::builder()
                    .mesh(&mesh)
                    .material(&material)
                    .build(),
            )
            .build()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Builds the quad, its two-colour texture and the material that draws them.
fn build_quad_assets(
    world: &mut World,
) -> Result<(pill_engine::Handle<Mesh>, pill_engine::Handle<Material>), String> {
    let assets = world
        .get_resource_mut::<pill_engine::AssetManager>()
        .ok_or("the engine AssetManager resource is missing")?;

    // Step 1: A two-colour texture, so the quad shows its mapping: the left
    // texel is orange and the right one blue.
    let texture = assets
        .add_named(
            "panel_texture",
            Texture::from_rgba(
                "panel_texture",
                TextureType::Color,
                vec![230, 120, 40, 255, 40, 90, 210, 255],
                2,
                1,
            )
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;

    // Step 2: The quad spans the camera's view at its distance, so it fills
    // the panel.
    let corner = |x: f32, y: f32, u: f32| MeshVertex {
        position: [x, y, 0.0],
        texture_coordinates: [u, 0.0],
        normal: [0.0, 0.0, 1.0],
        tangent: [1.0, 0.0, 0.0],
        bitangent: [0.0, 1.0, 0.0],
    };
    let half = 1.4;
    let mesh = assets
        .add_named(
            "panel_quad",
            Mesh::from_data(
                "panel_quad",
                vec![
                    corner(-half, half, 0.0),
                    corner(half, half, 1.0),
                    corner(half, -half, 1.0),
                    corner(-half, -half, 0.0),
                ],
                vec![0, 1, 2, 0, 2, 3],
            ),
        )
        .map_err(|error| error.to_string())?;

    // Step 3: The material tints the texture white, so the texels arrive as
    // authored.
    let material = assets
        .add_named(
            "panel_material",
            Material::builder("panel_material")
                .color_parameter("pbr_base", [1.0, 1.0, 1.0])
                .texture("base_color", &texture)
                .build(),
        )
        .map_err(|error| error.to_string())?;
    Ok((mesh, material))
}

// =============================================================================
// UI
// =============================================================================

/// The UI frame the renderer draws: one bar with a lighter block in it, over a
/// white texel so a solid fill needs no image.
fn panel_ui_frame() -> UiFrame {
    let mut textures = UiTextureSet::new();
    textures.push_full(
        TextureId(1),
        1,
        1,
        1,
        UiTextureFilter::Nearest,
        Arc::from(vec![255u8, 255, 255, 255]),
    );

    let mut list = DrawList::new();
    let (minimum, maximum) = PANEL_BAR;
    push_quad(
        &mut list,
        minimum,
        maximum,
        Color::rgba(0.1, 0.6, 0.2, 0.85),
    );
    push_quad(
        &mut list,
        [minimum[0] + 8.0, minimum[1] + 6.0],
        [minimum[0] + 96.0, minimum[1] + 18.0],
        Color::rgba(0.95, 0.95, 0.6, 0.95),
    );
    list.commands.push(DrawCommand::Draw {
        texture: TextureId(1),
        first_index: 0,
        index_count: list.indices.len() as u32,
    });

    UiFrame {
        draws: vec![UiDraw {
            surface: 0,
            scale_factor: 1.0,
            list,
        }],
        textures,
    }
}

/// Appends one solid-colour quad to `list`.
fn push_quad(list: &mut DrawList, minimum: [f32; 2], maximum: [f32; 2], color: Color) {
    let first = list.vertices.len() as u32;
    let vertex = |position: [f32; 2]| UiVertex {
        position: pill_core::math::Vector2f::new(position[0], position[1]),
        uv: pill_core::math::Vector2f::new(0.5, 0.5),
        color,
    };
    list.vertices.extend([
        vertex(minimum),
        vertex([maximum[0], minimum[1]]),
        vertex(maximum),
        vertex([minimum[0], maximum[1]]),
    ]);
    list.indices
        .extend([first, first + 1, first + 2, first, first + 2, first + 3]);
}

/// The UI panel's rectangle, as an engine rectangle, for a producer that clips.
pub fn panel_rect() -> Rect {
    Rect::from_min_size(
        pill_core::math::Vector2f::new(PANEL_BAR.0[0], PANEL_BAR.0[1]),
        pill_core::math::Vector2f::new(
            PANEL_BAR.1[0] - PANEL_BAR.0[0],
            PANEL_BAR.1[1] - PANEL_BAR.0[1],
        ),
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel's frame carries one draw list, one texture and a clipped bar.
    #[test]
    fn the_panel_frame_carries_its_lists_and_its_texture() {
        let frame = panel_ui_frame();

        assert_eq!(frame.draws.len(), 1);
        assert_eq!(frame.textures.textures.len(), 1);
        assert_eq!(frame.draws[0].list.indices.len(), 12);
        assert_eq!(frame.draws[0].surface, 0);
    }

    /// The panel's rectangle is the bar's own corners.
    #[test]
    fn the_panel_rectangle_is_the_bars_corners() {
        let rect = panel_rect();

        assert_eq!((rect.min.x, rect.min.y), PANEL_BAR.0);
        assert_eq!((rect.max.x, rect.max.y), PANEL_BAR.1);
    }

    /// Building the scene twice adds no second copy of anything, which is what
    /// a reload does.
    #[test]
    fn building_the_scene_twice_keeps_one_of_each_entity() {
        let mut engine = Engine::new();
        pill_rendering::register(&mut engine);

        build_scene(engine.world_mut()).expect("the first build");
        build_scene(engine.world_mut()).expect("the second build");

        let cameras = Query::<&CameraComponent>::new(engine.world_mut())
            .iter_mut()
            .count();
        let drawn = Query::<&MeshRendererComponent>::new(engine.world_mut())
            .iter_mut()
            .count();
        assert_eq!(cameras, 1);
        assert_eq!(drawn, 1);
    }
}
