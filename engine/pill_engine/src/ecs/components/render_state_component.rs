use crate::{
    ecs::components::{GlobalComponent, GlobalComponentStorage},
    graphics::PassTextureHandle,
};

use pill_core::PillTypeMapKey;

/// HDR → display mapping applied by the tonemap pass.
#[derive(Clone, Copy, Debug)]
pub enum Tonemap {
    /// Linear pass-through; values above 1 clip.
    None,
    /// Extended Reinhard c·(1 + c/W²)/(1 + c): `white_point` W maps to 1.0; INFINITY = plain c/(1 + c).
    Reinhard { white_point: f32 },
    /// Lottes' filmic curve x^a / (x^(a·d)·b + c) per channel (GDC 2016, "Advanced Techniques and
    /// Optimization of VDR Color Pipelines"); b and c are solved so `mid_in` maps to `mid_out` and
    /// `hdr_max` to 1. `Tonemap::lottes()` gives the demo's values.
    Lottes {
        /// a: toe steepness.
        contrast: f32,
        /// d: shoulder speed.
        shoulder: f32,
        /// Input mapped to white.
        hdr_max: f32,
        /// Middle grey in and out.
        mid_in: f32,
        mid_out: f32,
    },
}

impl Tonemap {
    /// Lottes curve with contrast 1.6, shoulder 0.977, white at 8, grey 0.18 → 0.267.
    pub fn lottes() -> Self {
        Self::Lottes {
            contrast: 1.6,
            shoulder: 0.977,
            hdr_max: 8.0,
            mid_in: 0.18,
            mid_out: 0.267,
        }
    }
}

/// Pass chain the engine builds on the first frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PassChain {
    /// Opaque PBR → game passes → tonemap.
    #[default]
    Default,
    /// Default chain plus the post-processing passes (see `config::post_fx_pass_chain`); needs
    /// pill_engine's `post_fx` feature.
    #[cfg(feature = "post_fx")]
    PostFx,
}

/// PostFx chain bloom.
#[derive(Clone, Copy, Debug)]
pub struct Bloom {
    /// Bloom added to the image (0 = off).
    pub strength: f32,
    /// Upsample tap offset as a fraction of the screen width; larger = wider glow.
    pub radius: f32,
}

/// PostFx chain depth of field. Circle of confusion = (1/focus_distance − 1/distance) · focus_scale,
/// clamped to ±1; negative is in front of the focal plane (near layer).
#[derive(Clone, Copy, Debug)]
pub struct DepthOfField {
    /// View-space distance in focus.
    pub focus_distance: f32,
    /// Blur gain (0 = off).
    pub focus_scale: f32,
    /// Near-layer blur width multiplier (0 = no near blur).
    pub near_blur: f32,
    /// Near CoC at which the near layer becomes fully opaque (lower = more foreground blurred).
    pub near_cut: f32,
    /// Horizontal bokeh squeeze: 1 = round, 0.5 = 2:1 anamorphic oval.
    pub anamorphic_squeeze: f32,
}

/// PostFx chain screen-space ambient occlusion; darkens only the indirect (IBL) light.
#[derive(Clone, Copy, Debug)]
pub struct Ssao {
    /// Blend of the occlusion (0 = off, 1 = full).
    pub strength: f32,
    /// World-space sampling radius.
    pub radius: f32,
    pub intensity: f32,
    /// Contrast exponent applied to the visibility.
    pub power: f32,
}

/// PostFx chain lens and film emulation; the defaults leave the image unchanged.
#[derive(Clone, Copy, Debug)]
pub struct Lens {
    /// k1 of r² distortion: negative = barrel, positive = pincushion.
    pub distortion: f32,
    /// Red/blue split at the frame corner in UV units, falling off as r² to 0 at the center.
    pub chromatic_aberration: f32,
    /// Vignette falloff scale (0 = off, ~0.6 subtle).
    pub vignette: f32,
    /// Film grain amplitude (0 = off, 0.03 barely visible, 0.12 medium film stock).
    pub grain_strength: f32,
    /// Grain texel size in pixels at 1280 px width (scales with resolution).
    pub grain_size: f32,
    /// Grain pattern updates per second (24 = filmic, 0 = every frame).
    pub grain_fps: f32,
    /// Grade: per-channel gamma, then smoothstep(edge0, edge1) (edge0 > edge1 inverts), blended in
    /// by `grade_mix`.
    pub grade_gamma: [f32; 3],
    pub grade_edge0: f32,
    pub grade_edge1: f32,
    /// 0 = no grade.
    pub grade_mix: f32,
}

impl Default for Lens {
    fn default() -> Self {
        Self {
            distortion: 0.0,
            chromatic_aberration: 0.0,
            vignette: 0.0,
            grain_strength: 0.0,
            grain_size: 4.0,
            grain_fps: 24.0,
            grade_gamma: [1.0; 3],
            grade_edge0: 0.0,
            grade_edge1: 1.0,
            grade_mix: 0.0,
        }
    }
}

/// Directional light; `direction` points from the light toward the scene.
#[derive(Clone, Copy, Debug)]
pub struct DirectionalLight {
    pub direction: [f32; 3],
    /// Linear RGB, multiplied by `intensity`.
    pub color: [f32; 3],
    pub intensity: f32,
}

/// Engine-owned (always present, cannot be removed or constructed by games); games set the
/// public environment, IBL, fog and tonemap fields.
pub struct RenderStateComponent {
    pub(crate) boot_done: bool,
    /// Chain built on the first frame; set it from `start()`, later changes have no effect.
    pub pass_chain: PassChain,
    /// Scene color target (HDR_TARGET_FORMAT) shared by scene passes; resolved by the tonemap pass.
    pub(crate) hdr_target: PassTextureHandle,
    /// Scene depth target (DEPTH_TARGET_FORMAT) shared by scene passes.
    pub(crate) depth_target: PassTextureHandle,
    /// Engine default IBL maps (diffuse, specular, BRDF LUT): used when a game-set handle is invalid.
    pub(crate) default_ibl: [PassTextureHandle; 3],
    /// Environment (equirect) image drawn behind the scene; `None` draws `background_color`.
    pub background: Option<PassTextureHandle>,
    /// Solid background color used when `background` is `None` (linear RGB).
    pub background_color: [f32; 3],
    pub ibl_diffuse: PassTextureHandle,
    pub ibl_specular: PassTextureHandle,
    pub ibl_brdf_lut: PassTextureHandle,
    /// Exponential-squared fog: color is mixed toward `fog_color` by 1 - exp(-density² · distance²);
    /// density 0 disables fog.
    pub fog_color: [f32; 3],
    pub fog_density: f32,
    /// Scales both IBL terms (diffuse irradiance and specular reflection); 0 = direct light only.
    pub ibl_intensity: f32,
    /// Key, rim and bounce lights of the PBR passes.
    pub lights: [DirectionalLight; 3],
    pub tonemap: Tonemap,
    /// PostFx chain: motion blur length as a fraction of the frame's motion (0.5 = 180° shutter,
    /// 0 = off).
    pub motion_blur: f32,
    pub ssao: Ssao,
    pub depth_of_field: DepthOfField,
    pub bloom: Bloom,
    pub lens: Lens,
    /// Bumped by `reset_motion_history`; passes drop last frame's transforms when it changes.
    pub(crate) motion_history_epoch: u32,
}

impl RenderStateComponent {
    /// Creates the component in its pre-boot state; handles are replaced by `create_default_resources`.
    pub(crate) fn new() -> Self {
        Self {
            boot_done: false,
            pass_chain: PassChain::Default,
            hdr_target: PassTextureHandle::default(),
            depth_target: PassTextureHandle::default(),
            default_ibl: [PassTextureHandle::default(); 3],
            background: None,
            background_color: [0.15, 0.15, 0.15],
            ibl_diffuse: PassTextureHandle::default(),
            ibl_specular: PassTextureHandle::default(),
            ibl_brdf_lut: PassTextureHandle::default(),
            fog_color: [0.0, 0.0, 0.0],
            fog_density: 0.0,
            ibl_intensity: 1.0,
            // Key behind the camera upper-left, cool rim front upper-right, neutral bounce from below
            lights: [
                DirectionalLight {
                    direction: [0.38, -0.38, -0.84],
                    color: [1.0, 0.98, 0.95],
                    intensity: 2.2,
                },
                DirectionalLight {
                    direction: [-0.50, 0.50, 0.71],
                    color: [0.6, 0.65, 1.0],
                    intensity: 0.8,
                },
                DirectionalLight {
                    direction: [0.0, -1.0, 0.0],
                    color: [0.8, 0.8, 0.8],
                    intensity: 0.3,
                },
            ],
            tonemap: Tonemap::Reinhard {
                white_point: f32::INFINITY,
            },
            motion_blur: 0.5,
            ssao: Ssao {
                strength: 1.0,
                radius: 0.5,
                intensity: 1.0,
                power: 1.5,
            },
            depth_of_field: DepthOfField {
                focus_distance: 6.0,
                focus_scale: 0.0,
                near_blur: 1.0,
                near_cut: 0.9,
                anamorphic_squeeze: 1.0,
            },
            bloom: Bloom {
                strength: 0.04,
                radius: 0.005,
            },
            lens: Lens::default(),
            motion_history_epoch: 0,
        }
    }
}

impl RenderStateComponent {
    /// Scene color target (HDR_TARGET_FORMAT) for game-defined passes.
    pub fn hdr_target(&self) -> PassTextureHandle {
        self.hdr_target
    }

    /// Scene depth target (DEPTH_TARGET_FORMAT) for game-defined passes.
    pub fn depth_target(&self) -> PassTextureHandle {
        self.depth_target
    }

    /// Next frame renders without motion vectors (previous transforms = current); call on camera
    /// cuts and teleports so motion blur does not smear across them.
    pub fn reset_motion_history(&mut self) {
        self.motion_history_epoch = self.motion_history_epoch.wrapping_add(1);
    }
}

impl PillTypeMapKey for RenderStateComponent {
    type Storage = GlobalComponentStorage<RenderStateComponent>;
}

impl GlobalComponent for RenderStateComponent {}
