use crate::config::{
    BLOOM_COMPOSITE_FRAGMENT_SHADER, BLOOM_DOWNSAMPLE_FRAGMENT_SHADER,
    BLOOM_UPSAMPLE_FRAGMENT_SHADER, HDR_TARGET_FORMAT,
};
use crate::ecs::RenderStateComponent;
use crate::graphics::fullscreen::{
    create_fullscreen_pipeline, draw_fullscreen, draw_fullscreen_with_load, linear_clamp_sampler,
    sampler_entry, target_view, texture_entry, uniform_buffer, uniform_entry, Binding,
    CachedBindGroup,
};
use crate::graphics::{
    FrameTarget, Pass, PassTextureHandle, PillRenderer, Pipeline, RendererTargetDesc, TargetSize,
    WorldQuery,
};

use pill_core::Result;

/// Mirrors `BloomDownsampleParams` in bloom_downsample_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DownsampleParams {
    karis_average: u32,
    _padding: [f32; 3],
}

/// Mirrors `BloomUpsampleParams` in bloom_upsample_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct UpsampleParams {
    radius: [f32; 2],
    _padding: [f32; 2],
}

/// Mirrors `BloomCompositeParams` in bloom_composite_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CompositeParams {
    strength: f32,
    _padding: [f32; 3],
}

struct BloomState {
    downsample: Pipeline,
    upsample: Pipeline,
    composite: Pipeline,
    sampler: wgpu::Sampler,
    // Separate buffers: all levels are recorded into one encoder, so one buffer can't vary per draw
    karis_params: wgpu::Buffer,
    plain_params: wgpu::Buffer,
    upsample_params: wgpu::Buffer,
    composite_params: wgpu::Buffer,
    // 1/2, 1/4, ... of the surface
    mips: Vec<PassTextureHandle>,
    // One per draw: downsample into mip i, upsample into mip i, composite
    downsample_bind_groups: Vec<CachedBindGroup>,
    upsample_bind_groups: Vec<CachedBindGroup>,
    composite_bind_group: CachedBindGroup,
}

/// Physically based bloom (Jimenez 2014): the input is downsampled into a mip chain, the chain is
/// upsampled back with a tent filter accumulating every level, and `input + bloom × strength` is
/// written to `output`. Strength and radius come from `RenderStateComponent`.
pub struct PassBloom {
    input: PassTextureHandle,
    output: PassTextureHandle,
    /// Levels below the input, each half the previous.
    pub mip_count: u32,
    state: Option<BloomState>,
}

impl PassBloom {
    pub fn new(input: PassTextureHandle, output: PassTextureHandle) -> Self {
        Self {
            input,
            output,
            mip_count: 5,
            state: None,
        }
    }
}

impl Pass for PassBloom {
    fn get_label(&self) -> &str {
        "pass_bloom"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        let mut mips = Vec::with_capacity(self.mip_count as usize);
        for level in 0..self.mip_count {
            mips.push(renderer.create_render_target(RendererTargetDesc {
                name: format!("bloom_mip_{level}"),
                format: HDR_TARGET_FORMAT,
                size: TargetSize::SurfaceDivisor(2 << level),
            })?);
        }
        let single_input = || vec![texture_entry(0, true), sampler_entry(1), uniform_entry(2)];
        let downsample = create_fullscreen_pipeline(
            renderer,
            "pass_bloom_downsample",
            BLOOM_DOWNSAMPLE_FRAGMENT_SHADER,
            single_input(),
            HDR_TARGET_FORMAT,
            None,
        )?;
        let upsample = create_fullscreen_pipeline(
            renderer,
            "pass_bloom_upsample",
            BLOOM_UPSAMPLE_FRAGMENT_SHADER,
            single_input(),
            HDR_TARGET_FORMAT,
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            }),
        )?;
        let composite = create_fullscreen_pipeline(
            renderer,
            "pass_bloom_composite",
            BLOOM_COMPOSITE_FRAGMENT_SHADER,
            vec![
                texture_entry(0, true),
                texture_entry(1, true),
                sampler_entry(2),
                uniform_entry(3),
            ],
            HDR_TARGET_FORMAT,
            None,
        )?;

        let karis_params = uniform_buffer::<DownsampleParams>(renderer, "pass_bloom_karis");
        let plain_params = uniform_buffer::<DownsampleParams>(renderer, "pass_bloom_plain");
        let queue = renderer.get_queue();
        for (buffer, karis_average) in [(&karis_params, 1), (&plain_params, 0)] {
            queue.write_buffer(
                buffer,
                0,
                bytemuck::bytes_of(&DownsampleParams {
                    karis_average,
                    _padding: [0.0; 3],
                }),
            );
        }

        self.state = Some(BloomState {
            downsample,
            upsample,
            composite,
            sampler: linear_clamp_sampler(renderer),
            karis_params,
            plain_params,
            upsample_params: uniform_buffer::<UpsampleParams>(renderer, "pass_bloom_upsample"),
            composite_params: uniform_buffer::<CompositeParams>(renderer, "pass_bloom_composite"),
            downsample_bind_groups: mips.iter().map(|_| CachedBindGroup::default()).collect(),
            upsample_bind_groups: mips.iter().map(|_| CachedBindGroup::default()).collect(),
            composite_bind_group: CachedBindGroup::default(),
            mips,
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
            .expect("PassBloom: state not initialized — call init() before draw()");
        let render_state = world.get_global::<RenderStateComponent>()?;
        let (width, height) = renderer.get_surface_size();
        let aspect = width as f32 / height.max(1) as f32;
        let queue = renderer.get_queue();
        queue.write_buffer(
            &state.upsample_params,
            0,
            bytemuck::bytes_of(&UpsampleParams {
                radius: [
                    render_state.bloom.radius,
                    render_state.bloom.radius * aspect,
                ],
                _padding: [0.0; 2],
            }),
        );
        queue.write_buffer(
            &state.composite_params,
            0,
            bytemuck::bytes_of(&CompositeParams {
                strength: render_state.bloom.strength,
                _padding: [0.0; 3],
            }),
        );

        // Off: only the composite, which then copies the input through
        let enabled = render_state.bloom.strength != 0.0;

        // Down: input → mip 0 (Karis) → mip 1 → ...
        let mut source = self.input;
        for (level, &mip) in state.mips.iter().enumerate().take_while(|_| enabled) {
            let params = if level == 0 {
                &state.karis_params
            } else {
                &state.plain_params
            };
            draw_fullscreen(
                encoder,
                renderer,
                "pass_bloom_downsample",
                &state.downsample,
                &mut state.downsample_bind_groups[level],
                &[
                    Binding::Target(source),
                    Binding::Sampler(&state.sampler),
                    Binding::Buffer(params),
                ],
                target_view(renderer, mip, "pass_bloom")?,
            )?;
            source = mip;
        }
        // Up: each level is added onto the next larger one, so mip 0 ends with the whole chain
        for (level, pair) in state
            .mips
            .windows(2)
            .enumerate()
            .rev()
            .take_while(|_| enabled)
        {
            draw_fullscreen_with_load(
                encoder,
                renderer,
                "pass_bloom_upsample",
                &state.upsample,
                &mut state.upsample_bind_groups[level],
                &[
                    Binding::Target(pair[1]),
                    Binding::Sampler(&state.sampler),
                    Binding::Buffer(&state.upsample_params),
                ],
                target_view(renderer, pair[0], "pass_bloom")?,
                wgpu::LoadOp::Load,
            )?;
        }
        let Some(&bloom) = state.mips.first() else {
            return Ok(());
        };
        draw_fullscreen(
            encoder,
            renderer,
            "pass_bloom_composite",
            &state.composite,
            &mut state.composite_bind_group,
            &[
                Binding::Target(self.input),
                Binding::Target(bloom),
                Binding::Sampler(&state.sampler),
                Binding::Buffer(&state.composite_params),
            ],
            target_view(renderer, self.output, "pass_bloom")?,
        )
    }
}
