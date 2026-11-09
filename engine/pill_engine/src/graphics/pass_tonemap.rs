use crate::config::{HDR_TARGET_FORMAT, TONEMAP_FRAGMENT_SHADER};
use crate::ecs::{RenderStateComponent, Tonemap};
use crate::graphics::fullscreen::{
    create_fullscreen_pipeline, draw_fullscreen, linear_clamp_sampler, sampler_entry, target_view,
    texture_entry, uniform_buffer, uniform_entry, Binding, CachedBindGroup,
};
use crate::graphics::{FrameTarget, Pass, PassTextureHandle, PillRenderer, Pipeline, WorldQuery};

use pill_core::Result;

pub struct PassTonemap {
    hdr_source: PassTextureHandle,
    // None: the swapchain (sRGB view); Some: an HDR_TARGET_FORMAT target for later post passes
    output: Option<PassTextureHandle>,
    state: Option<TonemapState>,
}

struct TonemapState {
    pipeline: Pipeline,
    sampler: wgpu::Sampler,
    params_buffer: wgpu::Buffer,
    bind_group: CachedBindGroup,
}

/// Mirrors `TonemapParams` in tonemap_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable, Default)]
struct TonemapParams {
    mode: u32,                  // 0 = pass-through, 1 = Reinhard, 2 = Lottes
    inverse_white_squared: f32, // Reinhard: 1/W², 0 for an infinite white point
    contrast: f32,              // Lottes a
    shoulder: f32,              // Lottes d
    b: f32,                     // Lottes b, c (solved from the art parameters)
    c: f32,
    _padding: [f32; 2],
}

impl TonemapParams {
    fn from(tonemap: Tonemap) -> Self {
        match tonemap {
            Tonemap::None => Self::default(),
            Tonemap::Reinhard { white_point } => Self {
                mode: 1,
                inverse_white_squared: 1.0 / (white_point * white_point),
                ..Self::default()
            },
            Tonemap::Lottes {
                contrast,
                shoulder,
                hdr_max,
                mid_in,
                mid_out,
            } => {
                let (b, c) = lottes_bc(contrast, shoulder, hdr_max, mid_in, mid_out);
                Self {
                    mode: 2,
                    contrast,
                    shoulder,
                    b,
                    c,
                    ..Self::default()
                }
            }
        }
    }
}

/// b and c of Lottes' curve x^a / (x^(a·d)·b + c) such that `mid_in` → `mid_out` and `hdr_max` → 1.
fn lottes_bc(a: f32, d: f32, hdr_max: f32, mid_in: f32, mid_out: f32) -> (f32, f32) {
    let hdr_a = hdr_max.powf(a);
    let mid_a = mid_in.powf(a);
    let hdr_ad = hdr_a.powf(d);
    let mid_ad = mid_a.powf(d);
    let denominator = (hdr_ad - mid_ad) * mid_out;
    let b = (hdr_a * mid_out - mid_a) / denominator;
    let c = (hdr_ad * mid_a - hdr_a * mid_ad * mid_out) / denominator;
    (b, c)
}

#[cfg(test)]
mod tests {
    use super::lottes_bc;

    fn lottes(x: f32, a: f32, d: f32, b: f32, c: f32) -> f32 {
        let xa = x.powf(a);
        xa / (xa.powf(d) * b + c)
    }

    #[test]
    fn lottes_bc_maps_mid_grey_and_white() {
        let (a, d) = (1.6, 0.977);
        let (b, c) = lottes_bc(a, d, 8.0, 0.18, 0.267);
        assert!((lottes(0.18, a, d, b, c) - 0.267).abs() < 1e-4);
        assert!((lottes(8.0, a, d, b, c) - 1.0).abs() < 1e-4);
    }
}

impl PassTonemap {
    pub fn new(hdr_source: PassTextureHandle) -> Self {
        Self {
            hdr_source,
            output: None,
            state: None,
        }
    }

    /// Writes the tonemapped (still linear) image to `target` instead of the swapchain.
    #[cfg_attr(not(feature = "post_fx"), allow(dead_code))]
    pub fn with_output(mut self, target: PassTextureHandle) -> Self {
        self.output = Some(target);
        self
    }
}

impl Pass for PassTonemap {
    fn get_label(&self) -> &str {
        "pass_tonemap"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        // To the swapchain: FrameTarget::srgb_view, so the GPU applies the OETF on write even when
        // the surface format is linear (e.g. WebGPU/wasm32)
        let output_format = match self.output {
            Some(_) => HDR_TARGET_FORMAT,
            None => renderer.get_surface_format().add_srgb_suffix(),
        };
        let pipeline = create_fullscreen_pipeline(
            renderer,
            "pass_tonemap",
            TONEMAP_FRAGMENT_SHADER,
            vec![texture_entry(0, true), sampler_entry(1), uniform_entry(2)],
            output_format,
            None,
        )?;
        self.state = Some(TonemapState {
            pipeline,
            sampler: linear_clamp_sampler(renderer),
            params_buffer: uniform_buffer::<TonemapParams>(renderer, "pass_tonemap_params"),
            bind_group: CachedBindGroup::default(),
        });
        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        frame_target: &FrameTarget,
        world: &WorldQuery<'_>,
    ) -> Result<()> {
        let state = self
            .state
            .as_mut()
            .expect("PassTonemap: state not initialized — call init() before draw()");
        let tonemap = world.get_global::<RenderStateComponent>()?.tonemap;
        renderer.get_queue().write_buffer(
            &state.params_buffer,
            0,
            bytemuck::bytes_of(&TonemapParams::from(tonemap)),
        );
        let output = match self.output {
            Some(handle) => target_view(renderer, handle, "pass_tonemap")?,
            None => &frame_target.srgb_view,
        };
        draw_fullscreen(
            encoder,
            renderer,
            "pass_tonemap",
            &state.pipeline,
            &mut state.bind_group,
            &[
                Binding::Target(self.hdr_source),
                Binding::Sampler(&state.sampler),
                Binding::Buffer(&state.params_buffer),
            ],
            output,
        )
    }
}
