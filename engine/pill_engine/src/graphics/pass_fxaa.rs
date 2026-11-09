use crate::config::{FXAA_FRAGMENT_SHADER, HDR_TARGET_FORMAT};
use crate::graphics::fullscreen::{
    create_fullscreen_pipeline, draw_fullscreen, linear_clamp_sampler, sampler_entry, target_view,
    texture_entry, uniform_buffer, uniform_entry, Binding, CachedBindGroup,
};
use crate::graphics::{FrameTarget, Pass, PassTextureHandle, PillRenderer, Pipeline, WorldQuery};

use pill_core::Result;

/// Mirrors `FxaaParams` in fxaa_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FxaaParams {
    reciprocal_frame: [f32; 2],
    subpixel: f32,
    edge_threshold: f32,
    edge_threshold_min: f32,
    _padding: [f32; 3],
}

struct FxaaState {
    pipeline: Pipeline,
    sampler: wgpu::Sampler,
    params_buffer: wgpu::Buffer,
    bind_group: CachedBindGroup,
}

/// FXAA 3.11 (Lottes, quality preset 39) over the tonemapped, still linear image.
pub struct PassFxaa {
    input: PassTextureHandle,
    output: PassTextureHandle,
    /// Sub-pixel aliasing removal, 0..1; lower is sharper.
    pub subpixel: f32,
    /// Minimum local contrast to filter, 0.063..0.333.
    pub edge_threshold: f32,
    /// Darks below this are left alone, 0.0312..0.0833.
    pub edge_threshold_min: f32,
    state: Option<FxaaState>,
}

impl PassFxaa {
    pub fn new(input: PassTextureHandle, output: PassTextureHandle) -> Self {
        Self {
            input,
            output,
            // Strongest settings of the FXAA 3.11 ranges: on HiDPI the render is upscaled 2×, so
            // every edge FXAA leaves aliased shows as 2-pixel steps
            subpixel: 1.0,
            edge_threshold: 0.063,
            edge_threshold_min: 0.0312,
            state: None,
        }
    }
}

impl Pass for PassFxaa {
    fn get_label(&self) -> &str {
        "pass_fxaa"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        let pipeline = create_fullscreen_pipeline(
            renderer,
            "pass_fxaa",
            FXAA_FRAGMENT_SHADER,
            vec![texture_entry(0, true), sampler_entry(1), uniform_entry(2)],
            HDR_TARGET_FORMAT,
            None,
        )?;
        self.state = Some(FxaaState {
            pipeline,
            sampler: linear_clamp_sampler(renderer),
            params_buffer: uniform_buffer::<FxaaParams>(renderer, "pass_fxaa_params"),
            bind_group: CachedBindGroup::default(),
        });
        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        _frame_target: &FrameTarget,
        _world: &WorldQuery<'_>,
    ) -> Result<()> {
        let state = self
            .state
            .as_mut()
            .expect("PassFxaa: state not initialized — call init() before draw()");
        let (width, height) = renderer.get_render_size();
        let params = FxaaParams {
            reciprocal_frame: [1.0 / width.max(1) as f32, 1.0 / height.max(1) as f32],
            subpixel: self.subpixel,
            edge_threshold: self.edge_threshold,
            edge_threshold_min: self.edge_threshold_min,
            _padding: [0.0; 3],
        };
        renderer
            .get_queue()
            .write_buffer(&state.params_buffer, 0, bytemuck::bytes_of(&params));
        draw_fullscreen(
            encoder,
            renderer,
            "pass_fxaa",
            &state.pipeline,
            &mut state.bind_group,
            &[
                Binding::Target(self.input),
                Binding::Sampler(&state.sampler),
                Binding::Buffer(&state.params_buffer),
            ],
            target_view(renderer, self.output, "pass_fxaa")?,
        )
    }
}
