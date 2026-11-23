//! The embedded renderer: a [`PillRenderer`] that draws on the processor into
//! an RGB565 panel.
//!
//! # Responsibilities
//!
//! - Own the panel's framebuffer and the rasterizer that fills it
//!   ([`EmbeddedRenderer`]).
//! - Turn the frame's primary camera into the matrices the rasterizer draws
//!   through, at the panel's aspect ratio.
//! - Implement the renderer contract: resize, viewport, frame submission,
//!   asset invalidation, capabilities and the frame's statistics.
//! - Hold the panel's resolution ([`PanelSize`], [`DEFAULT_PANEL_SIZE`]).
//!
//! # Design
//!
//! The renderer draws at the panel's resolution, never at the window's: the
//! window is only where a preview shows the finished frame, scaled up. A frame
//! therefore looks the same on a 240x240 panel and in a 960x960 window, which
//! is the point of a preview.
//!
//! `resize_surface` records the window's size, because a surface the frontend
//! minimized draws nothing, but it never changes the panel. The panel comes
//! from [`EmbeddedRenderer::new`], defaulting to the 240x240 ST7789 the first
//! device build targets.
//!
//! There is no GPU state to keep in step with the assets: a mesh is read from
//! the store each frame and a texture is converted once per content version by
//! [`crate::texture_cache`]. `invalidate_assets` therefore only empties the
//! texture cache.
//!
//! The panel's memory is small enough to state: at the default 240x240 the
//! framebuffer and the depth buffer are 240 * 240 * 2 bytes each, 225 KiB
//! together, before any texture is cached.

// External crates
use pill_core::platform::Instant;
use pill_engine::AssetManager;
use pill_renderer_api::{
    FrameOutcome, PillRenderer, RenderCapabilities, RenderFrame, RenderMetrics, RenderViewport,
    RendererError, SurfaceId,
};
use pill_rendering_core::camera::CameraParameters;
use pill_rendering_core::frame::consumed_component_names;

// Current crate
use crate::debug_draw;
use crate::framebuffer::{pack_rgb565, Framebuffer};
use crate::rasterizer::{ScenePass, SceneRasterizer};
use crate::texture_cache::TextureCache;
use crate::ui::UiRasterizer;

// =============================================================================
// Constants
// =============================================================================

/// The resolution of the panel the first device build targets: a 240x240
/// ST7789.
pub const DEFAULT_PANEL_SIZE: PanelSize = PanelSize {
    width: 240,
    height: 240,
};

/// The colour an empty frame clears to, as sRGB channel values in
/// `0.0..=1.0`.
///
/// The dark blue-grey `#23282F`, the colour the GPU chain clears its surface
/// to, so an empty panel and an empty window look the same. Nothing is
/// converted to linear: the panel takes sRGB values as stored.
const BACKGROUND_SRGB: [f32; 3] = [35.0 / 255.0, 40.0 / 255.0, 47.0 / 255.0];

/// The variable that sets the panel's width in pixels.
const PANEL_WIDTH_VARIABLE: &str = "PILL_PANEL_WIDTH";

/// The variable that sets the panel's height in pixels.
const PANEL_HEIGHT_VARIABLE: &str = "PILL_PANEL_HEIGHT";

// =============================================================================
// PanelSize
// =============================================================================

/// The panel's pixel resolution.
///
/// The device's display decides it: a 240x240 ST7789 is the default because it
/// is the smallest panel the first device build targets. A larger panel costs
/// memory in proportion: the framebuffer and the depth buffer are two bytes per
/// pixel each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelSize {
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
}

impl Default for PanelSize {
    fn default() -> Self {
        DEFAULT_PANEL_SIZE
    }
}

/// Reads the panel's resolution from the environment, defaulting to
/// [`DEFAULT_PANEL_SIZE`].
///
/// A preview or a device frontend sets `PILL_PANEL_WIDTH` and
/// `PILL_PANEL_HEIGHT` to describe the display it drives; a value that is not a
/// positive whole number is ignored, so a typo leaves the default panel rather
/// than a buffer of no pixels.
pub fn panel_from_environment() -> PanelSize {
    let read = |variable: &str| -> Option<u32> {
        std::env::var(variable)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|value| *value > 0)
    };
    PanelSize {
        width: read(PANEL_WIDTH_VARIABLE).unwrap_or(DEFAULT_PANEL_SIZE.width),
        height: read(PANEL_HEIGHT_VARIABLE).unwrap_or(DEFAULT_PANEL_SIZE.height),
    }
}

// =============================================================================
// EmbeddedRenderer
// =============================================================================

/// Draws a frame's meshes into an RGB565 framebuffer, on the processor.
pub struct EmbeddedRenderer {
    /// The panel's resolution, which the framebuffer is sized to.
    panel: PanelSize,
    /// The frame being drawn.
    framebuffer: Framebuffer,
    /// The triangle rasterizer and its depth buffer.
    rasterizer: SceneRasterizer,
    /// The packed copies of the textures the frame samples.
    textures: TextureCache,
    /// The frame's UI draw lists and their textures.
    ui: UiRasterizer,
    /// The size of the window the frame is presented in, in physical pixels.
    ///
    /// A zero on either axis means the surface is minimized and no frame is
    /// drawn. The value never changes the panel.
    window_size: (u32, u32),
    /// The part of the panel the frame draws into, when the frontend asked for
    /// one.
    viewport: Option<RenderViewport>,
    /// The statistics of the most recent frame that drew.
    metrics: RenderMetrics,
}

impl EmbeddedRenderer {
    /// Builds a renderer for a panel of the given resolution.
    ///
    /// The window starts as the panel, so a frame can be drawn before a
    /// frontend reports a resize.
    pub fn new(panel: PanelSize) -> Self {
        let panel = PanelSize {
            width: panel.width.max(1),
            height: panel.height.max(1),
        };
        let framebuffer = Framebuffer::new(panel.width, panel.height);
        let window_size = (framebuffer.width(), framebuffer.height());
        Self {
            panel,
            framebuffer,
            rasterizer: SceneRasterizer::new(),
            textures: TextureCache::new(),
            ui: UiRasterizer::new(),
            window_size,
            viewport: None,
            metrics: RenderMetrics::default(),
        }
    }

    /// The panel's resolution.
    pub fn panel_size(&self) -> PanelSize {
        self.panel
    }

    /// The finished frame, for a frontend that shows it.
    pub fn framebuffer(&self) -> &Framebuffer {
        &self.framebuffer
    }

    /// The finished frame as mutable pixels, for a test.
    pub fn framebuffer_mut(&mut self) -> &mut Framebuffer {
        &mut self.framebuffer
    }

    /// The part of the panel this frame drew into.
    fn drawing_viewport(&self) -> RenderViewport {
        let full = RenderViewport::full(self.panel.width, self.panel.height);
        self.viewport
            .and_then(|viewport| viewport.clamped_to(self.panel.width, self.panel.height))
            .unwrap_or(full)
    }

    /// Clears the panel to the background colour.
    fn clear(&mut self) {
        let [red, green, blue] = BACKGROUND_SRGB;
        self.framebuffer.clear(pack_rgb565(red, green, blue));
    }
}

impl Default for EmbeddedRenderer {
    fn default() -> Self {
        Self::new(DEFAULT_PANEL_SIZE)
    }
}

impl PillRenderer for EmbeddedRenderer {
    fn capabilities(&self) -> RenderCapabilities {
        RenderCapabilities {
            // A processor reads a texture straight from memory, so the only
            // limit is the panel's own size. The host only compares the
            // consumed components, which is what a frontend uses to warn about
            // a component this renderer ignores.
            consumed_components: consumed_component_names(),
            ..RenderCapabilities::default()
        }
    }

    fn metrics(&self) -> RenderMetrics {
        self.metrics
    }

    fn resize_surface(&mut self, id: SurfaceId, width: u32, height: u32) {
        if id == SurfaceId::PRIMARY {
            self.window_size = (width, height);
        }
    }

    fn set_surface_viewport(&mut self, id: SurfaceId, viewport: Option<RenderViewport>) {
        if id == SurfaceId::PRIMARY {
            self.viewport = viewport;
        }
    }

    fn render(
        &mut self,
        frame: &RenderFrame,
        assets: &AssetManager,
    ) -> Result<FrameOutcome, RendererError> {
        // Step 1: A minimized surface, or a frame with no camera for the
        // primary surface, has nothing to present.
        if self.window_size.0 == 0 || self.window_size.1 == 0 {
            return Ok(FrameOutcome::Skipped);
        }
        let Some(view) = frame.surface_views.get(&SurfaceId::PRIMARY) else {
            return Ok(FrameOutcome::Skipped);
        };

        // Step 2: Build the camera's matrices at the panel's aspect ratio, not
        // the window's, because the panel is what the frame is drawn into.
        let prepare_started = Instant::now();
        let mut camera = CameraParameters::new();
        camera.update(
            &view.camera,
            &view.transform,
            self.panel.width as f32 / self.panel.height as f32,
        );
        let viewport = self.drawing_viewport();

        // Step 3: Clear and draw the scene.
        self.clear();
        let mut drew = self.rasterizer.render(
            &mut self.framebuffer,
            &mut self.textures,
            &ScenePass {
                view_projection: &camera.view_projection_matrix,
                viewport,
                camera_position: [camera.position.x, camera.position.y, camera.position.z],
                instances: &frame.instances,
                assets,
            },
        );

        // Step 4: Draw the UI the frame carries for this surface, then the
        // debug lines and labels over it. The UI textures are applied even
        // when no list draws this frame, so the next frame that draws one has
        // them.
        self.ui.apply_textures(&frame.ui.textures);
        let mut drew_ui = false;
        for draw in &frame.ui.draws {
            if draw.surface != SurfaceId::PRIMARY.0 {
                continue;
            }
            drew_ui |= self
                .ui
                .render(&mut self.framebuffer, draw, viewport, self.window_size);
        }
        let drew_debug = debug_draw::render(
            &mut self.framebuffer,
            &frame.debug,
            &camera.view_projection_matrix,
            viewport,
            self.rasterizer.depth(),
        );
        drew |= drew_ui || drew_debug;
        let prepare_micros = prepare_started.elapsed().as_micros() as u64;

        // Step 4: Record what the frame did, so the host can log it the way it
        // logs the GPU backend's stats.
        self.metrics = RenderMetrics {
            prepare_micros,
            submit_micros: 0,
            passes_with_draws: u32::from(drew),
            passes: frame.passes.len() as u32,
            instance_bytes: (frame.instances.len()
                * std::mem::size_of::<pill_renderer_api::RenderInstance>())
                as u64,
        };
        Ok(FrameOutcome::Presented)
    }

    fn invalidate_assets(&mut self) {
        self.textures.clear();
    }

    fn panel_frame(&self) -> Option<pill_renderer_api::PanelFrame<'_>> {
        Some(pill_renderer_api::PanelFrame {
            width: self.framebuffer.width(),
            height: self.framebuffer.height(),
            pixels: self.framebuffer.pixels(),
        })
    }
}
// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_engine::common_components::TransformComponent;
    use pill_engine::Entity;
    use pill_renderer_api::{AssetKey, CameraComponent, RenderInstance, SurfaceView};
    use pill_rendering::assets::{asset_key, Material, Mesh, MeshVertex, Texture, TextureType};
    use pill_rendering_core::materials;

    /// The panel's background as one packed pixel.
    fn background() -> u16 {
        let [red, green, blue] = BACKGROUND_SRGB;
        pack_rgb565(red, green, blue)
    }

    /// The colour channels of one panel pixel.
    fn channels(renderer: &EmbeddedRenderer, x: u32, y: u32) -> (f32, f32, f32) {
        crate::framebuffer::unpack_rgb565(
            renderer
                .framebuffer()
                .pixel(x, y)
                .unwrap_or_else(|| panic!("({x}, {y}) is on the panel")),
        )
    }

    /// A camera three units in front of the origin, looking down `-Z` with the
    /// default 60-degree field of view.
    fn camera_transform() -> TransformComponent {
        TransformComponent {
            translation: [0.0, 0.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }

    /// A frame with the camera bound to its primary surface and these
    /// instances.
    fn frame_with(instances: Vec<RenderInstance>) -> RenderFrame {
        let mut frame = RenderFrame {
            instances,
            ..RenderFrame::default()
        };
        frame.surface_views.insert(
            SurfaceId::PRIMARY,
            SurfaceView {
                camera: CameraComponent::default(),
                transform: camera_transform(),
            },
        );
        frame
    }

    /// A vertex with the fields the rasterizer reads.
    fn vertex(position: [f32; 3], texture_coordinates: [f32; 2]) -> MeshVertex {
        MeshVertex {
            position,
            texture_coordinates,
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
        }
    }

    /// Half the view's height three world units from the camera at the default
    /// field of view: what a frame-filling quad spans.
    const VIEW_HALF_HEIGHT: f32 = 1.732_050_8;

    /// A triangle one unit across in the `XY` plane, facing the camera.
    fn screen_triangle() -> Mesh {
        Mesh::from_data(
            "triangle",
            vec![
                vertex([-0.5, -0.5, 0.0], [0.0, 1.0]),
                vertex([0.5, -0.5, 0.0], [1.0, 1.0]),
                vertex([0.0, 0.5, 0.0], [0.5, 0.0]),
            ],
            vec![0, 1, 2],
        )
    }

    /// A quad covering exactly the camera's view, with the corners of a texture
    /// at its own corners.
    fn view_filling_quad() -> Mesh {
        Mesh::from_data(
            "quad",
            vec![
                vertex([-VIEW_HALF_HEIGHT, VIEW_HALF_HEIGHT, 0.0], [0.0, 0.0]),
                vertex([VIEW_HALF_HEIGHT, VIEW_HALF_HEIGHT, 0.0], [1.0, 0.0]),
                vertex([VIEW_HALF_HEIGHT, -VIEW_HALF_HEIGHT, 0.0], [1.0, 1.0]),
                vertex([-VIEW_HALF_HEIGHT, -VIEW_HALF_HEIGHT, 0.0], [0.0, 1.0]),
            ],
            vec![0, 1, 2, 0, 2, 3],
        )
    }

    /// A material painted one flat colour.
    fn painted(name: &str, colour: [f32; 3]) -> Material {
        Material::builder(name)
            .color_parameter(materials::BASE_COLOUR_SLOT, colour)
            .build()
    }

    /// An instance of the mesh and material the handles name, at `translation`.
    fn instance(
        mesh: pill_engine::Handle<Mesh>,
        material: pill_engine::Handle<Material>,
        translation: [f32; 3],
        rotation: [f32; 4],
    ) -> RenderInstance {
        RenderInstance {
            entity: Entity::from_raw(1, 1),
            transform: TransformComponent {
                translation,
                rotation,
                scale: [1.0; 3],
            },
            mesh: AssetKey(asset_key(mesh)),
            material: AssetKey(asset_key(material)),
            rendering_order: u8::MAX,
        }
    }

    /// A 2x2 texture whose four texels are the four primary colours, so a
    /// sampled corner is recognisable by its channels.
    fn corner_texture() -> Texture {
        Texture::from_rgba(
            "corners",
            TextureType::Color,
            vec![
                255, 0, 0, 255, // top-left red
                0, 255, 0, 255, // top-right green
                0, 0, 255, 255, // bottom-left blue
                255, 255, 255, 255, // bottom-right white
            ],
            2,
            2,
        )
        .expect("a 2x2 RGBA texture")
    }

    #[test]
    fn a_frame_draws_the_triangle_and_leaves_the_rest_at_the_background() {
        let mut assets = AssetManager::new();
        let mesh = assets
            .add_named("triangle", screen_triangle())
            .expect("a free name");
        let material = assets
            .add_named("white", painted("white", [1.0; 3]))
            .expect("a free name");
        let mut renderer = EmbeddedRenderer::default();

        let outcome = renderer
            .render(
                &frame_with(vec![instance(
                    mesh,
                    material,
                    [0.0; 3],
                    [0.0, 0.0, 0.0, 1.0],
                )]),
                &assets,
            )
            .expect("a frame from a camera");

        assert_eq!(outcome, FrameOutcome::Presented);
        // Inside the triangle, the material's white is lit but not pure white.
        let (red, green, _) = channels(&renderer, 120, 130);
        assert!(red > 0.5 && green > 0.5, "the triangle should be drawn");
        // Outside it, the panel keeps the background colour.
        assert_eq!(renderer.framebuffer().pixel(10, 10), Some(background()));
        assert_eq!(renderer.framebuffer().pixel(230, 220), Some(background()));
        assert_eq!(renderer.metrics().passes_with_draws, 1);
    }

    #[test]
    fn the_panel_stays_at_its_resolution_whatever_the_window_size() {
        let mut renderer = EmbeddedRenderer::default();

        renderer.resize_surface(SurfaceId::PRIMARY, 960, 720);

        assert_eq!(renderer.panel_size(), DEFAULT_PANEL_SIZE);
        assert_eq!(renderer.framebuffer().width(), 240);
        assert_eq!(renderer.framebuffer().height(), 240);
    }

    #[test]
    fn a_frame_without_a_camera_is_skipped() {
        let assets = AssetManager::new();
        let mut renderer = EmbeddedRenderer::default();
        let frame = RenderFrame::default();

        let outcome = renderer.render(&frame, &assets).expect("a skipped frame");
        assert_eq!(outcome, FrameOutcome::Skipped);
    }

    #[test]
    fn a_minimized_surface_draws_nothing() {
        let mut assets = AssetManager::new();
        let mesh = assets
            .add_named("triangle", screen_triangle())
            .expect("a free name");
        let material = assets
            .add_named("white", painted("white", [1.0; 3]))
            .expect("a free name");
        let mut renderer = EmbeddedRenderer::default();
        renderer.resize_surface(SurfaceId::PRIMARY, 0, 0);
        let frame = frame_with(vec![instance(
            mesh,
            material,
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
        )]);

        let outcome = renderer.render(&frame, &assets).expect("a skipped frame");
        assert_eq!(outcome, FrameOutcome::Skipped);
        // Nothing is drawn into a minimized surface, so the panel keeps what
        // it had: the black it started with.
        assert_eq!(renderer.framebuffer().pixel(120, 130), Some(0x0000));
    }

    #[test]
    fn a_textured_quad_samples_the_texel_under_each_corner() {
        let mut assets = AssetManager::new();
        let mesh = assets
            .add_named("quad", view_filling_quad())
            .expect("a free name");
        let texture = assets
            .add_named("corners", corner_texture())
            .expect("a free name");
        let material = assets
            .add_named(
                "painted",
                Material::builder("painted")
                    .color_parameter(materials::BASE_COLOUR_SLOT, [1.0; 3])
                    .texture(materials::BASE_COLOUR_TEXTURE_SLOT, &texture)
                    .build(),
            )
            .expect("a free name");
        let mut renderer = EmbeddedRenderer::default();

        renderer
            .render(
                &frame_with(vec![instance(
                    mesh,
                    material,
                    [0.0; 3],
                    [0.0, 0.0, 0.0, 1.0],
                )]),
                &assets,
            )
            .expect("a frame from a camera");

        // The light leaves the red texel red, the green one mostly green and
        // the blue one mostly blue; a mixed-up uv would swap the corners. The
        // sample points stay off the quad's diagonal, which is where the two
        // triangles meet.
        let (top_left_red, _, top_left_blue) = channels(&renderer, 3, 20);
        let (_, top_right_green, _) = channels(&renderer, 236, 20);
        let (_, _, bottom_left_blue) = channels(&renderer, 3, 236);
        let (bottom_right_red, bottom_right_green, bottom_right_blue) =
            channels(&renderer, 236, 220);
        assert!(
            top_left_red > 0.5,
            "top-left is the red texel (red={top_left_red})"
        );
        assert!(
            top_left_blue < 0.1,
            "top-left is not the blue texel (blue={top_left_blue})"
        );
        assert!(top_right_green > 0.5, "top-right is the green texel");
        assert!(bottom_left_blue > 0.5, "bottom-left is the blue texel");
        assert!(
            bottom_right_red > 0.5 && bottom_right_green > 0.5 && bottom_right_blue > 0.5,
            "bottom-right is the white texel"
        );
    }

    #[test]
    fn a_face_turned_from_the_light_draws_dimmer() {
        let mut assets = AssetManager::new();
        let mesh = assets
            .add_named("triangle", screen_triangle())
            .expect("a free name");
        let material = assets
            .add_named("white", painted("white", [1.0; 3]))
            .expect("a free name");
        let mut renderer = EmbeddedRenderer::default();
        // The second triangle is turned 45 degrees about X, which tilts its
        // face away from the key light's direction.
        let turned = glam::Quat::from_rotation_x(std::f32::consts::FRAC_PI_4).to_array();
        let instances = vec![
            instance(mesh, material, [-1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
            instance(mesh, material, [1.0, 0.0, 0.0], turned),
        ];

        renderer
            .render(&frame_with(instances), &assets)
            .expect("a frame from a camera");

        let (facing_red, _, _) = channels(&renderer, 50, 120);
        let (turned_red, _, _) = channels(&renderer, 180, 120);
        assert!(facing_red > 0.5, "the face-on triangle is drawn");
        assert!(turned_red > 0.05, "the turned triangle is drawn");
        assert!(
            facing_red > turned_red + 0.2,
            "the face toward the light ({facing_red}) should be brighter than the turned one ({turned_red})"
        );
    }

    #[test]
    fn the_nearer_triangle_stays_visible_whatever_the_draw_order() {
        for far_first in [true, false] {
            let mut assets = AssetManager::new();
            let mesh = assets
                .add_named("triangle", screen_triangle())
                .expect("a free name");
            let near_material = assets
                .add_named("red", painted("red", [1.0, 0.0, 0.0]))
                .expect("a free name");
            let far_material = assets
                .add_named("green", painted("green", [0.0, 1.0, 0.0]))
                .expect("a free name");
            let identity = [0.0, 0.0, 0.0, 1.0];
            let near = instance(mesh, near_material, [0.0, 0.0, 0.0], identity);
            let far = instance(mesh, far_material, [0.0, 0.0, -1.0], identity);
            let instances = if far_first {
                vec![far, near]
            } else {
                vec![near, far]
            };
            let mut renderer = EmbeddedRenderer::default();

            renderer
                .render(&frame_with(instances), &assets)
                .expect("a frame from a camera");

            let (red, green, _) = channels(&renderer, 120, 130);
            assert!(
                red > 0.5 && green < 0.1,
                "the near red triangle should stay on top (far_first={far_first}, red={red}, green={green})"
            );
        }
    }

    #[test]
    fn invalidating_the_assets_empties_the_texture_cache() {
        let mut assets = AssetManager::new();
        let mesh = assets
            .add_named("quad", view_filling_quad())
            .expect("a free name");
        let texture = assets
            .add_named("corners", corner_texture())
            .expect("a free name");
        let material = assets
            .add_named(
                "painted",
                Material::builder("painted")
                    .texture(materials::BASE_COLOUR_TEXTURE_SLOT, &texture)
                    .build(),
            )
            .expect("a free name");
        let mut renderer = EmbeddedRenderer::default();
        let frame = frame_with(vec![instance(
            mesh,
            material,
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
        )]);

        renderer
            .render(&frame, &assets)
            .expect("a frame from a camera");
        assert_eq!(renderer.textures.len(), 1);

        renderer.invalidate_assets();

        assert!(renderer.textures.is_empty());
    }

    #[test]
    fn the_consumed_components_are_the_ones_the_frame_needs() {
        let renderer = EmbeddedRenderer::default();

        let consumed = renderer.capabilities().consumed_components;

        assert!(consumed
            .iter()
            .any(|name| name == "pill_rendering::CameraComponent"));
        assert!(consumed
            .iter()
            .any(|name| name == "pill_rendering::MeshRendererComponent"));
    }

    /// A white 1x1 UI texture: what a solid fill samples.
    fn white_ui_texture() -> pill_ui_frame::UiTextureSet {
        let mut set = pill_ui_frame::UiTextureSet::new();
        set.push_full(
            pill_ui_frame::TextureId(1),
            1,
            1,
            1,
            pill_ui_frame::UiTextureFilter::Nearest,
            std::sync::Arc::from(vec![255u8, 255, 255, 255]),
        );
        set
    }

    /// A red quad from `(0, 0)` to `(120, 60)` in logical pixels.
    fn red_ui_draw() -> pill_ui_frame::UiDraw {
        let corner = |x: f32, y: f32, u: f32, v: f32| pill_ui_frame::UiVertex {
            position: pill_core::math::Vector2f::new(x, y),
            uv: pill_core::math::Vector2f::new(u, v),
            color: pill_ui_frame::Color::rgba(1.0, 0.0, 0.0, 1.0),
        };
        let mut list = pill_ui_frame::DrawList::new();
        list.vertices.extend([
            corner(0.0, 0.0, 0.0, 0.0),
            corner(120.0, 0.0, 1.0, 0.0),
            corner(120.0, 60.0, 1.0, 1.0),
            corner(0.0, 60.0, 0.0, 1.0),
        ]);
        list.indices.extend([0, 1, 2, 0, 2, 3]);
        list.commands.push(pill_ui_frame::DrawCommand::Draw {
            texture: pill_ui_frame::TextureId(1),
            first_index: 0,
            index_count: 6,
        });
        pill_ui_frame::UiDraw {
            surface: SurfaceId::PRIMARY.0,
            scale_factor: 1.0,
            list,
        }
    }

    /// A frame with something of every kind the renderer draws, and the same
    /// frame's UI and debug payload for the second draw.
    fn frame_with_everything(
        mesh: pill_engine::Handle<Mesh>,
        material: pill_engine::Handle<Material>,
    ) -> RenderFrame {
        let mut frame = frame_with(vec![instance(
            mesh,
            material,
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
        )]);
        frame.ui.textures = white_ui_texture();
        frame.ui.draws.push(red_ui_draw());
        frame.debug.lines.push(pill_engine::DebugLine {
            start: [-0.5, 0.0, 0.0],
            end: [0.5, 0.0, 0.0],
            color: [0.0, 1.0, 0.0, 1.0],
            depth_test: false,
            chrome: false,
        });
        frame.debug.labels.push(pill_engine::DebugLabel {
            anchor: pill_engine::LabelAnchor::Screen { x: 200.0, y: 210.0 },
            text: "OK".to_owned(),
            color: [1.0, 1.0, 1.0, 1.0],
        });
        frame
    }

    #[test]
    fn a_frame_with_ui_and_debug_lines_draws_all_of_them() {
        let mut assets = AssetManager::new();
        let mesh = assets
            .add_named("triangle", screen_triangle())
            .expect("a free name");
        let material = assets
            .add_named("white", painted("white", [1.0; 3]))
            .expect("a free name");
        let frame = frame_with_everything(mesh, material);
        let mut renderer = EmbeddedRenderer::default();

        let outcome = renderer
            .render(&frame, &assets)
            .expect("a frame from a camera");

        assert_eq!(outcome, FrameOutcome::Presented);
        // The UI quad covers the top-left quarter in red.
        let (ui_red, ui_green, _) = channels(&renderer, 30, 30);
        assert!(
            ui_red > 0.9 && ui_green < 0.1,
            "the UI quad drew over the scene"
        );
        // The debug line crosses the panel's middle in green.
        let (_, line_green, _) = channels(&renderer, 120, 120);
        assert!(line_green > 0.5, "the debug line drew over the scene");
        // The label is drawn near its anchor.
        let label_lit = (200..230).any(|x| (210..235).any(|y| channels(&renderer, x, y).0 > 0.2));
        assert!(label_lit, "the label drew at its anchor");
    }

    #[test]
    fn a_second_identical_frame_draws_the_same_pixels_without_growing_the_caches() {
        let mut assets = AssetManager::new();
        let mesh = assets
            .add_named("triangle", screen_triangle())
            .expect("a free name");
        let material = assets
            .add_named("white", painted("white", [1.0; 3]))
            .expect("a free name");
        let frame = frame_with_everything(mesh, material);
        let mut renderer = EmbeddedRenderer::default();

        renderer
            .render(&frame, &assets)
            .expect("a frame from a camera");
        let first: Vec<u16> = (0..240 * 240)
            .map(|index| {
                renderer
                    .framebuffer()
                    .pixel(index % 240, index / 240)
                    .expect("on the panel")
            })
            .collect();
        let (textures, ui_textures, clip_capacity) = (
            renderer.textures.len(),
            renderer.ui.texture_count(),
            renderer.ui.clip_capacity(),
        );

        renderer
            .render(&frame, &assets)
            .expect("a frame from a camera");

        let second: Vec<u16> = (0..240 * 240)
            .map(|index| {
                renderer
                    .framebuffer()
                    .pixel(index % 240, index / 240)
                    .expect("on the panel")
            })
            .collect();
        assert_eq!(first, second, "the same frame draws the same pixels");
        assert_eq!(renderer.textures.len(), textures);
        assert_eq!(renderer.ui.texture_count(), ui_textures);
        assert_eq!(renderer.ui.clip_capacity(), clip_capacity);
    }
}
