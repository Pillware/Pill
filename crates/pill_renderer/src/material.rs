use std::path::Path;
use std::path::PathBuf;
use anyhow::{Result, Context, Error};
use wgpu::BindGroup;

use pill_core::na::SliceRange;
use pill_engine::internal::ResourceHandle;

use crate::pipeline::RendererPipelineHandle;

#[derive(Clone, Copy)]
pub struct RendererMaterialHandle {
    pub index: u32,
}

impl ResourceHandle for RendererMaterialHandle
{
    fn get_index(&self) -> u32 {
        self.index
    }
}

pub struct RendererMaterial {
    pub pipeline: RendererPipelineHandle,
    pub bind_group: BindGroup,
}

impl RendererMaterial {
    pub fn new(name: &str) -> Result<Self> { 

        let renderer_material = Self { 

        };

        Ok(renderer_material)
    }
}
