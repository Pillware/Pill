use crate::config::{
    DOF_BOKEH_FRAGMENT_SHADER, DOF_DOWNSAMPLE_FRAGMENT_SHADER, DOF_NEAR_BLUR_FRAGMENT_SHADER,
    DOF_NEAR_COMPOSITE_FRAGMENT_SHADER, DOF_NEAR_PREFILTER_FRAGMENT_SHADER, HDR_TARGET_FORMAT,
    LINEARIZE_DEPTH_FRAGMENT_SHADER,
};
use crate::ecs::RenderStateComponent;
use crate::graphics::fullscreen::{
    create_fullscreen_pipeline, draw_fullscreen, linear_clamp_sampler, sampler_entry, target_view,
    texture_entry, uniform_buffer, uniform_entry, Binding, CachedBindGroup,
};
use crate::graphics::projection::depth_unpack;
use crate::graphics::{
    FrameTarget, Pass, PassTextureHandle, PillRenderer, Pipeline, RendererTargetDesc, TargetSize,
    WorldQuery,
};

use pill_core::Result;

const LINEAR_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

/// Near-layer Gaussian sigma in eighth-res pixels at `near_blur` 1 (3σ ≈ 65 full-res pixels).
const NEAR_SIGMA_PX: f32 = 2.7;
/// Past sigma 4 the 25 taps leave gaps and thin near features alias through.
const NEAR_SIGMA_MAX_PX: f32 = 4.0;

/// Mirrors `DepthUnpack` in linearize_depth_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DepthUnpack {
    depth_mul: f32,
    depth_add: f32,
    _padding: [f32; 2],
}

/// Mirrors `DofParams` in dof_bokeh_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DofParams {
    focus_point: f32,
    focus_scale: f32,
    squeeze_x: f32,
    _padding: f32,
}

/// Mirrors `DofNearParams` in the dof_near_*.hlsl shaders.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DofNearParams {
    focus_point: f32,
    focus_scale: f32,
    near_blur_scale: f32,
    near_cut: f32,
    near_sigma_px: f32,
    _padding: f32,
    blur_dir: [f32; 2],
}

struct Stage {
    pipeline: Pipeline,
    params: wgpu::Buffer,
}

struct DepthOfFieldState {
    linearize: Stage,
    downsample: Pipeline,
    bokeh: Stage,
    near_prefilter: Stage,
    near_blur: Stage,
    near_composite: Stage,
    sampler: wgpu::Sampler,
    linear_depth: PassTextureHandle,   // full res
    quarter: PassTextureHandle,        // downsampled color
    far: PassTextureHandle,            // quarter-res far bokeh, alpha = CoC²
    near: PassTextureHandle,           // eighth-res premultiplied near layer
    near_blurred: PassTextureHandle,   // ... blurred horizontally
    bind_groups: [CachedBindGroup; 6], // one per step, in draw order
}

/// Depth of field after Sousa (SIGGRAPH 2013, CryENGINE 3): a quarter-res far-field gather on a
/// golden-angle spiral, plus an eighth-res near layer (premultiplied by CoC, separable Gaussian)
/// composited over it. `output` (quarter res) holds the blurred image with the blend factor in
/// alpha; `PassMotionBlur::with_depth_of_field` composites it with the full-res image.
/// Parameters: `RenderStateComponent::depth_of_field`.
pub struct PassDepthOfField {
    color: PassTextureHandle,
    depth: PassTextureHandle,
    output: PassTextureHandle,
    state: Option<DepthOfFieldState>,
}

impl PassDepthOfField {
    /// `output`: an HDR_TARGET_FORMAT target at a quarter of the surface (`SurfaceDivisor(4)`).
    pub fn new(
        color: PassTextureHandle,
        depth: PassTextureHandle,
        output: PassTextureHandle,
    ) -> Self {
        Self {
            color,
            depth,
            output,
            state: None,
        }
    }
}

fn target(
    renderer: &mut dyn PillRenderer,
    name: &str,
    format: wgpu::TextureFormat,
    divisor: u32,
) -> Result<PassTextureHandle> {
    renderer.create_render_target(RendererTargetDesc {
        name: name.to_string(),
        format,
        size: TargetSize::SurfaceDivisor(divisor),
    })
}

fn stage<T>(
    renderer: &mut dyn PillRenderer,
    label: &str,
    shader: &str,
    entries: Vec<wgpu::BindGroupLayoutEntry>,
    format: wgpu::TextureFormat,
) -> Result<Stage> {
    Ok(Stage {
        pipeline: create_fullscreen_pipeline(renderer, label, shader, entries, format, None)?,
        params: uniform_buffer::<T>(renderer, label),
    })
}

impl Pass for PassDepthOfField {
    fn get_label(&self) -> &str {
        "pass_depth_of_field"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        let linear_depth = target(renderer, "dof_linear_depth", LINEAR_DEPTH_FORMAT, 1)?;
        let quarter = target(renderer, "dof_quarter", HDR_TARGET_FORMAT, 4)?;
        let far = target(renderer, "dof_far", HDR_TARGET_FORMAT, 4)?;
        let near = target(renderer, "dof_near", HDR_TARGET_FORMAT, 8)?;
        let near_blurred = target(renderer, "dof_near_blurred", HDR_TARGET_FORMAT, 8)?;

        let linearize = stage::<DepthUnpack>(
            renderer,
            "pass_dof_linearize_depth",
            LINEARIZE_DEPTH_FRAGMENT_SHADER,
            vec![texture_entry(0, false), uniform_entry(1)],
            LINEAR_DEPTH_FORMAT,
        )?;
        let downsample = create_fullscreen_pipeline(
            renderer,
            "pass_dof_downsample",
            DOF_DOWNSAMPLE_FRAGMENT_SHADER,
            vec![texture_entry(0, true)],
            HDR_TARGET_FORMAT,
            None,
        )?;
        let bokeh = stage::<DofParams>(
            renderer,
            "pass_dof_bokeh",
            DOF_BOKEH_FRAGMENT_SHADER,
            vec![
                texture_entry(0, true),
                texture_entry(1, true),
                uniform_entry(2),
                sampler_entry(3),
            ],
            HDR_TARGET_FORMAT,
        )?;
        let near_prefilter = stage::<DofNearParams>(
            renderer,
            "pass_dof_near_prefilter",
            DOF_NEAR_PREFILTER_FRAGMENT_SHADER,
            vec![
                texture_entry(0, true),
                texture_entry(1, true),
                uniform_entry(2),
            ],
            HDR_TARGET_FORMAT,
        )?;
        let near_blur = stage::<DofNearParams>(
            renderer,
            "pass_dof_near_blur",
            DOF_NEAR_BLUR_FRAGMENT_SHADER,
            vec![texture_entry(0, true), uniform_entry(1), sampler_entry(2)],
            HDR_TARGET_FORMAT,
        )?;
        let near_composite = stage::<DofNearParams>(
            renderer,
            "pass_dof_near_composite",
            DOF_NEAR_COMPOSITE_FRAGMENT_SHADER,
            vec![
                texture_entry(0, true),
                texture_entry(1, true),
                uniform_entry(2),
                sampler_entry(3),
            ],
            HDR_TARGET_FORMAT,
        )?;

        self.state = Some(DepthOfFieldState {
            linearize,
            downsample,
            bokeh,
            near_prefilter,
            near_blur,
            near_composite,
            sampler: linear_clamp_sampler(renderer),
            linear_depth,
            quarter,
            far,
            near,
            near_blurred,
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
            .expect("PassDepthOfField: state not initialized — call init() before draw()");
        let settings = world.get_global::<RenderStateComponent>()?.depth_of_field;
        // Off: PassMotionBlur skips the composite, so the output is never read
        if settings.focus_scale == 0.0 {
            return Ok(());
        }
        let (camera, _) = world.active_camera()?;
        let [depth_mul, depth_add] = depth_unpack(camera.range.start, camera.range.end);

        let queue = renderer.get_queue();
        queue.write_buffer(
            &state.linearize.params,
            0,
            bytemuck::bytes_of(&DepthUnpack {
                depth_mul,
                depth_add,
                _padding: [0.0; 2],
            }),
        );
        queue.write_buffer(
            &state.bokeh.params,
            0,
            bytemuck::bytes_of(&DofParams {
                focus_point: settings.focus_distance,
                focus_scale: settings.focus_scale,
                squeeze_x: settings.anamorphic_squeeze,
                _padding: 0.0,
            }),
        );
        let near_params = |blur_dir| DofNearParams {
            focus_point: settings.focus_distance,
            focus_scale: settings.focus_scale,
            near_blur_scale: settings.near_blur,
            near_cut: settings.near_cut,
            near_sigma_px: (NEAR_SIGMA_PX * settings.near_blur).clamp(0.0, NEAR_SIGMA_MAX_PX),
            _padding: 0.0,
            blur_dir,
        };
        for (stage, blur_dir) in [
            (&state.near_prefilter, [1.0, 0.0]),
            (&state.near_blur, [1.0, 0.0]),
            (&state.near_composite, [0.0, 1.0]),
        ] {
            queue.write_buffer(&stage.params, 0, bytemuck::bytes_of(&near_params(blur_dir)));
        }

        let steps: [(&str, &Pipeline, Vec<Binding>, PassTextureHandle); 6] = [
            (
                "pass_dof_linearize_depth",
                &state.linearize.pipeline,
                vec![
                    Binding::Target(self.depth),
                    Binding::Buffer(&state.linearize.params),
                ],
                state.linear_depth,
            ),
            (
                "pass_dof_downsample",
                &state.downsample,
                vec![Binding::Target(self.color)],
                state.quarter,
            ),
            (
                "pass_dof_bokeh",
                &state.bokeh.pipeline,
                vec![
                    Binding::Target(state.quarter),
                    Binding::Target(state.linear_depth),
                    Binding::Buffer(&state.bokeh.params),
                    Binding::Sampler(&state.sampler),
                ],
                state.far,
            ),
            (
                "pass_dof_near_prefilter",
                &state.near_prefilter.pipeline,
                vec![
                    Binding::Target(self.color),
                    Binding::Target(state.linear_depth),
                    Binding::Buffer(&state.near_prefilter.params),
                ],
                state.near,
            ),
            (
                "pass_dof_near_blur",
                &state.near_blur.pipeline,
                vec![
                    Binding::Target(state.near),
                    Binding::Buffer(&state.near_blur.params),
                    Binding::Sampler(&state.sampler),
                ],
                state.near_blurred,
            ),
            (
                "pass_dof_near_composite",
                &state.near_composite.pipeline,
                vec![
                    Binding::Target(state.near_blurred),
                    Binding::Target(state.far),
                    Binding::Buffer(&state.near_composite.params),
                    Binding::Sampler(&state.sampler),
                ],
                self.output,
            ),
        ];
        for ((label, pipeline, bindings, output), bind_group) in
            steps.into_iter().zip(&mut state.bind_groups)
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
