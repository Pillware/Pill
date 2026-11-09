use crate::{
    ecs::{
        deferred_update_system, input_system, rendering_system, time_system,
        DeferredUpdateComponent, InputComponent, PlayerId, RenderStateComponent, SystemFunction,
        TimeComponent, UpdatePhase,
    },
    engine::Engine,
    graphics::{
        Pass, PassPBROpaque, PassTextureHandle, PassTonemap, RendererMaterialHandle,
        RendererTargetDesc, RendererTextureHandle, TargetSize,
    },
    renderer::resources::RendererTexture,
    resources::{MaterialHandle, TextureHandle, TextureType},
};

#[cfg(feature = "ui")]
use crate::ecs::egui_system;

#[cfg(feature = "post_fx")]
use crate::graphics::{
    PassBloom, PassDepthOfField, PassFxaa, PassLens, PassMotionBlur, PassPBRTranslucent, PassSsao,
};

#[cfg(not(target_arch = "wasm32"))]
use crate::ecs::{audio_system, haptics_system, AudioManagerComponent};

#[cfg(feature = "physics")]
use crate::ecs::physics_system;

use pill_core::PillSlotMapKeyData;

use pill_core::Result;

use lazy_static::lazy_static;
use std::any::TypeId;

// --- General ---

pub const PANIC_ON_GAME_ERRORS: bool = true;

// --- ECS ---

pub const MAX_ENTITIES: usize = 1000;
pub const MAX_CONCURRENT_2D_SOUNDS: usize = 10;
pub const MAX_CONCURRENT_3D_SOUNDS: usize = 10;
pub const MAX_CAMERAS: usize = 10;
pub const NUM_SUPPORTED_GAMEPADS: usize = PlayerId::Player4 as usize + 1; // Maximum number of supported gamepads

pub struct SystemConfig {
    pub name: &'static str,
    pub system_function: SystemFunction,
    pub update_phase: UpdatePhase,
}

pub const INPUT_SYSTEM: SystemConfig = SystemConfig {
    name: "input_system",
    system_function: input_system,
    update_phase: UpdatePhase::PreGame,
};

#[cfg(not(target_arch = "wasm32"))]
pub const HAPTICS_SYSTEM: SystemConfig = SystemConfig {
    name: "haptics_system",
    system_function: haptics_system,
    update_phase: UpdatePhase::PostGame,
};

pub const TIME_SYSTEM: SystemConfig = SystemConfig {
    name: "time_system",
    system_function: time_system,
    update_phase: UpdatePhase::PostGame,
};

#[cfg(not(target_arch = "wasm32"))]
pub const AUDIO_SYSTEM: SystemConfig = SystemConfig {
    name: "audio_system",
    system_function: audio_system,
    update_phase: UpdatePhase::PostGame,
};

pub const DEFERRED_UPDATE_SYSTEM: SystemConfig = SystemConfig {
    name: "deferred_update_system",
    system_function: deferred_update_system,
    update_phase: UpdatePhase::PostGame,
};

pub const RENDERING_SYSTEM: SystemConfig = SystemConfig {
    name: "rendering_system",
    system_function: rendering_system,
    update_phase: UpdatePhase::PostGame,
};

#[cfg(feature = "physics")]
pub const PHYSICS_SYSTEM: SystemConfig = SystemConfig {
    name: "physics_system",
    system_function: physics_system,
    update_phase: UpdatePhase::PostGame,
};

#[cfg(feature = "ui")]
pub const EGUI_SYSTEM: SystemConfig = SystemConfig {
    name: "egui_system",
    system_function: egui_system,
    update_phase: UpdatePhase::PostGame,
};

// --- Rendering ---

pub const HDR_TARGET_NAME: &str = "hdr_target";
pub const HDR_TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH_TARGET_NAME: &str = "depth_target";
pub const DEPTH_TARGET_FORMAT: wgpu::TextureFormat = RendererTexture::DEPTH_FORMAT;
/// Motion vectors (UV offset to last frame's position) written by the PBR passes.
pub const VELOCITY_TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
/// Indirect (IBL) lighting alone, written by the PBR opaque pass for SSAO.
pub const AMBIENT_TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

// WGSL is generated from res/shaders/*.hlsl by pill_assets (build.rs)
// The post_fx variants also write motion vectors and the ambient term
#[cfg(not(feature = "post_fx"))]
pub const PBR_OPAQUE_VERTEX_SHADER: &str = include_str!("../res/shaders/pbr_opaque_vertex.wgsl");
#[cfg(feature = "post_fx")]
pub const PBR_OPAQUE_VERTEX_SHADER: &str =
    include_str!("../res/shaders/pbr_opaque_post_fx_vertex.wgsl");
#[cfg(not(feature = "post_fx"))]
pub const PBR_OPAQUE_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/pbr_opaque_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const PBR_OPAQUE_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/pbr_opaque_post_fx_fragment.wgsl");
pub const BACKGROUND_VERTEX_SHADER: &str = include_str!("../res/shaders/background_vertex.wgsl");
#[cfg(not(feature = "post_fx"))]
pub const BACKGROUND_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/background_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const BACKGROUND_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/background_post_fx_fragment.wgsl");
pub const FULLSCREEN_VERTEX_SHADER: &str = include_str!("../res/shaders/fullscreen_vertex.wgsl");
pub const TONEMAP_FRAGMENT_SHADER: &str = include_str!("../res/shaders/tonemap_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const BLOOM_DOWNSAMPLE_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/bloom_downsample_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const BLOOM_UPSAMPLE_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/bloom_upsample_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const BLOOM_COMPOSITE_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/bloom_composite_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const LINEARIZE_DEPTH_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/linearize_depth_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const DOF_DOWNSAMPLE_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/dof_downsample_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const DOF_BOKEH_FRAGMENT_SHADER: &str = include_str!("../res/shaders/dof_bokeh_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const DOF_NEAR_PREFILTER_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/dof_near_prefilter_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const DOF_NEAR_BLUR_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/dof_near_blur_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const DOF_NEAR_COMPOSITE_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/dof_near_composite_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const SSAO_FRAGMENT_SHADER: &str = include_str!("../res/shaders/ssao_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const SSAO_BLUR_FRAGMENT_SHADER: &str = include_str!("../res/shaders/ssao_blur_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const SSAO_APPLY_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/ssao_apply_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const FXAA_FRAGMENT_SHADER: &str = include_str!("../res/shaders/fxaa_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const LENS_FRAGMENT_SHADER: &str = include_str!("../res/shaders/lens_fragment.wgsl");
#[cfg(feature = "post_fx")]
pub const MOTION_BLUR_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/motion_blur_fragment.wgsl");

/// Frame composition: opaque PBR → game-defined passes (`Engine::add_render_pass`) → tonemap → UI.
/// Creates the shared HDR and depth targets and records them in `RenderStateComponent`.
pub fn default_pass_chain(engine: &mut Engine) -> Result<Vec<Box<dyn Pass>>> {
    let (hdr_target, depth_target) = create_scene_targets(engine)?;
    let mut passes: Vec<Box<dyn Pass>> =
        vec![Box::new(opaque_pass(engine, hdr_target, depth_target)?)];
    passes.append(&mut engine.custom_render_passes);
    passes.push(Box::new(PassTonemap::new(hdr_target)));
    push_ui_pass(engine, &mut passes)?;
    Ok(passes)
}

#[cfg(feature = "post_fx")]
/// `PassChain::PostFx`: the default chain with post-processing between the scene passes and
/// tonemapping; parameters live in `RenderStateComponent`:
/// opaque PBR (+ motion vectors, ambient) → SSAO → translucent PBR → game passes → depth of
/// field → motion blur (+ DoF composite) → bloom → tonemap → FXAA → lens → UI.
pub fn post_fx_pass_chain(engine: &mut Engine) -> Result<Vec<Box<dyn Pass>>> {
    // The opaque pass renders into `opaque_target`; SSAO writes the shared HDR target that the
    // translucent and game passes then draw onto
    let (hdr_target, depth_target) = create_scene_targets(engine)?;
    let opaque_target = create_target(engine, "opaque_target", HDR_TARGET_FORMAT)?;
    let ambient_target = create_target(engine, "ambient_target", AMBIENT_TARGET_FORMAT)?;
    let velocity_target = create_target(engine, "velocity_target", VELOCITY_TARGET_FORMAT)?;
    let depth_of_field_target = engine.renderer.create_render_target(RendererTargetDesc {
        name: "depth_of_field_target".to_string(),
        format: HDR_TARGET_FORMAT,
        size: TargetSize::SurfaceDivisor(4),
    })?;
    let motion_blur_target = create_target(engine, "motion_blur_target", HDR_TARGET_FORMAT)?;
    let bloom_target = create_target(engine, "bloom_target", HDR_TARGET_FORMAT)?;
    // Tonemapped, still linear; the lens pass encodes sRGB into the swapchain
    let ldr_target = create_target(engine, "ldr_target", HDR_TARGET_FORMAT)?;
    // FXAA at the render size, before the upscale: its blend reaches less than a pixel, so it
    // smooths 1-texel stair steps but not the 2-pixel steps of an upscaled image
    let fxaa_target = create_target(engine, "fxaa_target", HDR_TARGET_FORMAT)?;

    let opaque = opaque_pass(engine, opaque_target, depth_target)?;
    let translucent = PassPBRTranslucent::new(
        hdr_target,
        depth_target,
        opaque.max_instances(),
        opaque.initial_ibl(),
    )
    .with_velocity_target(velocity_target);
    let mut passes: Vec<Box<dyn Pass>> = vec![
        Box::new(
            opaque
                .without_translucent()
                .with_velocity_target(velocity_target)
                .with_ambient_target(ambient_target),
        ),
        Box::new(PassSsao::new(
            opaque_target,
            ambient_target,
            depth_target,
            hdr_target,
        )),
        Box::new(translucent),
    ];
    passes.append(&mut engine.custom_render_passes);
    passes.push(Box::new(PassDepthOfField::new(
        hdr_target,
        depth_target,
        depth_of_field_target,
    )));
    passes.push(Box::new(
        PassMotionBlur::new(
            hdr_target,
            velocity_target,
            depth_target,
            motion_blur_target,
        )
        .with_depth_of_field(depth_of_field_target),
    ));
    passes.push(Box::new(PassBloom::new(motion_blur_target, bloom_target)));
    passes.push(Box::new(
        PassTonemap::new(bloom_target).with_output(ldr_target),
    ));
    passes.push(Box::new(PassFxaa::new(ldr_target, fxaa_target)));
    passes.push(Box::new(PassLens::new(fxaa_target)));
    push_ui_pass(engine, &mut passes)?;
    Ok(passes)
}

/// Surface-sized render target.
fn create_target(
    engine: &mut Engine,
    name: &str,
    format: wgpu::TextureFormat,
) -> Result<PassTextureHandle> {
    engine.renderer.create_render_target(RendererTargetDesc {
        name: name.to_string(),
        format,
        size: TargetSize::Surface,
    })
}

/// Shared HDR color and depth targets, recorded in `RenderStateComponent` for game passes.
fn create_scene_targets(engine: &mut Engine) -> Result<(PassTextureHandle, PassTextureHandle)> {
    let hdr_target = create_target(engine, HDR_TARGET_NAME, HDR_TARGET_FORMAT)?;
    let depth_target = engine.renderer.create_depth_texture(DEPTH_TARGET_NAME)?;
    let render_state = engine.get_global_component_mut::<RenderStateComponent>()?;
    render_state.hdr_target = hdr_target;
    render_state.depth_target = depth_target;
    Ok((hdr_target, depth_target))
}

fn opaque_pass(
    engine: &Engine,
    hdr_target: PassTextureHandle,
    depth_target: PassTextureHandle,
) -> Result<PassPBROpaque> {
    // Engine defaults, not the game's handles: the pass falls back to these if the game's are invalid
    let initial_ibl = engine
        .get_global_component::<RenderStateComponent>()?
        .default_ibl;
    // At most one mesh instance per entity
    let max_instances = engine
        .config
        .get_int("MAX_ENTITIES")
        .unwrap_or(MAX_ENTITIES as i64) as usize;
    Ok(PassPBROpaque::new(
        hdr_target,
        depth_target,
        max_instances,
        initial_ibl,
    ))
}

#[allow(unused_variables)]
fn push_ui_pass(engine: &Engine, passes: &mut Vec<Box<dyn Pass>>) -> Result<()> {
    #[cfg(feature = "ui")]
    {
        use crate::{ecs::EguiComponent, graphics::PassEgui};
        let client = engine
            .get_global_component::<EguiComponent>()?
            .egui_client
            .clone();
        let window = engine.renderer.get_window();
        passes.push(Box::new(PassEgui::new(window, client)));
    }
    Ok(())
}

// --- Resources ---

pub const RESOURCE_VERSION_LIMIT: usize = 255;

pub const MAX_MATERIALS: usize = 10;
pub const MAX_TEXTURES: usize = 10;
pub const MAX_MESHES: usize = 10;
pub const MAX_SOUNDS: usize = 10;

// Convention: All resource names starting with "pill_engine_" are restricted, cannot be added and removed from game
pub const DEFAULT_RESOURCE_PREFIX: &str = "pill_engine";
pub const DEFAULT_COLOR_TEXTURE_NAME: &str = "pill_engine_default_color";
pub const DEFAULT_NORMAL_TEXTURE_NAME: &str = "pill_engine_default_normal";
pub const DEFAULT_METALLIC_ROUGHNESS_TEXTURE_NAME: &str = "pill_engine_default_metallic_roughness";
pub const DEFAULT_EMISSIVE_TEXTURE_NAME: &str = "pill_engine_default_emissive";

// RTEX layout: b"RTEX" | u32LE version=1 | u32LE width | u32LE height | raw RGBA bytes
pub const DEFAULT_COLOR_TEXTURE_BYTES: [u8; 20] = [
    b'R', b'T', b'E', b'X', 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 255, 255, 255,
    255, // white albedo
];
pub const DEFAULT_NORMAL_TEXTURE_BYTES: [u8; 20] = [
    b'R', b'T', b'E', b'X', 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 128, 128, 255,
    255, // flat normal (0,0,1)
];
// Neutral metallic_roughness (glTF): G=255 roughness, B=255 metallic, so the material's
// roughness and metallic factors pass through unchanged when no texture is set
pub const DEFAULT_METALLIC_ROUGHNESS_TEXTURE_BYTES: [u8; 20] = [
    b'R', b'T', b'E', b'X', 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 255, 255, 255,
];
// White emissive: emission = texture × Material.emissive (glTF), so the factor alone drives it
pub const DEFAULT_EMISSIVE_TEXTURE_BYTES: [u8; 20] = [
    b'R', b'T', b'E', b'X', 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 255, 255, 255, 255,
];

// 1×1 Rgba32Float white — background equirect fallback (PassPBROpaque); tinted by RenderStateComponent::background_color
// 1.0f32 LE = 00 00 80 3F
pub const DEFAULT_EQUIRECT_FALLBACK_PIXEL: &[u8] = &[
    0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x80, 0x3F,
];
// 1×1 Rgba8Unorm neutral-gray — IBL diffuse/specular fallback: RGBA = [77, 77, 77, 255] ≈ 0.3 linear
pub const DEFAULT_IBL_FALLBACK_PIXEL: &[u8] = &[77, 77, 77, 255];

pub const DEFAULT_MATERIAL_NAME: &str = "pill_engine_default_material";

// Render queue key
pub type RenderQueueKeyType = u64;

// 64-bit render sort key layout (field masks in graphics/render_queue.rs):
// +--------+------------------+----------------------------------------------+
// | Bits   | Field            | Description                                  |
// +--------+------------------+----------------------------------------------+
// | 56..63 | Material Index   |  8 bits: renderer material slot (max 256)    |
// | 48..55 | Material Version |  8 bits: renderer material version (max 256) |
// | 40..47 | Mesh Index       |  8 bits: renderer mesh slot (max 256)        |
// | 32..39 | Mesh Version     |  8 bits: renderer mesh version (max 256)     |
// |  0..31 | Free             | 32 bits: free for future use                 |
// +--------+------------------+----------------------------------------------+
// Sorting by key groups draws by material first (fewest bind group switches), then by mesh.

// Default resource handle - Material
pub const DEFAULT_MATERIAL_HANDLE: MaterialHandle = MaterialHandle(PillSlotMapKeyData {
    index: 1,
    version: std::num::NonZeroU32::new(1).unwrap(),
});
pub const DEFAULT_RENDERER_MATERIAL_HANDLE: RendererMaterialHandle =
    RendererMaterialHandle(PillSlotMapKeyData {
        index: 1,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

pub fn get_default_material_handles() -> (MaterialHandle, RendererMaterialHandle) {
    (DEFAULT_MATERIAL_HANDLE, DEFAULT_RENDERER_MATERIAL_HANDLE)
}

lazy_static! {
    pub static ref ENGINE_GLOBAL_COMPONENTS: Vec<TypeId> = {
        #[allow(unused_mut)]
        let mut component_types = vec![
            TypeId::of::<InputComponent>(),
            TypeId::of::<TimeComponent>(),
            TypeId::of::<DeferredUpdateComponent>(),
            TypeId::of::<RenderStateComponent>(),
        ];
        #[cfg(not(target_arch = "wasm32"))]
        component_types.push(TypeId::of::<AudioManagerComponent>());
        #[cfg(feature = "ui")]
        component_types.push(TypeId::of::<crate::ecs::EguiComponent>());
        component_types
    };
}
