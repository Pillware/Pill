use crate::config::LENS_FRAGMENT_SHADER;
use crate::ecs::RenderStateComponent;
use crate::graphics::fullscreen::{
    create_fullscreen_pipeline, draw_fullscreen, linear_clamp_sampler, sampler_entry,
    texture_entry, uniform_buffer, uniform_entry, Binding, CachedBindGroup,
};
use crate::graphics::{
    FrameTarget, Pass, PassTextureHandle, PillRenderer, Pipeline, TextureData, WorldQuery,
};

use pill_core::Result;

const GRAIN_SIZE: usize = 256;

/// Mirrors `LensParams` in lens_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LensParams {
    ca_scale: f32,
    distortion: f32,
    vignette: f32,
    grain_strength: f32,
    grain_offset_x: i32,
    grain_offset_y: i32,
    grain_pixel_scale: f32,
    aspect_yx: f32,
    grade_gamma: [f32; 4],
    grade_params: [f32; 4],
}

struct LensState {
    pipeline: Pipeline,
    sampler: wgpu::Sampler,
    grain_sampler: wgpu::Sampler,
    grain_texture: PassTextureHandle,
    params_buffer: wgpu::Buffer,
    bind_group: CachedBindGroup,
}

/// Final pass of the PostFx chain: lens distortion, chromatic aberration, grade, vignette and film
/// grain from `RenderStateComponent::lens`, written to the swapchain (sRGB view).
pub struct PassLens {
    input: PassTextureHandle,
    // Grain pattern index; advances `lens.grain_fps` times per second
    grain_frame: u32,
    grain_time: f32,
    state: Option<LensState>,
}

impl PassLens {
    pub fn new(input: PassTextureHandle) -> Self {
        Self {
            input,
            grain_frame: 0,
            grain_time: 0.0,
            state: None,
        }
    }
}

/// Halton(2, 3) point `index` scaled to the grain tile: 1024 well-spread offsets without visible
/// periodicity.
fn halton_offset(index: u32) -> (i32, i32) {
    fn radical_inverse(mut n: u32, base: u32) -> f32 {
        let (mut fraction, mut result) = (1.0_f32, 0.0_f32);
        while n > 0 {
            fraction /= base as f32;
            result += fraction * (n % base) as f32;
            n /= base;
        }
        result
    }
    let n = index % 1024 + 1;
    let size = GRAIN_SIZE as f32;
    (
        (radical_inverse(n, 2) * size) as i32,
        (radical_inverse(n, 3) * size) as i32,
    )
}

/// 256×256 RGBA8 high-passed white noise (center minus its 4-neighbour mean), independent per
/// channel: fine grain without low-frequency blotches.
fn grain_pixels() -> Vec<u8> {
    const N: usize = GRAIN_SIZE;
    let mut state: u32 = 0xdead_beef;
    let mut random = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / 16_777_216.0 * 2.0 - 1.0
    };
    let noise: Vec<[f32; 4]> = (0..N * N)
        .map(|_| [random(), random(), random(), random()])
        .collect();
    let mut pixels = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let index = y * N + x;
            let left = noise[y * N + (x + N - 1) % N];
            let right = noise[y * N + (x + 1) % N];
            let up = noise[((y + N - 1) % N) * N + x];
            let down = noise[((y + 1) % N) * N + x];
            for channel in 0..4 {
                // In [-1, 1] after halving, then packed to UNORM
                let high_pass = (noise[index][channel]
                    - (left[channel] + right[channel] + up[channel] + down[channel]) * 0.25)
                    * 0.5;
                pixels[index * 4 + channel] = ((high_pass * 0.5 + 0.5) * 255.0 + 0.5) as u8;
            }
        }
    }
    pixels
}

/// CA coefficient that splits R/B by `strength` UV units at the frame corner.
fn chromatic_aberration_scale(strength: f32, aspect_yx: f32) -> f32 {
    let corner = 0.5_f32.sqrt() * 0.25 * (aspect_yx * aspect_yx + 1.0);
    strength / corner
}

impl Pass for PassLens {
    fn get_label(&self) -> &str {
        "pass_lens"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        // Writes FrameTarget::srgb_view: the GPU encodes the linear output
        let output_format = renderer.get_surface_format().add_srgb_suffix();
        let pipeline = create_fullscreen_pipeline(
            renderer,
            "pass_lens",
            LENS_FRAGMENT_SHADER,
            vec![
                texture_entry(0, true),
                sampler_entry(1),
                uniform_entry(2),
                texture_entry(3, true),
                sampler_entry(4),
            ],
            output_format,
            None,
        )?;
        let grain_texture = renderer.create_texture_from_pixels(
            "pass_lens_grain",
            TextureData {
                width: GRAIN_SIZE as u32,
                height: GRAIN_SIZE as u32,
                format: wgpu::TextureFormat::Rgba8Unorm,
                mips: &[&grain_pixels()],
            },
        )?;
        let grain_sampler = renderer
            .get_device()
            .create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
        self.state = Some(LensState {
            pipeline,
            sampler: linear_clamp_sampler(renderer),
            grain_sampler,
            grain_texture,
            params_buffer: uniform_buffer::<LensParams>(renderer, "pass_lens_params"),
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
        let lens = world.get_global::<RenderStateComponent>()?.lens;

        // Advance the grain pattern at grain_fps, keeping the remainder so the rate stays exact
        if lens.grain_fps > 0.0 {
            let interval = 1.0 / lens.grain_fps;
            // delta_time is in milliseconds
            self.grain_time += world.delta_time / 1000.0;
            if self.grain_time >= interval {
                self.grain_time %= interval;
                self.grain_frame = self.grain_frame.wrapping_add(1);
            }
        } else {
            self.grain_frame = self.grain_frame.wrapping_add(1);
        }

        let state = self
            .state
            .as_mut()
            .expect("PassLens: state not initialized — call init() before draw()");
        let (width, height) = renderer.get_surface_size();
        let aspect_yx = height as f32 / width.max(1) as f32;
        let (grain_offset_x, grain_offset_y) = halton_offset(self.grain_frame);
        let [red, green, blue] = lens.grade_gamma;
        let params = LensParams {
            ca_scale: chromatic_aberration_scale(lens.chromatic_aberration, aspect_yx),
            distortion: lens.distortion,
            vignette: lens.vignette,
            grain_strength: lens.grain_strength,
            grain_offset_x,
            grain_offset_y,
            grain_pixel_scale: (lens.grain_size * width as f32 / 1280.0).max(1e-3),
            aspect_yx,
            grade_gamma: [red, green, blue, 0.0],
            grade_params: [lens.grade_edge0, lens.grade_edge1, lens.grade_mix, 0.0],
        };
        renderer
            .get_queue()
            .write_buffer(&state.params_buffer, 0, bytemuck::bytes_of(&params));
        draw_fullscreen(
            encoder,
            renderer,
            "pass_lens",
            &state.pipeline,
            &mut state.bind_group,
            &[
                Binding::Target(self.input),
                Binding::Sampler(&state.sampler),
                Binding::Buffer(&state.params_buffer),
                Binding::Target(state.grain_texture),
                Binding::Sampler(&state.grain_sampler),
            ],
            &frame_target.srgb_view,
        )
    }
}
