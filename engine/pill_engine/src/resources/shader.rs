use crate::{
    engine::Engine,
    ecs::DeferredUpdateManagerPointer,
    resources::{
        Resource,
        ResourceStorage,
        TextureType,
        ResourceLoader
    },
};

use crate::internal::RendererShaderHandle;

use pill_core::{
    get_type_name,
    PillSlotMapKey,
    PillStyle,
    PillTypeMapKey
};

use std::collections::HashMap;

use anyhow::{ Result, Context };

#[derive(Debug, Clone)]
pub enum ShaderParameterType {
    Scalar,
    Bool,
    Color,
    // Extend by additional types if needed
}

#[derive(Debug, Clone)]
pub struct ShaderTextureSlot {
    //pub name: String,
    pub texture_type: TextureType,

    // NOTE: Each texture in a shader requires two resources for sampling in GLSL/WGSL:
    pub texture_binding: u32,
    pub sampler_binding: u32,
}

impl ShaderTextureSlot {
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
pub struct ShaderParameterSlot {
    //pub name: String,
    pub parameter_type: ShaderParameterType,
}

impl ShaderParameterSlot {
    // NOTE: Multiple parameters can share the same uniform binding (they will be passed together in the same bind group)
    pub fn new(parameter_type: ShaderParameterType) -> Self {
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
    pub vertex_shader_resource_loader: ResourceLoader,
    #[readonly]
    pub fragment_shader_resource_loader: ResourceLoader,
    #[readonly]
    pub parameter_slots: HashMap<String, ShaderParameterSlot>, // TODO: We dont need ShaderParameterSlot, just the type is enough
    #[readonly]
    pub texture_slots: HashMap<String, ShaderTextureSlot>,
    #[readonly]
    pub enable_engine_binding: bool,
    #[readonly]
    pub enable_camera_binding: bool,

    pub(crate) renderer_resource_handle: Option<RendererShaderHandle>,
    handle: Option<ShaderHandle>,
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
}

impl Shader {
    // NOTE: Builder pattern for Shader makes no sense, because all fields are required
    // TODO: Parse shader files and create slots automatically as well as pass_engine_parameters and pass_camera_parameters options

    pub fn new(
        name: &str,
        vertex_shader_resource_loader: ResourceLoader,
        fragment_shader_resource_loader: ResourceLoader,
        parameter_slots: HashMap<String, ShaderParameterSlot>,
        texture_slots: HashMap<String, ShaderTextureSlot>,
        enable_engine_binding: bool, // If true, the engine uniform data will be accessible to the shader at (set = 0, binding = 0)
        enable_camera_binding: bool  // If true, the engine uniform data will be accessible to the shader at (set = 1, binding = 0)
    ) -> Self {
        Self {
            name: name.to_string(),
            vertex_shader_resource_loader,
            fragment_shader_resource_loader,
            parameter_slots,
            texture_slots,
            enable_engine_binding,
            enable_camera_binding,
            renderer_resource_handle: None,
            handle: None,
            deferred_update_manager: None,
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

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        let error_message = format!("Initializing {} {} failed", "Resource".general_object_style(), get_type_name::<Self>().specific_object_style());

        // This resource is using DeferredUpdateSystem so keep DeferredUpdateManager
        //let deferred_update_component = engine.get_global_component_mut::<DeferredUpdateComponent>().expect("Critical: No DeferredUpdateComponent");
        //self.deferred_update_manager = Some(deferred_update_component.borrow_deferred_update_manager());

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
            vertex_shader_bytes,
            fragment_shader_bytes,
            &self.texture_slots,
            &self.parameter_slots,
            self.enable_engine_binding,
            self.enable_camera_binding
        ).context(error_message)?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn pass_handle<H: PillSlotMapKey>(&mut self, self_handle: H) {
        self.handle = Some(ShaderHandle::from(self_handle.data()));
    }

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
