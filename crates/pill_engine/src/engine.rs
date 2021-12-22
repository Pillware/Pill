use std::{any::type_name, collections::VecDeque};
use anyhow::{Context, Result, Error};
use boolinator::Boolinator;
use log::{debug, info, error};
use winit::{ event::*, dpi::PhysicalPosition,};

use pill_core::{EngineError, get_type_name, PillSlotMapKey, PillStyle};
use crate::{ 
    resources::*,
    ecs::*,
    graphics::*,
    input::*, 
    config::*,
};

// ---------------------------------------------------------------------

pub type Game = Box<dyn PillGame>;
pub trait PillGame { 
    fn start(&self, engine: &mut Engine);
}
pub struct Engine { 
    pub(crate) game: Option<Game>,
    pub(crate) renderer: Renderer,
    pub(crate) scene_manager: SceneManager, // [TODO: What will happen with objects registered in renderer if we change the scene for which they were registered?]
    pub(crate) system_manager: SystemManager,
    pub(crate) resource_manager: ResourceManager,
    pub(crate) input_queue: VecDeque<InputEvent>,
    pub(crate) render_queue: Vec<RenderQueueItem>,
    pub(crate) window_size: winit::dpi::PhysicalSize<u32>,
}

// ---- INTERNAL -----------------------------------------------------------------

impl Engine {

     fn create_default_resources(&mut self) {
        self.register_resource_type::<Texture>().unwrap();
        self.register_resource_type::<Mesh>().unwrap();
        self.register_resource_type::<Material>().unwrap();

        // Create default resources

        // Load master shader data to executable
        let master_vertex_shader_bytes = include_bytes!("../res/shaders/built/master.vert.spv");
        let master_fragment_shader_bytes = include_bytes!("../res/shaders/built/master.frag.spv");
        self.renderer.set_master_pipeline(master_vertex_shader_bytes, master_fragment_shader_bytes).unwrap();

        // Load default resource data to executable
        let default_color_texture_bytes = Box::new(*include_bytes!("../res/textures/default_color.png"));
        let default_normal_texture_bytes = Box::new(*include_bytes!("../res/textures/default_normal.png"));

        // Create default textures
        let mut default_color_texture = Texture::new(DEFAULT_COLOR_TEXTURE_NAME, TextureType::Color, ResourceLoadType::Bytes(default_color_texture_bytes));
        default_color_texture.initialize(self);
        self.resource_manager.add_resource(default_color_texture).unwrap();

        let mut default_normal_texture = Texture::new(DEFAULT_NORMAL_TEXTURE_NAME, TextureType::Normal, ResourceLoadType::Bytes(default_normal_texture_bytes));
        default_normal_texture.initialize(self);
        self.resource_manager.add_resource(default_normal_texture).unwrap();
       
        // Create default material
        let mut default_material = Material::new(DEFAULT_MATERIAL_NAME);
        default_material.initialize(self);
        self.resource_manager.add_resource(default_material).unwrap();
    }
}

// ---- INTERNAL API -----------------------------------------------------------------

impl Engine {

    pub fn new(game: Box<dyn PillGame>, renderer: Box<dyn PillRenderer>) -> Self {
        let scene_manager = SceneManager::new();
        let resource_manager = ResourceManager::new();
        let system_manager = SystemManager::new();

        let mut engine = Self { 
            game: Some(game),
            renderer,
            scene_manager,
            system_manager,
            resource_manager,
            input_queue: VecDeque::new(),
            render_queue: Vec::<RenderQueueItem>::with_capacity(1000),
            window_size: winit::dpi::PhysicalSize::<u32>::default(),
        };

        engine.create_default_resources();

        engine
    }

    pub fn initialize(&mut self, window_size: winit::dpi::PhysicalSize<u32>) {
        info!("Initializing {}", "Engine".mobj_style());

        // Set window size
        self.window_size = window_size;

        // Add built-in systems
        self.system_manager.add_system("RenderingSystem", rendering_system, UpdatePhase::PostGame).unwrap();

        // Initialize game
        let game = self.game.take().unwrap(); 
        game.start(self); 
        self.game = Some(game);
    }

    pub fn update(&mut self, dt: std::time::Duration) {
        use pill_core::{PillStyle, get_value_type_name};

        // Run systems
        let update_phase_count = self.system_manager.update_phases.len();
        for i in (0..update_phase_count).rev() {
            let systems_count = self.system_manager.update_phases[i].len();
            for j in (0..systems_count).rev() {
                let system = &self.system_manager.update_phases[i][j];
                if !system.enabled { continue; }
                let system_name = system.name.to_string();
                (system.system_function)(self).context(
                    format!("{}", EngineError::SystemUpdateFailed(system_name, get_value_type_name(self.system_manager.update_phases.get_index(i).unwrap().0)))
                ).unwrap();
            }
        }
 
        let frame_time = dt.as_secs_f32() * 1000.0;
        let fps =  1000.0 / frame_time;
        info!("Frame finished (Time: {:.3}ms, FPS {:.0})", frame_time, fps);
    }

    pub fn shutdown(&mut self) {
        info!("Shutting down {}", "Engine".mobj_style());
    }

    pub fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        info!("{} resized to {}x{}", "Window".mobj_style(), new_window_size.width, new_window_size.height);
        self.window_size = new_window_size;
        self.renderer.resize(new_window_size);
    }

    pub fn pass_keyboard_key_input(&mut self, keyboard_input: &KeyboardInput) {
        let key: VirtualKeyCode = keyboard_input.virtual_keycode.unwrap();
        let state: ElementState = keyboard_input.state;

        let input_event = InputEvent::KeyboardKey { key: key, state: state };
        self.input_queue.push_back(input_event);
        info!("Got new keyboard key input: {:?} {:?}", key, state);
    }

    pub fn pass_mouse_key_input(&mut self, key: &MouseButton, state: &ElementState) {
        let input_event = InputEvent::MouseKey { key: *key, state: *state }; // Here using * we actually are copying the value of key because MouseButton implements a Copy trait
        self.input_queue.push_back(input_event);
        info!("Got new mouse key input");
    }

    pub fn pass_mouse_wheel_input(&mut self, delta: &MouseScrollDelta) {
        let input_event = InputEvent::MouseWheel { delta: *delta };
        self.input_queue.push_back(input_event);
        info!("Got new mouse wheel input");
    }

    pub fn pass_mouse_motion_input(&mut self, position: &PhysicalPosition<f64>) {
        let input_event = InputEvent::MouseMotion { position: *position };
        self.input_queue.push_back(input_event);
        info!("Got new mouse motion input");
    }

    pub fn get_input_queue(&self) -> &VecDeque<InputEvent> {
        &self.input_queue
    }
}

// --- GAME API ------------------------------------------------------------------

impl Engine { 

    // --- ECS API ---

    pub fn register_component<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene_handle: SceneHandle) -> Result<()> {
        self.scene_manager.register_component::<T>(scene_handle).context(format!("Registering {} failed", "Component".gobj_style()))
    }

    pub fn add_system(&mut self, name: &str, system_function: fn(engine: &mut Engine) -> Result<()>) -> Result<()> {
        self.system_manager.add_system(name, system_function, UpdatePhase::Game).context(format!("Adding {} failed", "System".gobj_style()))
    }

    // [TODO] Implement remove_system

    pub fn create_entity(&mut self, scene_handle: SceneHandle) -> Result<EntityHandle> {
        self.scene_manager.create_entity(scene_handle).context(format!("Creating {} failed", "Entity".gobj_style()))
    }
    
    pub fn add_component_to_entity<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle, component: T) -> Result<()> {
        debug!("Adding {} {} to {} {} in {} {}", "Component".gobj_style(), get_type_name::<T>().sobj_style(), "Entity".gobj_style(), entity_handle.index, "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name);
        self.scene_manager.add_component_to_entity::<T>(scene_handle, entity_handle, component).context(format!("Adding {} to {} failed", "Component".gobj_style(), "Entity".gobj_style()))
    }
    
    // [TODO] Implement remove_component_from_entity

    // [TODO] Implement get_component_from_entity


    // --- Scene API ---

    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        info!("Creating scene: {}", name);
        self.scene_manager.create_scene(name).context(format!("Creating new {} failed", "Scene".gobj_style()))
    }

    pub fn get_scene_handle(&self, name: &str) -> Result<SceneHandle> {
        self.scene_manager.get_scene_handle(name).context(format!("Getting {} failed", "SceneHandle".sobj_style()))
    }

    pub fn get_scene(&self, scene_handle: SceneHandle) -> Result<&Scene> {
        self.scene_manager.get_scene(scene_handle).context(format!("Getting {} failed", "Scene".gobj_style()))
    }

    pub fn get_scene_mut(&mut self, scene_handle: SceneHandle) -> Result<&mut Scene> {
        self.scene_manager.get_scene_mut(scene_handle).context(format!("Getting {} as mutable failed", "Scene".gobj_style()))
    }


    pub fn set_active_scene(&mut self, scene_handle: SceneHandle) -> Result<()> {
        self.scene_manager.set_active_scene(scene_handle).context(format!("Setting active {} failed", "Scene".gobj_style()))
    }

    pub fn get_active_scene_handle(&mut self) -> Result<SceneHandle> {
        self.scene_manager.get_active_scene_handle().context(format!("Getting {} of active {} failed", "SceneHandle".sobj_style(), "Scene".gobj_style()))
    }

    fn get_active_scene(&mut self) -> Result<&Scene> {
        self.scene_manager.get_active_scene().context(format!("Getting active {} failed", "Scene".gobj_style()))
    }

    fn get_active_scene_mut(&mut self) -> Result<&mut Scene> {
        self.scene_manager.get_active_scene_mut().context(format!("Getting active {} as mutable failed", "Scene".gobj_style()))
    }


    // --- Resource API ---

    pub fn register_resource_type<T: Resource<Value = Option<ResourceStorage::<T>>>>(&mut self) -> Result<()> {
        self.resource_manager.register_resource_type::<T>()
    }

    pub fn add_resource<T>(&mut self, mut resource: T) -> Result<T::Handle> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Check if resource has proper name
        let resource_name = resource.get_name();
        resource_name.starts_with(DEFAULT_RESOURCE_PREFIX).eq(&false).ok_or(Error::new(EngineError::WrongResourceName(resource_name.clone())))?;

        // Initialize resource
        resource.initialize(self);
        
        // Add resource
        let resource_handle = self.resource_manager.add_resource(resource)?;

        // [TODO]: In resource initialization it may happen that renderer resource will be created, and after that add_resource may fail, so resource will not be added to the engine 
        // but rendering resource will be left there without any user. This should be handled! 
        // (We can first check if there will be a place in resource_manager and then start procedure, or if add_resource will fail then deinitialize resource what will remove renderer resource)

        Ok(resource_handle)
    }

    pub fn get_resource<'a, T>(&'a self, resource_handle: &'a T::Handle) -> Result<&'a T> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        Ok(self.resource_manager.get_resource::<T>(resource_handle)?)
    }

    pub fn get_resource_by_name<T>(&self, name: &str) -> Result<&T> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        Ok(self.resource_manager.get_resource_by_name::<T>(name)?)
    }

    pub fn get_resource_mut<'a, T>(&'a mut self, resource_handle: &'a T::Handle) -> Result<&'a mut T> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        Ok(self.resource_manager.get_resource_mut::<T>(resource_handle)?)
    }

    pub fn get_resource_by_name_mut<T>(&mut self, name: &str) -> Result<&mut T> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        Ok(self.resource_manager.get_resource_by_name_mut::<T>(name)?)
    }

    pub fn remove_resource<T>(&mut self, resource_handle: &T::Handle) -> Result<()> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Check if resource is not default
        let resource_name = self.resource_manager.get_resource::<T>(resource_handle)?.get_name();
        resource_name.starts_with(DEFAULT_RESOURCE_PREFIX).eq(&false).ok_or(Error::new(EngineError::RemoveDefaultResource(resource_name.clone())))?;

        // Remove resource
        let mut remove_result = self.resource_manager.remove_resource::<T>(resource_handle)?;
        remove_result.1.destroy(self, *resource_handle);
        Ok(())
    }

    pub fn remove_resource_by_name<T>(&mut self, name: &str) -> Result<()> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Check if resource exists
        self.resource_manager.get_resource_by_name::<T>(name)?;

        // Check if resource is not default
        name.starts_with(DEFAULT_RESOURCE_PREFIX).eq(&false).ok_or(Error::new(EngineError::RemoveDefaultResource(name.to_string())))?;

        // Remove resource
        let mut remove_result = self.resource_manager.remove_resource_by_name::<T>(name)?;
        remove_result.1.destroy(self, remove_result.0);
        Ok(())
    }
}