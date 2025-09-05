use std::sync::Arc;
use pill_core::{debug, LogContext, PillStyle, Timer};
use pill_engine::{game::ShaderType, internal::{PostprocessingEffectsRendererData, PostprocessingVolumeRendererData, RendererShaderHandle}};
use wgpu::wgc::device;
use winit::event::WindowEvent;
use winit::window::Window;
use anyhow::{Error, Result};

use crate::{config::{CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX}, resources::{RendererCamera, RendererMaterial, RendererResourceStorage, RendererShader}};

pub struct PostprocessingDrawer {
}

impl PostprocessingDrawer {
    pub fn new(
    ) -> PostprocessingDrawer {
        PostprocessingDrawer {
        }
    }

    pub fn change_shader(
        &mut self, 
        renderer_resource_storage: &RendererResourceStorage, 
        shader_handle: RendererShaderHandle,
        render_pass: &mut wgpu::RenderPass,
        camera: &RendererCamera,
    ) {
        let shader: &RendererShader = renderer_resource_storage.shaders.get(shader_handle).unwrap();

        debug!(LogContext::Frame => "Changing shader to: {}", shader.name.name_style());

        render_pass.set_pipeline(&shader.render_pipeline);

        if shader.pass_engine_parameters {
            render_pass.set_bind_group(ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, &renderer_resource_storage.engine_parameters.bind_group, &[]);
            debug!(LogContext::Frame => "Engine parameters bound");
        }   

        if shader.pass_camera_parameters {
            render_pass.set_bind_group(CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, &camera.bind_group, &[]);
            debug!(LogContext::Frame => "Camera parameters bound");
        }

        debug!(LogContext::Frame => "Renderer pipeline shader changed");
    }

    pub fn record_draw_commands(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        postprocessing_volumes_renderer_data: &Vec<PostprocessingVolumeRendererData>,
        renderer_camera: &RendererCamera,
        renderer_resource_storage: &mut RendererResourceStorage, 
        timer: &mut Timer,
    ) -> Result<()> {
        timer.record("Prepare window and input");

        // One render pass per each postprocessing volumes

        // Render pass for each postprocessing effect
        for postprocessing_volume_renderer_data in postprocessing_volumes_renderer_data {

            // Update all parameters for all materials used in postprocessing volume
            // NOTE: This can't be done during render pass recording, since these are two different processes
            for effect_data in &postprocessing_volume_renderer_data.effect_data {


                let material = renderer_resource_storage.materials.get(effect_data.material_handle)
                    .ok_or(Error::msg(format!("Material handle {:?} not found in renderer resource storage", effect_data.material_handle)))?;
                let shader: &RendererShader = renderer_resource_storage.shaders.get(material.shader_handle)
                    .ok_or(Error::msg(format!("Shader handle {:?} not found in renderer resource storage", material.shader_handle)))?;
                if shader.shader_type != ShaderType::Fullscreen {
                    return Err(Error::msg(format!("Postprocessing effect shader {} is not defined as a fullscreen shader", shader.name.name_style())));
                }
                RendererMaterial::update_parameters(
                    device,
                    queue,
                    effect_data.material_handle,
                    renderer_resource_storage,
                    &effect_data.material_parameters
                )?;
            }

            // Render
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("postprocess_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Draw each effect in the postprocessing volume
            for effect in &postprocessing_volume_renderer_data.effect_data {
                let material = renderer_resource_storage.materials.get(effect.material_handle)
                    .ok_or(Error::msg(format!("Material handle {:?} not found in renderer resource storage", effect.material_handle)))?;
                let shader: &RendererShader = renderer_resource_storage.shaders.get(material.shader_handle)
                    .ok_or(Error::msg(format!("Shader handle {:?} not found in renderer resource storage", material.shader_handle)))?;
                self.change_shader(&renderer_resource_storage, material.shader_handle, &mut render_pass, &renderer_camera);

                render_pass.set_pipeline(&shader.render_pipeline);
                render_pass.set_bind_group(0, self.scene_texture_bind_group.as_ref().unwrap(), &[]);
                render_pass.set_bind_group(1, &self.params_bind_group, &[]);
                render_pass.draw(0..3, 0..1); // Draw fullscreen triangle
            }
        }

        Ok(())
    }
}

