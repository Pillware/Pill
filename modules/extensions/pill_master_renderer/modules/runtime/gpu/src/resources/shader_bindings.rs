//! The bind groups a shader's declared slots resolve to, built once for each
//! material or pass that has any.
//!
//! # Responsibilities
//!
//! - Pack the parameter values of a material or a pass into the uniform buffer
//!   its shader reads ([`write_parameters`], sized by [`uniform_size`]).
//! - Resolve every texture slot the shader declares into one textures bind
//!   group ([`shader_bind_groups`]): a texture the material or pass names, a
//!   source the caller resolves, or the renderer's stand-in.
//!
//! # Design
//!
//! To a shader, a material and a pass look the same: a set of parameter values
//! and one texture per declared slot. They differ only in where an extra
//! texture can come from. A pass reads a frame an earlier pass wrote; a
//! material has nothing beyond the store. That difference is one closure, and
//! everything else is shared here, so a change to the way a slot binds is made
//! in one file and the slots of both are read by the same code.
//!
//! Both groups follow the shader's declaration, not the maps the material or
//! pass carries. A value or texture named for a slot the shader never declares
//! is ignored, and a declared slot left unresolved gets the renderer's
//! stand-in for its type. Depth is refused instead: the renderer has no default
//! depth texture, so a slot that asks for depth has to name the source it
//! means.

// Standard library
use std::collections::BTreeMap;

// External crates
use indexmap::IndexMap;
use pill_renderer_api::frame::MaterialParameter;

// Current crate
use crate::{
    assets::{ShaderParameterSlot, ShaderParameterType, ShaderTextureSlot},
    error::{capturing_validation, ErrorContext, RendererError, Result},
    resources::{check_slot_shape, RendererResourceStorage, RendererShader, RendererTextureHandle},
};

// =============================================================================
// Free Functions
// =============================================================================

/// The byte size of the uniform buffer a shader's parameter slots need.
///
/// Each slot is one 16-byte region, aligned to `vec4` in WGSL, so the size is
/// the slot count times 16. A material and a pass both take their buffer size
/// from here, through [`shader_bind_groups`], so the two cannot disagree about
/// what the shader expects.
pub(crate) fn uniform_size(parameter_slots: &IndexMap<String, ShaderParameterSlot>) -> usize {
    parameter_slots.len() * 16
}

/// Packs the parameter values of a material or a pass into the uniform buffer
/// its shader reads.
///
/// Reads the shader's slots in declaration order and writes one 16-byte region
/// per slot, so each value is placed by the name the shader gave its uniform,
/// not by its position in the map. A slot with no value packs as zero, and so
/// does a value stored under that name with a different type, so a partly
/// filled material still draws.
pub(crate) fn write_parameters(
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    parameter_slots: &IndexMap<String, ShaderParameterSlot>,
    parameters: &BTreeMap<String, MaterialParameter>,
) {
    let mut packed_uniform_bytes = Vec::new();

    // Step 1: Pack one 16-byte region per declared slot. The padding is part of
    // it because WGSL aligns every parameter to `vec4`. Packing by type would
    // use less space, but the fixed region is simpler and packs a material and
    // a pass the same way.
    for (slot_name, slot) in parameter_slots {
        match slot.parameter_type {
            ShaderParameterType::Color => {
                if let Some(MaterialParameter::Color(value)) = parameters.get(slot_name) {
                    packed_uniform_bytes.extend_from_slice(&value[0].to_le_bytes());
                    packed_uniform_bytes.extend_from_slice(&value[1].to_le_bytes());
                    packed_uniform_bytes.extend_from_slice(&value[2].to_le_bytes());
                    packed_uniform_bytes.extend_from_slice(&0.0f32.to_le_bytes());
                // Padding
                } else {
                    packed_uniform_bytes.extend_from_slice(&[0u8; 16]);
                }
            }
            ShaderParameterType::Scalar => {
                if let Some(MaterialParameter::Scalar(value)) = parameters.get(slot_name) {
                    packed_uniform_bytes.extend_from_slice(&value.to_le_bytes());
                    packed_uniform_bytes.extend_from_slice(&[0u8; 12]); // Padding to 16 bytes
                } else {
                    packed_uniform_bytes.extend_from_slice(&[0u8; 16]);
                }
            }
            ShaderParameterType::Bool => {
                if let Some(MaterialParameter::Bool(value)) = parameters.get(slot_name) {
                    let packed_flag: u32 = if *value { 1 } else { 0 };
                    packed_uniform_bytes.extend_from_slice(&packed_flag.to_le_bytes());
                    packed_uniform_bytes.extend_from_slice(&[0u8; 12]); // Padding to 16 bytes
                } else {
                    packed_uniform_bytes.extend_from_slice(&[0u8; 16]);
                }
            }
        }
    }

    // Step 2: Upload the packed block in one write. A shader with no slots
    // leaves nothing to send.
    if !packed_uniform_bytes.is_empty() {
        queue.write_buffer(buffer, 0, &packed_uniform_bytes);
    }
}

/// Builds the parameters and textures bind groups a shader reads, for one
/// material or pass.
///
/// `owner_label` names the material or pass being built. It appears in the
/// debug labels and in every message about a slot, so a refused slot says what
/// asked for it.
///
/// `resolve_extra_texture` is the caller's own source of textures: a pass's
/// inputs, and nothing for a material. It runs before the textures the material
/// or pass names, so an input a pass names wins over a texture of the same slot
/// name.
///
/// # Errors
///
/// Returns [`RendererError`](crate::RendererError) when a slot cannot be
/// resolved: a source the caller could not produce, a depth slot with no
/// source, a texture that is not loaded, or a texture whose view shape does not
/// match its slot. It also returns it when a shader declares slots but has no
/// layout for them, and when wgpu refuses a group. Group creation runs under
/// [`capturing_validation`], so a refusal becomes an error instead of reaching
/// wgpu's uncaptured-error handler; a build that cannot capture errors gets
/// the handler's log instead.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shader_bind_groups<'views>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rendering_resource_storage: &RendererResourceStorage,
    owner_label: &str,
    shader: &RendererShader,
    parameters: &BTreeMap<String, MaterialParameter>,
    textures: &[(String, RendererTextureHandle)],
    resolve_extra_texture: impl Fn(
        &str,
        &ShaderTextureSlot,
    )
        -> Result<Option<(&'views wgpu::TextureView, &'views wgpu::Sampler)>>,
) -> Result<(Option<wgpu::BindGroup>, Option<wgpu::BindGroup>)> {
    // Step 1: The parameters group, when the shader declares parameter slots.
    let parameters_bind_group = if shader.parameter_slots.is_empty() {
        None
    } else {
        // The buffer is not kept anywhere else: a bind group holds what it
        // references, so the buffer lives as long as the group that reads it.
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("{owner_label}_parameters_buffer")),
            size: uniform_size(&shader.parameter_slots) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        write_parameters(queue, &buffer, &shader.parameter_slots, parameters);
        let layout = shader
            .parameters_bind_group_layout
            .as_ref()
            .context("a shader with parameter slots has a parameters layout")?;
        let bind_group = capturing_validation(device, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("{owner_label}_parameters_bind_group")),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0, // (set = 2, binding = 0)
                    resource: buffer.as_entire_binding(),
                }],
            })
        })
        .map_err(|detail| RendererError::Other { detail })?;
        Some(bind_group)
    };

    // Step 2: The textures group, when the shader declares texture slots.
    let textures_bind_group = if shader.texture_slots.is_empty() {
        None
    } else {
        let mut entries = Vec::new();
        for (slot_name, slot) in &shader.texture_slots {
            let resolved_texture = match resolve_extra_texture(slot_name, slot)? {
                Some(caller_texture) => Some(caller_texture),
                None => match textures.iter().find(|(name, _)| name == slot_name) {
                    Some((_, committed_handle)) => {
                        let texture = rendering_resource_storage
                            .textures
                            .get(*committed_handle)
                            .context(format!(
                                "{owner_label} binds `{slot_name}` to a texture that is not loaded"
                            ))?;
                        check_slot_shape(owner_label, slot_name, slot.kind, texture)?;
                        Some((&texture.texture_view, &texture.sampler))
                    }
                    // The shader declares the slot and the material or pass
                    // names nothing for it, so it gets the renderer's stand-in
                    // texture for that kind of map. A depth slot has no
                    // stand-in and fails the bind: only the pass that owns a
                    // depth buffer knows which one it means.
                    None => {
                        let default_texture_handle = rendering_resource_storage
                            .default_texture_for(slot.kind)
                            .ok_or_else(|| RendererError::Other {
                                detail: format!(
                                    "{owner_label} declares texture slot `{slot_name}` as depth, which has no stand-in"
                                ),
                            })?;
                        let texture = rendering_resource_storage
                            .textures
                            .get(default_texture_handle)
                            .context("the renderer's default textures exist")?;
                        Some((&texture.texture_view, &texture.sampler))
                    }
                },
            };
            let Some((view, sampler)) = resolved_texture else {
                continue;
            };
            entries.push(wgpu::BindGroupEntry {
                binding: slot.texture_binding,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: slot.sampler_binding,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        let layout = shader
            .textures_bind_group_layout
            .as_ref()
            .context("a shader with texture slots has a textures layout")?;
        let bind_group = capturing_validation(device, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("{owner_label}_textures_bind_group")),
                layout,
                entries: &entries,
            })
        })
        .map_err(|detail| RendererError::Other { detail })?;
        Some(bind_group)
    };

    Ok((parameters_bind_group, textures_bind_group))
}
