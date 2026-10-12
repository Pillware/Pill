use crate::{
    ecs::{
        deferred_update_system, input_system, rendering_system, time_system,
        DeferredUpdateComponent, InputComponent, PlayerId, RenderStateComponent, SystemFunction,
        TimeComponent, UpdatePhase,
    },
    engine::Engine,
    graphics::{
        Pass, PassPBROpaque, PassTonemap, RendererMaterialHandle, RendererTargetDesc,
        RendererTextureHandle, TargetSize,
    },
    renderer::resources::RendererTexture,
    resources::{MaterialHandle, TextureHandle, TextureType},
};

#[cfg(feature = "ui")]
use crate::ecs::egui_system;

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

// WGSL is generated from res/shaders/*.hlsl by pill_assets (build.rs)
pub const PBR_OPAQUE_VERTEX_SHADER: &str = include_str!("../res/shaders/pbr_opaque_vertex.wgsl");
pub const PBR_OPAQUE_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/pbr_opaque_fragment.wgsl");
pub const BACKGROUND_VERTEX_SHADER: &str = include_str!("../res/shaders/background_vertex.wgsl");
pub const BACKGROUND_FRAGMENT_SHADER: &str =
    include_str!("../res/shaders/background_fragment.wgsl");
pub const TONEMAP_VERTEX_SHADER: &str = include_str!("../res/shaders/tonemap_vertex.wgsl");
pub const TONEMAP_FRAGMENT_SHADER: &str = include_str!("../res/shaders/tonemap_fragment.wgsl");

/// Frame composition: opaque PBR → game-defined passes (`Engine::add_render_pass`) → tonemap → UI.
/// Creates the shared HDR and depth targets and records them in `RenderStateComponent`.
pub fn default_pass_chain(engine: &mut Engine) -> Result<Vec<Box<dyn Pass>>> {
    let hdr_target = engine.renderer.create_render_target(RendererTargetDesc {
        name: HDR_TARGET_NAME.to_string(),
        format: HDR_TARGET_FORMAT,
        size: TargetSize::Surface,
    })?;
    let depth_target = engine.renderer.create_depth_texture(DEPTH_TARGET_NAME)?;

    let render_state = engine.get_global_component_mut::<RenderStateComponent>()?;
    render_state.hdr_target = hdr_target;
    render_state.depth_target = depth_target;
    let initial_ibl = [
        render_state.ibl_diffuse,
        render_state.ibl_specular,
        render_state.ibl_brdf_lut,
    ];

    // At most one mesh instance per entity
    let max_instances = engine
        .config
        .get_int("MAX_ENTITIES")
        .unwrap_or(MAX_ENTITIES as i64) as usize;
    let mut passes: Vec<Box<dyn Pass>> = vec![Box::new(PassPBROpaque::new(
        hdr_target,
        depth_target,
        max_instances,
        initial_ibl,
    ))];
    passes.append(&mut engine.custom_render_passes);
    passes.push(Box::new(PassTonemap::new(hdr_target)));

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

    Ok(passes)
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
// 1×1 Rgba8Unorm — BRDF LUT fallback: scale 1, bias 0, so specular IBL = prefiltered · F0
pub const DEFAULT_BRDF_LUT_FALLBACK_PIXEL: &[u8] = &[255, 0, 0, 255];

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

// Default resource handle - Color texture
pub const DEFAULT_COLOR_TEXTURE_HANDLE: TextureHandle = TextureHandle(PillSlotMapKeyData {
    index: 1,
    version: std::num::NonZeroU32::new(1).unwrap(),
});
pub const DEFAULT_RENDERER_COLOR_TEXTURE_HANDLE: RendererTextureHandle =
    RendererTextureHandle(PillSlotMapKeyData {
        index: 1,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

// Default resource handle - Normal texture
pub const DEFAULT_NORMAL_TEXTURE_HANDLE: TextureHandle = TextureHandle(PillSlotMapKeyData {
    index: 2,
    version: std::num::NonZeroU32::new(1).unwrap(),
});
pub const DEFAULT_RENDERER_NORMAL_TEXTURE_HANDLE: RendererTextureHandle =
    RendererTextureHandle(PillSlotMapKeyData {
        index: 2,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

// Default resource handle - MetallicRoughness texture
pub const DEFAULT_METALLIC_ROUGHNESS_TEXTURE_HANDLE: TextureHandle =
    TextureHandle(PillSlotMapKeyData {
        index: 3,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });
pub const DEFAULT_RENDERER_METALLIC_ROUGHNESS_TEXTURE_HANDLE: RendererTextureHandle =
    RendererTextureHandle(PillSlotMapKeyData {
        index: 3,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

// Default resource handle - Emissive texture
pub const DEFAULT_EMISSIVE_TEXTURE_HANDLE: TextureHandle = TextureHandle(PillSlotMapKeyData {
    index: 4,
    version: std::num::NonZeroU32::new(1).unwrap(),
});
pub const DEFAULT_RENDERER_EMISSIVE_TEXTURE_HANDLE: RendererTextureHandle =
    RendererTextureHandle(PillSlotMapKeyData {
        index: 4,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

pub fn get_default_texture_handles(
    texture_type: TextureType,
) -> (TextureHandle, RendererTextureHandle) {
    match texture_type {
        TextureType::Color => (
            DEFAULT_COLOR_TEXTURE_HANDLE,
            DEFAULT_RENDERER_COLOR_TEXTURE_HANDLE,
        ),
        TextureType::Normal => (
            DEFAULT_NORMAL_TEXTURE_HANDLE,
            DEFAULT_RENDERER_NORMAL_TEXTURE_HANDLE,
        ),
        TextureType::MetallicRoughness => (
            DEFAULT_METALLIC_ROUGHNESS_TEXTURE_HANDLE,
            DEFAULT_RENDERER_METALLIC_ROUGHNESS_TEXTURE_HANDLE,
        ),
        TextureType::Emissive => (
            DEFAULT_EMISSIVE_TEXTURE_HANDLE,
            DEFAULT_RENDERER_EMISSIVE_TEXTURE_HANDLE,
        ),
    }
}

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
        ];
        #[cfg(not(target_arch = "wasm32"))]
        component_types.push(TypeId::of::<AudioManagerComponent>());
        #[cfg(feature = "ui")]
        component_types.push(TypeId::of::<crate::ecs::EguiComponent>());
        component_types
    };
}
