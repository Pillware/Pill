use crate::config::{
    HDR_TARGET_FORMAT, SSAO_APPLY_FRAGMENT_SHADER, SSAO_BLUR_FRAGMENT_SHADER, SSAO_FRAGMENT_SHADER,
};
use crate::ecs::RenderStateComponent;
use crate::graphics::fullscreen::{
    create_fullscreen_pipeline, draw_fullscreen, target_view, texture_entry, uniform_buffer,
    uniform_entry, Binding, CachedBindGroup,
};
use crate::graphics::projection::depth_unpack;
use crate::graphics::{
    FrameTarget, Pass, PassTextureHandle, PillRenderer, Pipeline, RendererTargetDesc, TargetSize,
    WorldQuery,
};

use pill_core::Result;

/// (ao, view depth) at half resolution; read with Load only.
const SSAO_TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;
/// Blur spatial sigma in half-res pixels: the 9-tap kernel's edge at 2σ covers the AO pass's 4×4
/// rotation cell.
const BLUR_SIGMA_PX: f32 = 2.0;
/// Blur depth sigma as a fraction of view depth (an absolute threshold suits only one distance).
const BLUR_SIGMA_DEPTH_RELATIVE: f32 = 0.02;
const UPSAMPLE_SIGMA_DEPTH_RELATIVE: f32 = 0.01;

/// Mirrors `SsaoParams` in ssao_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SsaoParams {
    depth_mul: f32,
    depth_add: f32,
    tan_half_x: f32,
    tan_half_y: f32,
    radius: f32,
    intensity: f32,
    power: f32,
    _padding: f32,
}

/// Mirrors `SsaoBlurParams` in ssao_blur_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SsaoBlurParams {
    blur_dir: [f32; 2],
    sigma_px: f32,
    sigma_depth_relative: f32,
}

/// Mirrors `SsaoApplyParams` in ssao_apply_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SsaoApplyParams {
    strength: f32,
    sigma_depth_relative: f32,
    depth_mul: f32,
    depth_add: f32,
}

struct SsaoState {
    occlusion: Pipeline,
    blur: Pipeline,
    apply: Pipeline,
    occlusion_params: wgpu::Buffer,
    blur_horizontal_params: wgpu::Buffer,
    blur_vertical_params: wgpu::Buffer,
    apply_params: wgpu::Buffer,
    raw: PassTextureHandle,
    blurred_horizontal: PassTextureHandle,
    blurred: PassTextureHandle,
    bind_groups: [CachedBindGroup; 4], // one per step, in draw order
}

/// Scalable Ambient Obscurance (McGuire, Mara, Luebke, HPG 2012) at half resolution from the depth
/// buffer, smoothed by a separable depth-aware blur and upsampled bilaterally. Only the indirect
/// (IBL) light is darkened: `output = color − ambient · (1 − ao)`, with `ambient` from
/// `PassPBROpaque::with_ambient_target`. Parameters: `RenderStateComponent::ssao`.
pub struct PassSsao {
    color: PassTextureHandle,
    ambient: PassTextureHandle,
    depth: PassTextureHandle,
    output: PassTextureHandle,
    state: Option<SsaoState>,
}

impl PassSsao {
    pub fn new(
        color: PassTextureHandle,
        ambient: PassTextureHandle,
        depth: PassTextureHandle,
        output: PassTextureHandle,
    ) -> Self {
        Self {
            color,
            ambient,
            depth,
            output,
            state: None,
        }
    }
}

impl Pass for PassSsao {
    fn get_label(&self) -> &str {
        "pass_ssao"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        let mut half_target = |name: &str| {
            renderer.create_render_target(RendererTargetDesc {
                name: name.to_string(),
                format: SSAO_TARGET_FORMAT,
                size: TargetSize::SurfaceDivisor(2),
            })
        };
        let raw = half_target("ssao_raw")?;
        let blurred_horizontal = half_target("ssao_blurred_horizontal")?;
        let blurred = half_target("ssao_blurred")?;

        let occlusion = create_fullscreen_pipeline(
            renderer,
            "pass_ssao_occlusion",
            SSAO_FRAGMENT_SHADER,
            vec![texture_entry(0, false), uniform_entry(1)],
            SSAO_TARGET_FORMAT,
            None,
        )?;
        let blur = create_fullscreen_pipeline(
            renderer,
            "pass_ssao_blur",
            SSAO_BLUR_FRAGMENT_SHADER,
            vec![texture_entry(0, false), uniform_entry(1)],
            SSAO_TARGET_FORMAT,
            None,
        )?;
        let apply = create_fullscreen_pipeline(
            renderer,
            "pass_ssao_apply",
            SSAO_APPLY_FRAGMENT_SHADER,
            vec![
                texture_entry(0, false),
                texture_entry(1, false),
                texture_entry(2, false),
                texture_entry(3, false),
                uniform_entry(4),
            ],
            HDR_TARGET_FORMAT,
            None,
        )?;

        let blur_horizontal_params = uniform_buffer::<SsaoBlurParams>(renderer, "pass_ssao_blur_h");
        let blur_vertical_params = uniform_buffer::<SsaoBlurParams>(renderer, "pass_ssao_blur_v");
        for (buffer, blur_dir) in [
            (&blur_horizontal_params, [1.0, 0.0]),
            (&blur_vertical_params, [0.0, 1.0]),
        ] {
            renderer.get_queue().write_buffer(
                buffer,
                0,
                bytemuck::bytes_of(&SsaoBlurParams {
                    blur_dir,
                    sigma_px: BLUR_SIGMA_PX,
                    sigma_depth_relative: BLUR_SIGMA_DEPTH_RELATIVE,
                }),
            );
        }

        self.state = Some(SsaoState {
            occlusion,
            blur,
            apply,
            occlusion_params: uniform_buffer::<SsaoParams>(renderer, "pass_ssao_params"),
            blur_horizontal_params,
            blur_vertical_params,
            apply_params: uniform_buffer::<SsaoApplyParams>(renderer, "pass_ssao_apply"),
            raw,
            blurred_horizontal,
            blurred,
            bind_groups: Default::default(),
        });
        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        _frame_target: &FrameTarget,
        world: &WorldQuery<'_>,
    ) -> Result<()> {
        let state = self
            .state
            .as_mut()
            .expect("PassSsao: state not initialized — call init() before draw()");
        let settings = world.get_global::<RenderStateComponent>()?.ssao;
        let (camera, _) = world.active_camera()?;
        let [depth_mul, depth_add] = depth_unpack(camera.range.start, camera.range.end);
        let tan_half_y = (camera.fov.to_radians() * 0.5).tan();

        let queue = renderer.get_queue();
        queue.write_buffer(
            &state.occlusion_params,
            0,
            bytemuck::bytes_of(&SsaoParams {
                depth_mul,
                depth_add,
                tan_half_x: tan_half_y * camera.aspect.get_value(),
                tan_half_y,
                radius: settings.radius.max(1e-3),
                intensity: settings.intensity,
                power: settings.power,
                _padding: 0.0,
            }),
        );
        queue.write_buffer(
            &state.apply_params,
            0,
            bytemuck::bytes_of(&SsaoApplyParams {
                strength: settings.strength.clamp(0.0, 1.0),
                sigma_depth_relative: UPSAMPLE_SIGMA_DEPTH_RELATIVE,
                depth_mul,
                depth_add,
            }),
        );

        let steps: [(&str, &Pipeline, Vec<Binding>, PassTextureHandle); 4] = [
            (
                "pass_ssao_occlusion",
                &state.occlusion,
                vec![
                    Binding::Target(self.depth),
                    Binding::Buffer(&state.occlusion_params),
                ],
                state.raw,
            ),
            (
                "pass_ssao_blur_horizontal",
                &state.blur,
                vec![
                    Binding::Target(state.raw),
                    Binding::Buffer(&state.blur_horizontal_params),
                ],
                state.blurred_horizontal,
            ),
            (
                "pass_ssao_blur_vertical",
                &state.blur,
                vec![
                    Binding::Target(state.blurred_horizontal),
                    Binding::Buffer(&state.blur_vertical_params),
                ],
                state.blurred,
            ),
            (
                "pass_ssao_apply",
                &state.apply,
                vec![
                    Binding::Target(self.color),
                    Binding::Target(self.ambient),
                    Binding::Target(state.blurred),
                    Binding::Target(self.depth),
                    Binding::Buffer(&state.apply_params),
                ],
                self.output,
            ),
        ];
        // Off: only the apply step, which then copies the color through
        let skipped = if settings.strength <= 0.0 { 3 } else { 0 };
        for ((label, pipeline, bindings, output), bind_group) in
            steps.into_iter().zip(&mut state.bind_groups).skip(skipped)
        {
            draw_fullscreen(
                encoder,
                renderer,
                label,
                pipeline,
                bind_group,
                &bindings,
                target_view(renderer, output, label)?,
            )?;
        }
        Ok(())
    }
}
