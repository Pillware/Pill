//! Falling pills (the demo's "Build" and "Crew2" shots) rendered through `PassChain::PostFx`: a rain
//! of clear-coated pills with fog, an emissive orb and the post-processing passes configured in
//! `RenderStateComponent`. The camera cuts between the takes (see shots.rs).

use pill_engine::game::*;

use crate::falling_pills::{
    falling_pills_system, spawn_falling_pills, FallingPill, FallingPillsDirection,
};
use crate::mesh::{make_pill_mesh_bytes, make_sphere_mesh_bytes};
use crate::shots::{shot_system, ShotCycle};
use crate::tex_gen;

// Deep navy, also the fog color, so distant pills dissolve into the background
const BACKGROUND_COLOR: [f32; 3] = [0.003, 0.004, 0.0175];

pub struct Game {}

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        engine.toggle_system("audio_system", UpdatePhase::PostGame, false)?;

        let scene = engine.create_scene("falling_pills")?;
        engine.set_active_scene(scene)?;
        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<MeshRenderingComponent>(scene)?;
        engine.register_component::<CameraComponent>(scene)?;
        engine.register_component::<FallingPill>(scene)?;
        engine.add_global_component(ShotCycle { current: None })?;
        engine.add_global_component(FallingPillsDirection { rising: false })?;
        // Shot first, so the pills move in this frame's direction
        engine.add_system("shots", shot_system)?;
        engine.add_system("light_fog", light_fog_system)?;
        engine.add_system("falling_pills", falling_pills_system)?;

        setup_render_state(engine)?;

        // --- Resources ---
        let pill_mesh = engine.add_resource(Mesh::from_cooked_mesh_bytes(
            "pill",
            &make_pill_mesh_bytes(
                tex_gen::CAP_STACKS,
                tex_gen::SLICES,
                tex_gen::HEIGHT,
                tex_gen::EXTRUDE,
            ),
        )?)?;
        let sphere_mesh = engine.add_resource(Mesh::from_cooked_mesh_bytes(
            "sphere",
            &make_sphere_mesh_bytes(24, 32),
        )?)?;
        let floor_mesh = engine.add_resource(Mesh::cube("floor", 1.0))?;

        let pill_albedo = engine.add_resource(Texture::from_bytes_owned(
            "pill_albedo",
            TextureType::Color,
            tex_gen::albedo(),
        ))?;
        let pill_normal = engine.add_resource(Texture::from_bytes_owned(
            "pill_normal",
            TextureType::Normal,
            tex_gen::normal_map(),
        ))?;
        let pill_roughness = engine.add_resource(Texture::from_bytes_owned(
            "pill_roughness",
            TextureType::MetallicRoughness,
            tex_gen::roughness(),
        ))?;

        // The demo's pill: painted (dielectric), greasy roughness map, glossy clear coat on top
        let pill_material = engine.add_resource(
            Material::new("pill")
                .albedo_texture(pill_albedo)
                .normal_texture(pill_normal)
                .metallic_roughness_texture(pill_roughness)
                .metallic(0.0)
                .roughness(1.0)
                .clearcoat(0.3, 0.1),
        )?;
        let orb_material = engine.add_resource(
            Material::new("orb")
                .albedo(Color::new(0.0, 0.0, 0.0))
                .emissive(Color::new(40.0, 22.0, 9.0)),
        )?;
        let floor_material = engine.add_resource(
            Material::new("floor")
                .albedo(Color::new(0.05, 0.05, 0.06))
                .roughness(0.35),
        )?;

        // --- Entities ---
        spawn_falling_pills(engine, scene, &pill_mesh, &pill_material);

        // Emissive orb: the bloom source
        engine
            .build_entity(scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(-0.9, 2.0, 1.4))
                    .scale(Vector3f::splat(0.06))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&sphere_mesh)
                    .material(&orb_material)
                    .build(),
            )
            .build();

        // Floor the pills fall through: contact shadows for SSAO
        engine
            .build_entity(scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, -1.0, 0.0))
                    .scale(Vector3f::new(60.0, 0.02, 60.0))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&floor_mesh)
                    .material(&floor_material)
                    .build(),
            )
            .build();

        // Pose and fov are set by shot_system
        engine
            .build_entity(scene)
            .with_component(TransformComponent::default())
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(38.0)
                    .range(0.05..60.0)
                    .build(),
            )
            .build();

        Ok(())
    }
}

/// Post-processing chain, studio IBL, lights and per-pass parameters.
fn setup_render_state(engine: &mut Engine) -> Result<()> {
    let diffuse = engine.load_cooked_texture(
        "studio_diffuse_ibl",
        include_bytes!(concat!(env!("OUT_DIR"), "/ibl_diffuse.rtex")),
    )?;
    let specular = engine.load_cooked_texture(
        "studio_specular_ibl",
        include_bytes!(concat!(env!("OUT_DIR"), "/ibl_specular.rtex")),
    )?;

    let render_state = engine.get_global_component_mut::<RenderStateComponent>()?;
    render_state.pass_chain = PassChain::PostFx;
    render_state.ibl_diffuse = diffuse;
    render_state.ibl_specular = specular;
    render_state.ibl_intensity = 0.5;
    render_state.background = None;
    render_state.background_color = BACKGROUND_COLOR;
    render_state.fog_color = BACKGROUND_COLOR;
    render_state.fog_density = 0.174;
    // Camera looks +Z: warm key from upper camera-left, cool rim from behind, faint bounce from below
    render_state.lights = [
        DirectionalLight {
            direction: [0.45, -0.55, 0.70],
            color: [1.0, 0.92, 0.82],
            intensity: 2.4,
        },
        DirectionalLight {
            direction: [-0.30, -0.35, -0.89],
            color: [0.45, 0.55, 0.8],
            intensity: 1.6,
        },
        DirectionalLight {
            direction: [0.0, 1.0, 0.0],
            color: [0.8, 0.9, 1.0],
            intensity: 0.15,
        },
    ];
    // Post-processing values of the demo's Crew2 shot (half-shutter motion blur, soft glow,
    // slight pincushion, corner fringing, cool-shadow grade, fine grain)
    render_state.tonemap = Tonemap::lottes();
    render_state.motion_blur = 0.5;
    // Contact shadows where pills pass over the floor (darkens the IBL term only)
    render_state.ssao = Ssao {
        strength: 1.0,
        radius: 0.5,
        intensity: 1.5,
        power: 1.5,
    };
    render_state.bloom = Bloom {
        strength: 0.04,
        radius: 0.005,
    };
    render_state.lens = Lens {
        distortion: 0.01,
        chromatic_aberration: 0.002,
        vignette: 0.6,
        grain_strength: 0.00375,
        grade_gamma: [1.0, 1.0, 0.8],
        grade_edge0: -0.05,
        grade_edge1: 0.9,
        grade_mix: 1.0,
        ..Lens::default()
    };
    Ok(())
}

/// Light and fog state from the demo's light.seq.ron keyframes.
struct LightFog {
    // Background and fog color (linear RGB)
    background: [f32; 3],
    fog_density: f32,
    ibl_intensity: f32,
    // Key, fill, rim
    lights: [DirectionalLight; 3],
}

const fn light(direction: [f32; 3], color: [f32; 3], intensity: f32) -> DirectionalLight {
    DirectionalLight {
        direction,
        color,
        intensity,
    }
}

#[rustfmt::skip]
const LIGHT_FOG: [LightFog; 6] = [
    // Setup: navy void, warm key
    LightFog { background: [0.003, 0.004, 0.0175], fog_density: 0.26, ibl_intensity: 0.5, lights: [
        light([0.78, -0.40, 0.45],   [1.00, 0.97, 0.92], 1.0),
        light([-0.45, -0.30, -0.84], [0.80, 0.90, 1.00], 0.35),
        light([0.00, -0.55, -0.83],  [0.80, 0.90, 1.00], 0.8),
    ] },
    // ShotB / Build strobe: red key
    LightFog { background: [0.06, 0.0, 0.0], fog_density: 0.26, ibl_intensity: 0.0, lights: [
        light([0.18, -0.5, 0.3],     [5.0, 0.0, 0.0],    10.0),
        light([-0.10, -0.5, 0.40],   [0.45, 0.55, 0.80], 0.3),
        light([0.10, -0.25, -0.3],   [0.80, 0.90, 1.00], 2.0),
    ] },
    // ShotB / Build strobe: blue key
    LightFog { background: [0.0, 0.0, 0.06], fog_density: 0.26, ibl_intensity: 0.0, lights: [
        light([0.18, -0.5, -0.3],    [0.0, 0.0, 1.0],    10.0),
        light([0.90, -0.2, 0.40],    [0.45, 0.55, 0.80], 4.0),
        light([0.00, -0.55, -0.83],  [0.80, 0.90, 1.00], 1.5),
    ] },
    // ShotA: warm key, cool fill
    LightFog { background: [0.003, 0.004, 0.0175], fog_density: 0.26, ibl_intensity: 0.2, lights: [
        light([0.78, -0.40, 0.45],   [1.00, 0.97, 0.92], 2.2),
        light([-0.80, -0.42, 0.40],  [0.45, 0.55, 0.80], 2.0),
        light([-0.45, -0.30, -0.84], [0.80, 0.90, 1.00], 1.2),
    ] },
    // Crew2: deep blue field, dim red key from above
    LightFog { background: [0.03, 0.04, 0.2], fog_density: 0.174, ibl_intensity: 0.5, lights: [
        light([0.0, -1.0, -0.3],     [3.0, 0.0, 0.0],    0.1),
        light([0.90, -0.2, 0.40],    [0.45, 0.55, 0.80], 0.2),
        light([-0.90, -1.0, -0.4],   [0.80, 0.90, 1.00], 0.2),
    ] },
    // Silence: red fill and rim
    LightFog { background: [0.02, 0.03, 0.1], fog_density: 0.26, ibl_intensity: 0.5, lights: [
        light([0.0, -1.0, -0.3],     [3.0, 0.0, 0.0],    1.0),
        light([0.90, -0.2, 0.40],    [0.90, 0.12, 0.08], 2.0),
        light([-0.90, -1.0, -0.4],   [1.00, 0.10, 0.05], 0.5),
    ] },
];

/// Every second: a pseudo-random LIGHT_FOG setup (hash of the second).
fn light_fog_system(engine: &mut Engine) -> Result<()> {
    let second = engine.get_global_component::<TimeComponent>()?.time.floor();
    let random = ((second * 12.9898).sin() * 43_758.547).fract().abs();
    let setup = &LIGHT_FOG[(random * LIGHT_FOG.len() as f32) as usize % LIGHT_FOG.len()];
    let render_state = engine.get_global_component_mut::<RenderStateComponent>()?;
    render_state.background_color = setup.background;
    render_state.fog_color = setup.background;
    render_state.fog_density = setup.fog_density;
    render_state.ibl_intensity = setup.ibl_intensity;
    render_state.lights = setup.lights;
    Ok(())
}
