use crate::{
    config::*, 
    ecs::{ 
        DeferredOperationManagerPointer, 
        MeshRenderingComponent 
    }, 
    engine::Engine, 
    graphics::{ 
        RendererMaterialHandle, 
        RendererShaderHandle, 
        RendererTextureHandle, 
        RENDER_QUEUE_KEY_ORDER 
    }, 
    resources::{ 
        texture, 
        Resource, 
        ResourceStorage, 
        Texture, 
        TextureHandle, 
        TextureType,
        ResourceLoader
    }
};

use pill_core::{ 
    debug, 
    enum_variant_eq, 
    get_enum_variant_type_name, 
    get_type_name, 
    Color, 
    EngineError, 
    LogContext, 
    PillSlotMapKey, 
    PillStyle, 
    PillTypeMapKey 
};

use anyhow::{ Result, Context, Error };
use boolinator::*;
use std::{ 
    path::{ Path, PathBuf },
    collections::HashMap, 
    ops::{Range, RangeInclusive} 
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShaderType {
    Mesh,
    Fullscreen,
}

#[derive(Debug, Clone)]
pub enum ShaderValueParameterType {
    Float,
    Bool,
    Color,
    Vector2,
    // Extend by additional types if needed
}

#[derive(Debug, Clone)]
pub struct ShaderTextureParameterSlot {
    //pub name: String,
    pub texture_type: TextureType,

    // NOTE: Each texture in a shader requires two resources for sampling in GLSL/WGSL:
    pub texture_binding: u32, 
    pub sampler_binding: u32,
}

impl ShaderTextureParameterSlot {
    // NOTE: Textures have to have unique sampler bindings (since they are always passed in their own bind group)
    pub fn new(texture_type: TextureType, (texture_binding, sampler_binding): (u32, u32)) -> Self {
        Self {
           // name: name.to_string(),
            texture_type,
            texture_binding,
            sampler_binding
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShaderValueParameterSlot {
    //pub name: String,
    pub parameter_type: ShaderValueParameterType,
}

impl ShaderValueParameterSlot {
    // NOTE: Multiple parameters can share the same uniform binding (they will be passed together in the same bind group)
    pub fn new(parameter_type: ShaderValueParameterType) -> Self {
        Self {
            //name: name.to_string(),
            parameter_type,
        }
    }
}

// --- Shader ---

pill_core::define_new_pill_slotmap_key! { 
    pub struct ShaderHandle;
}

#[readonly::make]
pub struct Shader {
    #[readonly]
    pub name: String,
    #[readonly]
    pub shader_type: ShaderType,
    #[readonly]
    pub vertex_shader_resource_loader: ResourceLoader,
    #[readonly]
    pub fragment_shader_resource_loader: ResourceLoader,
    #[readonly]
    pub value_parameters_slots: HashMap<String, ShaderValueParameterSlot>, // TODO: We dont need ShaderParameterSlot, just the type is enough
    #[readonly]
    pub texture_parameters_slots: HashMap<String, ShaderTextureParameterSlot>,
    #[readonly]
    pub enable_engine_binding: bool,
    #[readonly]
    pub enable_camera_binding: bool,

    pub(crate) renderer_resource_handle: Option<RendererShaderHandle>,
    handle: Option<ShaderHandle>,
    deferred_operation_manager: Option<DeferredOperationManagerPointer>,
}

impl Shader {
    // NOTE: Builder pattern for Shader makes no sense, because all fields are required
    // TODO: Parse shader files and create slots automatically as well as pass_engine_parameters and pass_camera_parameters options

    pub fn new(
        name: &str,
        shader_type: ShaderType,
        vertex_shader_resource_loader: ResourceLoader, 
        fragment_shader_resource_loader: ResourceLoader,
        value_parameters_slots: HashMap<String, ShaderValueParameterSlot>,
        texture_parameters_slots: HashMap<String, ShaderTextureParameterSlot>,
        enable_engine_binding: bool, // If true, the engine uniform data will be accessible to the shader at (set = 0, binding = 0)
        enable_camera_binding: bool  // If true, the engine uniform data will be accessible to the shader at (set = 1, binding = 0)
    ) -> Self {
        Self {
            name: name.to_string(),
            shader_type,
            vertex_shader_resource_loader,
            fragment_shader_resource_loader,
            value_parameters_slots,
            texture_parameters_slots,
            enable_engine_binding,
            enable_camera_binding,
            renderer_resource_handle: None,
            handle: None,
            deferred_operation_manager: None,
        }
    }

    pub fn get_name(&self) -> String {
        self.name.clone()
    }
}

impl PillTypeMapKey for Shader {
    type Storage = ResourceStorage<Shader>; 
}

impl Resource for Shader {
    type Handle = ShaderHandle;

    fn get_name(&self) -> String {
        self.name.clone()
    }

    fn is_initialized(&self) -> bool {
        self.handle.is_some()
    }

    fn set_handle(&mut self, handle: Self::Handle) {
        self.handle = Some(handle);
    }

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        let error_message = format!("Initializing {} {} failed", "Resource".general_object_style(), get_type_name::<Self>().specific_object_style());

        // This resource is using DeferredOperationSystem so keep DeferredOperationManager
        //let deferred_operation_component = engine.get_global_component_mut::<DeferredOperationComponent>().expect("Critical: No DeferredOperationComponent");
        //self.deferred_operation_manager = Some(deferred_operation_component.borrow_deferred_operation_manager());

        // Read vertex shader data
        let vertex_shader_bytes_vec: Vec<u8>;
        let vertex_shader_bytes: &[u8] = match &self.vertex_shader_resource_loader {
            ResourceLoader::Path(path) => {
                // Check if path to asset is correct
                let resource_file_path = engine.game_resources_directory_path.join(path);
                pill_core::validate_asset_path(&resource_file_path, &["glsl"])?;

                // Load data
                vertex_shader_bytes_vec = std::fs::read(&resource_file_path)
                    .with_context(|| format!("Failed to read vertex shader file: {:?}", &resource_file_path))?;

                vertex_shader_bytes_vec.as_slice()
            },
            ResourceLoader::Bytes(bytes) => {
                bytes
            },
        };

        // Read fragment shader data
        let fragment_shader_bytes_vec: Vec<u8>;
        let fragment_shader_bytes: &[u8]  = match &self.fragment_shader_resource_loader {
            ResourceLoader::Path(path) => {
                // Check if path to asset is correct
                let resource_file_path = engine.game_resources_directory_path.join(path);
                pill_core::validate_asset_path(&resource_file_path, &["glsl"])?;

                // Load data
                fragment_shader_bytes_vec = std::fs::read(&resource_file_path)
                    .with_context(|| format!("Failed to read fragment shader file: {:?}", &resource_file_path))?;

                fragment_shader_bytes_vec.as_slice()
            },
            ResourceLoader::Bytes(bytes) => {
                bytes
            },
        };

        // TODO: Parse shader files and validate texture and parameter slots, or create them automatically here, so the user does not have to do it manually

        // Load data
        let renderer_resource_handle = engine.renderer.create_shader(
            &self.name, 
            self.shader_type.clone(),
            &vertex_shader_bytes, 
            &fragment_shader_bytes,
            &self.value_parameters_slots,
            &self.texture_parameters_slots,
            self.enable_engine_binding,
            self.enable_camera_binding
        ).context(error_message)?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    // fn pass_handle<H: PillSlotMapKey>(&mut self, self_handle: H) { 
    //     self.handle = Some(ShaderHandle::from(self_handle.data()));
    // }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_shader(v).unwrap();
        }

        // Find materials that use this shader and update them
        // for (scene_handle, scene) in engine.scene_manager.scenes.iter_mut() {
        //     let x = &engine.resource_manager;

        //     // for (entity_handle, mesh_rendering_component) in engine.iterate_one_component::<MeshRenderingComponent>()? {
        //     //     if let Some(material_handle) = mesh_rendering_component.material_handle {
        //     //         // If mesh rendering component has handle to this material 
        //     //         if material_handle.data() == self_handle.data() {
        //     //             mesh_rendering_component.set_material_handle(Option::<MaterialHandle>::None);
        //     //             mesh_rendering_component.update_render_queue_key(&engine.resource_manager).unwrap();
        //     //         }
        //     //     }
        //     // }
        // }

        Ok(())
    }
}