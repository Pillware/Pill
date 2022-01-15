/// Necessary imports
use crate::{ 
    resources::*,
    ecs::*,
    graphics::*,
    config::*,
};

use pill_core::{ 
    EngineError, 
    PillSlotMapKey, 
    PillStyle, 
    PillTypeMap,
    get_type_name, 
    get_value_type_name, 
    get_enum_variant_type_name, get_game_error_message, 
};

use std::{ any::type_name, any::Any, any::TypeId, collections::VecDeque, cell::RefCell };
use anyhow::{Context, Result, Error};
use boolinator::Boolinator;
use log::{debug, info, error};
use winit::{ event::*, dpi::PhysicalPosition,};

// -------------------------------------------------------------------------------

/// Simplified type of the Box<dyn PillGame> for the PillGame
pub type Game = Box<dyn PillGame>;

/// Simplified type of the VirtualKeyCode
/// 
/// Used for easy access to InputComponent information about the pressed keys
pub type Key = VirtualKeyCode;

/// Simplified type of the MouseButton
/// 
/// Used for easy access to InputComponent information about the pressed mouse buttons
pub type Mouse = MouseButton;

/// Trait used for PillGame functionality definition
pub trait PillGame { 
    fn start(&self, engine: &mut Engine) -> Result<()>;
}

/// Engine structure
pub struct Engine { 
    pub(crate) config: config::Config,
    pub(crate) game: Option<Game>,
    pub(crate) renderer: Renderer,
    pub(crate) scene_manager: SceneManager,
    pub(crate) system_manager: SystemManager,
    pub(crate) resource_manager: ResourceManager,
    pub(crate) global_components: PillTypeMap,
    pub(crate) input_queue: VecDeque<InputEvent>,
    pub(crate) render_queue: Vec<RenderQueueItem>,
    pub(crate) window_size: winit::dpi::PhysicalSize<u32>,
    pub(crate) frame_delta_time: f32,
}

// ---- INTERNAL -----------------------------------------------------------------

impl Engine {
    fn create_default_resources(&mut self) -> Result<()> {

        let max_texture_count = self.config.get_int("MAX_TEXTURE_COUNT").unwrap_or(MAX_TEXTURE_COUNT as i64) as usize;
        let max_mesh_count = self.config.get_int("MAX_MESH_COUNT").unwrap_or(MAX_MESH_COUNT as i64) as usize;
        let max_material_count = self.config.get_int("MAX_MATERIAL_COUNT").unwrap_or(MAX_MATERIAL_COUNT as i64) as usize;
        let max_sound_count = self.config.get_int("MAX_SOUND_COUNT").unwrap_or(MAX_SOUND_COUNT as i64) as usize;

        self.register_resource_type::<Texture>(max_texture_count)?;
        self.register_resource_type::<Mesh>(max_mesh_count)?;
        self.register_resource_type::<Material>(max_material_count)?;
        self.register_resource_type::<Sound>(max_sound_count)?;

        // - Create default resources

        // Load master shader data to executable
        let master_vertex_shader_bytes = include_bytes!("../res/shaders/built/master.vert.spv");
        let master_fragment_shader_bytes = include_bytes!("../res/shaders/built/master.frag.spv");
        self.renderer.set_master_pipeline(master_vertex_shader_bytes, master_fragment_shader_bytes)?;

        // Load default resource data to executable
        let default_color_texture_bytes = Box::new(*include_bytes!("../res/textures/default_color.png"));
        let default_normal_texture_bytes = Box::new(*include_bytes!("../res/textures/default_normal.png"));

        // Create default textures
        let mut default_color_texture = Texture::new(DEFAULT_COLOR_TEXTURE_NAME, TextureType::Color, ResourceLoadType::Bytes(default_color_texture_bytes));
        default_color_texture.initialize(self)?;
        self.resource_manager.add_resource(default_color_texture)?;

        let mut default_normal_texture = Texture::new(DEFAULT_NORMAL_TEXTURE_NAME, TextureType::Normal, ResourceLoadType::Bytes(default_normal_texture_bytes));
        default_normal_texture.initialize(self)?;
        self.resource_manager.add_resource(default_normal_texture)?;
        
        // Create default material
        let mut default_material = Material::new(DEFAULT_MATERIAL_NAME);
        default_material.initialize(self)?;
        self.resource_manager.add_resource(default_material)?;
        
        Ok(())
    }
}

// ---- INTERNAL API -----------------------------------------------------------------

/// Implementation of the Engine Internal API 
/// 
/// Created for pill_standalone
impl Engine {

    /// Associated function creating new engine
    /// 
    /// Takes fame, renderer, and config as the parameters
    pub fn new(game: Box<dyn PillGame>, renderer: Box<dyn PillRenderer>, config: config::Config) -> Self {
        let max_entity_count = config.get_int("MAX_ENTITY_COUNT").unwrap_or(MAX_ENTITY_COUNT as i64) as usize;

        Self { 
            config,
            game: Some(game),
            renderer,
            scene_manager: SceneManager::new(max_entity_count),
            system_manager: SystemManager::new(),
            resource_manager: ResourceManager::new(),
            global_components: PillTypeMap::new(),
            input_queue: VecDeque::new(),
            render_queue: Vec::<RenderQueueItem>::with_capacity(max_entity_count),
            window_size: winit::dpi::PhysicalSize::<u32>::default(),
            frame_delta_time: 0.0,
        }
    }

    /// Method used for engine initialization
    pub fn initialize(&mut self, window_size: winit::dpi::PhysicalSize<u32>) -> Result<()> {
        info!("Initializing {}", "Engine".mobj_style());

        // Set window size
        self.window_size = window_size;

        // Register global components
        self.add_global_component(InputComponent::new())?;
        self.add_global_component(TimeComponent::new())?;
        self.add_global_component(DeferredUpdateComponent::new())?;

        let max_ambient_sink_count = self.config.get_int("MAX_CONCURRENT_2D_SOUND_COUNT").unwrap_or(MAX_CONCURRENT_2D_SOUND_COUNT as i64) as usize;
        let max_spatial_sink_count = self.config.get_int("MAX_CONCURRENT_3D_SOUND_COUNT").unwrap_or(MAX_CONCURRENT_3D_SOUND_COUNT as i64) as usize;
        self.add_global_component(AudioManagerComponent::new(max_ambient_sink_count, max_spatial_sink_count))?;

        // Add built-in systems
        self.system_manager.add_system("InputSystem", input_system, UpdatePhase::PreGame)?;
        self.system_manager.add_system("TimeSystem", time_system, UpdatePhase::PostGame)?;
        self.system_manager.add_system("RenderingSystem", rendering_system, UpdatePhase::PostGame)?;
        self.system_manager.add_system("AudioSystem", audio_system, UpdatePhase::PostGame)?;
        self.system_manager.add_system("DeferredUpdateSystem", deferred_update_system, UpdatePhase::PostGame)?;

        // Create default resources
        self.create_default_resources().context("Failed to create default resources")?;

        // Initialize game
        let game = self.game.take().ok_or(EngineError::Other("Cannot get game".to_string()))?;
        let stop_on_game_errors = self.config.get_bool("PANIC_ON_GAME_ERRORS").unwrap_or(PANIC_ON_GAME_ERRORS);
        let result = game.start(self);
        match stop_on_game_errors {
            true => result.context(format!("{} error", "Game".mobj_style()))?,
            false => { 
                if let Some(message) = get_game_error_message(result) {
                    error!("{}", message);
                } 
            },
        }
        self.game = Some(game);

        Ok(())
    }


    /// Method used for running game systems
    pub fn update(&mut self, delta_time: std::time::Duration) {
        let stop_on_game_errors = self.config.get_bool("PANIC_ON_GAME_ERRORS").unwrap_or(PANIC_ON_GAME_ERRORS);
        
        // Run systems
        let update_phase_count = self.system_manager.update_phases.len();
        for i in (0..update_phase_count).rev() {
            let systems_count = self.system_manager.update_phases[i].len();
            for j in (0..systems_count).rev() {
                let system = &self.system_manager.update_phases[i][j];
                if !system.enabled { continue; }
                let system_name = system.name.to_string();

                if system.update_phase.clone() == UpdatePhase::Game && stop_on_game_errors {
                    let mut result = (system.system_function)(self);
                    result = result.context(EngineError::SystemUpdateFailed(system_name, get_enum_variant_type_name(self.system_manager.update_phases.get_index(i).unwrap().0)));
                    if let Some(message) = get_game_error_message(result) {
                        error!("{}", message);
                    }
                }
                else {
                    let result = (system.system_function)(self);
                    result.context(EngineError::SystemUpdateFailed(system_name, get_enum_variant_type_name(self.system_manager.update_phases.get_index(i).unwrap().0))).unwrap();
                }
            }
        }
 
        // Update FPS counter
        let new_frame_time = delta_time.as_secs_f32() * 1000.0;
        let fps =  1000.0 / new_frame_time;
        self.frame_delta_time = new_frame_time;
        debug!("Frame finished (Time: {:.3}ms, FPS {:.0})", new_frame_time, fps);
    }

    /// Function used for shutdown information printing
    /// 
    /// Used by pill_standalone
    pub fn shutdown(&mut self) {
        info!("Shutting down {}", "Engine".mobj_style());
    }

    /// Function used for window resizing option
    /// 
    /// Used by pill_standalone
    pub fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        debug!("{} resized to {}x{}", "Window".mobj_style(), new_window_size.width, new_window_size.height);
        self.window_size = new_window_size;
        self.renderer.resize(new_window_size);
    }

    /// Function used for keyboard input passing
    /// 
    /// Used by pill_standalone
    pub fn pass_keyboard_key_input(&mut self, keyboard_input: &KeyboardInput) {
        if let Some(key) = keyboard_input.virtual_keycode {
            let state: ElementState = keyboard_input.state;
            let input_event = InputEvent::KeyboardKey { key: key, state: state };
            self.input_queue.push_back(input_event);
            debug!("Got new keyboard key input: {:?} {:?}", key, state);
        }
    }

    /// Function used for mouse key input passing
    /// 
    /// Used by pill_standalone
    pub fn pass_mouse_key_input(&mut self, key: &MouseButton, state: &ElementState) {
        let input_event = InputEvent::MouseButton { key: *key, state: *state }; // Here using * we actually are copying the value of key because MouseButton implements a Copy trait
        self.input_queue.push_back(input_event);
        debug!("Got new mouse key input");
    }

    /// Function used for mouse wheel input passing
    /// 
    /// Used by pill_standalone
    pub fn pass_mouse_wheel_input(&mut self, delta: &MouseScrollDelta) {
        let input_event = InputEvent::MouseWheel { delta: *delta };
        self.input_queue.push_back(input_event);
        debug!("Got new mouse wheel input");
    }

    /// Function used for mouse motion input passing
    /// 
    /// Used by pill_standalone
    pub fn pass_mouse_motion_input(&mut self, delta: &(f64, f64)) {
        let input_event = InputEvent::MouseMotion { delta: *delta };
        self.input_queue.push_back(input_event);
        debug!("Got new mouse motion input");
    }

    pub fn pass_mouse_position_input(&mut self, position: &PhysicalPosition<f64>) {
        let input_event = InputEvent::MousePosition { position: *position };
        self.input_queue.push_back(input_event);
        debug!("Got new mouse position input");
    }

    pub fn get_input_queue(&self) -> &VecDeque<InputEvent> {
        &self.input_queue
    }
}

// --- API ------------------------------------------------------------------

/// Implementation of the engine's API methods 
/// 
/// Used by Game Developers
impl Engine { 

    // --- System API ---

    /// Method used for custom game systems implementation
    ///
    /// Systems need to be first creating with different functions, and then added by usage of this method
    pub fn add_system(&mut self, name: &str, system_function: fn(engine: &mut Engine) -> Result<()>) -> Result<()> {
        debug!("Adding {} {} to {} {}", "System".gobj_style(), name.name_style(), "UpdatePhase".sobj_style(), "Game".name_style());

        self.system_manager.add_system(name, system_function, UpdatePhase::Game).context(format!("Adding {} failed", "System".gobj_style()))
    }

    /// Function used for custom game system removal
    pub fn remove_system(&mut self, name: &str) -> Result<()> {
        debug!("Removing {} {} from {} {}", "System".gobj_style(), name.name_style(), "UpdatePhase".sobj_style(), "Game".name_style());

        self.system_manager.remove_system(name, UpdatePhase::Game).context(format!("Removing {} failed", "System".gobj_style()))
    }

    /// Function used for custom system toggling
    pub fn toggle_system(&mut self, name: &str, enabled: bool) -> Result<()> {
        debug!("Toggling {} {} from {} {} to {} state", "System".gobj_style(), name.name_style(), "UpdatePhase".sobj_style(), "Game".name_style(), if enabled { "Enabled" } else { "Disabled" });

        self.system_manager.toggle_system(name, UpdatePhase::Game, enabled).context(format!("Toggling {} failed", "System".gobj_style()))
    }
    
    // --- Entity API ---

    /// Method used for entity creation
    /// 
    /// Returns EntityBuilder, which gives the ability to chain the methods for components addition
    pub fn build_entity(&mut self, scene_handle: SceneHandle) -> EntityBuilder {
        let entity_handle = self.create_entity(scene_handle).unwrap();
        EntityBuilder {
            engine: self,
            entity_handle,
            scene_handle,
        }
    }

    /// Method used for entity creation
    /// 
    /// Return EntityHandle in case of success
    pub fn create_entity(&mut self, scene_handle: SceneHandle) -> Result<EntityHandle> {
        debug!("Creating {} in {} {}", "Entity".gobj_style(), "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        self.scene_manager.create_entity(scene_handle).context(format!("Creating {} failed", "Entity".gobj_style()))
    }

    /// Method used for entity removal
    pub fn remove_entity(&mut self, entity_handle: EntityHandle, scene_handle: SceneHandle) -> Result<()> {
        debug!("Removing {} from {} {}", "Entity".gobj_style(), "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        self.scene_manager.remove_entity(entity_handle, scene_handle).context(format!("Creating {} failed", "Entity".gobj_style()))
    }

    // --- Component API ---

    /// Method used for fetching component belonging to entity
    /// 
    /// Returns None if the component does not exist, or RefCell to the Option of the component
    pub fn get_component_by_entity<T>(&self, entity_handle: EntityHandle, scene_handle: SceneHandle) -> Result<Option<&RefCell<Option<T>>>>
        where T: Component<Storage = ComponentStorage<T>>
    {   
        debug!("Fetching component {} from {} {} in {} {}", get_type_name::<T>().sobj_style(), "Entity".gobj_style(), entity_handle.data().index, "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        let component = self.scene_manager.fetch_component_by_entity::<T>(entity_handle, scene_handle)?;

        Ok(component)
    }

    /// Method used for new component type registering
    pub fn register_component<T>(&mut self, scene_handle: SceneHandle) -> Result<()> 
        where T: Component<Storage = ComponentStorage::<T>>
    {
        debug!("Registering {} {} in {} {}", "Component".gobj_style(), get_type_name::<T>().sobj_style(), "Scene".sobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        self.scene_manager.register_component::<T>(scene_handle).context(format!("Registering {} failed", "Component".gobj_style()))
    }

    /// Method used for component to entity addition
    pub fn add_component_to_entity<T>(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle, mut component: T) -> Result<()> 
        where T : Component<Storage = ComponentStorage::<T>>
    {
        debug!("Adding {} {} to {} {} in {} {}", "Component".gobj_style(), get_type_name::<T>().sobj_style(), "Entity".gobj_style(), entity_handle.data().index, "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());
        
        // Initialize component
        component.initialize(self).context(format!("Adding {} {} failed", "Component".gobj_style(), get_type_name::<T>().sobj_style()))?;
        
        // Add component
        self.scene_manager.add_component_to_entity::<T>(scene_handle, entity_handle, component).context(format!("Adding {} to {} failed", "Component".gobj_style(), "Entity".gobj_style()))
    }

    /// Method used for component from entity removal
    pub fn remove_component_from_entity<T>(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle) -> Result<()> 
        where T : Component<Storage = ComponentStorage::<T>>
    {
        debug!("Removing {} {} from {} {} in {} {}", "Component".gobj_style(), get_type_name::<T>().sobj_style(), "Entity".gobj_style(), entity_handle.data().index, "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());
        
        let mut component = self.scene_manager.remove_component_from_entity::<T>(scene_handle, entity_handle).context("Removing component from entity failed").unwrap();

        // Destroy component
        component.destroy(self, entity_handle, scene_handle)?;

        Ok(())
    }

    // --- Global Component API ---

    /// Method used for addition of a custom global component 
    pub fn add_global_component<T>(&mut self, mut component: T) -> Result<()> 
        where T: GlobalComponent<Storage = GlobalComponentStorage::<T>>
    {
        // Check if component of this type is not already added
        self.global_components.contains_key::<T>().eq(&false).ok_or(Error::new(EngineError::GlobalComponentAlreadyExists(get_type_name::<T>())))?;

        // Initialize component
        component.initialize(self)?;

        // Add component
        self.global_components.insert::<T>(GlobalComponentStorage::<T>::new(component));

        Ok(())
    }

    /// Method used for fetching immutable reference of chosen global component
    pub fn get_global_component<T>(&self) -> Result<&T> 
        where T: GlobalComponent<Storage = GlobalComponentStorage::<T>>
    {
        // Get component
        let component = self.global_components.get::<T>().ok_or(Error::new(EngineError::GlobalComponentNotFound(get_type_name::<T>())))?.data.as_ref().unwrap();
        
        Ok(component)
    }

    /// Method used for fetching mutable reference of chosen global component
    pub fn get_global_component_mut<T>(&mut self) -> Result<&mut T> 
        where T: GlobalComponent<Storage = GlobalComponentStorage::<T>>
    {
        // Get component
        let component = self.global_components.get_mut::<T>().ok_or(Error::new(EngineError::GlobalComponentNotFound(get_type_name::<T>())))?.data.as_mut().unwrap();

        Ok(component)
    }

    /// Method used for removal of a custom global component
    pub fn remove_global_component<T>(&mut self) -> Result<()> 
        where T: GlobalComponent<Storage = GlobalComponentStorage::<T>>
    {
        // Check if component of this type is added
        self.global_components.contains_key::<T>().eq(&true).ok_or(Error::new(EngineError::GlobalComponentNotFound(get_type_name::<T>())))?;

        // Get the type of the component
        let component_type = TypeId::of::<T>();

        // Check if the type of the component is the same as of the ones, which cannot be removed
        if component_type == TypeId::of::<InputComponent>() {
            return Err(Error::new(EngineError::GlobalComponentCannotBeRemoved(get_type_name::<T>())));
        }
        else if component_type == TypeId::of::<TimeComponent>() {
            return Err(Error::new(EngineError::GlobalComponentCannotBeRemoved(get_type_name::<T>())));
        }
        else if component_type == TypeId::of::<AudioManagerComponent>() {
            return Err(Error::new(EngineError::GlobalComponentCannotBeRemoved(get_type_name::<T>())));
        }
        else if component_type == TypeId::of::<DeferredUpdateComponent>() {
            return Err(Error::new(EngineError::GlobalComponentCannotBeRemoved(get_type_name::<T>())));
        }

        // Remove component
        self.global_components.remove::<T>();
        
        Ok(())
    }

    // --- Iterator API ---
    
    /// Method returning an iterator to one ComponentStorage
    /// 
    /// Only components of existant entities are returned
    pub fn iterate_one_component<A>(&self) -> Result<impl Iterator<Item = &RefCell<Option<A>>>> 
        where A: Component<Storage = ComponentStorage<A>>
    {
        // Get scene handle
        let scene_handle = self.scene_manager.get_active_scene_handle()?;

        // Get iterator
        let iterator = self.scene_manager.fetch_one_component_storage::<A>(scene_handle)?;

        Ok(iterator)
    }

    /// Method returning an iterator to one ComponentStorage with adequate entities
    /// 
    /// Only components of existant entities are returned
    pub fn iterate_one_component_with_entities<A>(&self) -> Result<impl Iterator<Item = (EntityHandle, &RefCell<Option<A>>)>> 
        where A: Component<Storage = ComponentStorage<A>>
    {
        // Get scene handle
        let scene_handle = self.scene_manager.get_active_scene_handle()?;

        // Get iterator
        let iterator = self.scene_manager.fetch_one_component_storage_with_entity_handles::<A>(scene_handle)?;

        Ok(iterator)
    }
    
    /// Method returning an iterator to two ComponentStorages
    /// 
    /// Only components of existant entities are returned
    pub fn iterate_two_components<A, B>(&self) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>)>> 
        where 
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>
    {
        // Get scene handle
        let scene_handle = self.scene_manager.get_active_scene_handle()?;

        // Get iterator
        let iterator = self.scene_manager.fetch_two_component_storages::<A, B>(scene_handle)?;

        Ok(iterator) 
    }

    /// Method returning an iterator to two ComponentStorages with adequate entities
    /// 
    /// Only components of existant entities are returned
    pub fn iterate_two_components_with_entities<A, B>(&self) -> Result<impl Iterator<Item = (EntityHandle, &RefCell<Option<A>>, &RefCell<Option<B>>)>> 
        where 
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>
    {
        // Get scene handle
        let scene_handle = self.scene_manager.get_active_scene_handle()?;

        // Get iterator
        let iterator = self.scene_manager.fetch_two_component_storages_with_entity_handles::<A, B>(scene_handle)?;

        Ok(iterator) 
    }

    /// Method returning an iterator to three ComponentStorages
    /// 
    /// Only components of existant entities are returned
    pub fn iterate_three_components<A, B, C>(&self) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>)>> 
        where 
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
        C: Component<Storage = ComponentStorage<C>>
    {

        // Get scene handle
        let scene_handle = self.scene_manager.get_active_scene_handle()?;

        // Get iterator
        let iterator = self.scene_manager.fetch_three_component_storages::<A, B, C>(scene_handle)?;

        Ok(iterator)
    }

    /// Method returning an iterator to three ComponentStorages with adequate entieis
    /// 
    /// Only components of existant entities are returned
    pub fn iterate_three_components_with_entities<A, B, C>(&self) -> Result<impl Iterator<Item = (EntityHandle, &RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>)>> 
        where 
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
        C: Component<Storage = ComponentStorage<C>>
    {

        // Get scene handle
        let scene_handle = self.scene_manager.get_active_scene_handle()?;

        // Get iterator
        let iterator = self.scene_manager.fetch_three_component_storages_with_entity_handles::<A, B, C>(scene_handle)?;

        Ok(iterator)
    }

    /// Method returning an iterator to four ComponentStorages
    /// 
    /// Only components of existant entities are returned
    pub fn iterate_four_components<A, B, C, D>(&self) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>, &RefCell<Option<D>>)>> 
        where 
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
        C: Component<Storage = ComponentStorage<C>>,
        D: Component<Storage = ComponentStorage<D>>
    {
        // Get scene handle
        let scene_handle = self.scene_manager.get_active_scene_handle()?;

        // Get iterator
        let iterator = self.scene_manager.fetch_four_component_storages::<A, B, C, D>(scene_handle)?;

        Ok(iterator) 
    }

    /// Method returning an iterator to four ComponentStorage with adequate entities 
    /// 
    /// Only components of existant entities are returned
    pub fn iterate_four_components_with_entities<A, B, C, D>(&self) -> Result<impl Iterator<Item = (EntityHandle, &RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>, &RefCell<Option<D>>)>> 
        where 
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
        C: Component<Storage = ComponentStorage<C>>,
        D: Component<Storage = ComponentStorage<D>>
    {
        // Get scene handle
        let scene_handle = self.scene_manager.get_active_scene_handle()?;

        // Get iterator
        let iterator = self.scene_manager.fetch_four_component_storages_with_entity_handles::<A, B, C, D>(scene_handle)?;

        Ok(iterator) 
    }

    // --- Scene API ---

    /// Method responsible for scene creation
    /// 
    /// If the operation is succesful, returns SceneHandle
    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        info!("Creating scene: {}", name);
        self.scene_manager.create_scene(name).context(format!("Creating new {} failed", "Scene".gobj_style()))
    }

    /// Method returning handle to the scene chosen by the name
    /// 
    /// If the operation in succesful, returns SceneHandle
    pub fn get_scene_handle(&self, name: &str) -> Result<SceneHandle> {
        self.scene_manager.get_scene_handle(name).context(format!("Getting {} failed", "SceneHandle".sobj_style()))
    }

    /// Method returning scene by the specific SceneHandle
    /// 
    /// If the operation is succesful, returns immutable reference to the Scene
    pub fn get_scene(&self, scene_handle: SceneHandle) -> Result<&Scene> {
        self.scene_manager.get_scene(scene_handle).context(format!("Getting {} failed", "Scene".gobj_style()))
    }

    /// Method returning scene by the specific SceneHandle
    /// 
    /// If the operation is succesful, returns mutable reference to the Scene
    pub fn get_scene_mut(&mut self, scene_handle: SceneHandle) -> Result<&mut Scene> {
        self.scene_manager.get_scene_mut(scene_handle).context(format!("Getting {} as mutable failed", "Scene".gobj_style()))
    }

    /// Method seting active scene by the chosen SceneHandle
    pub fn set_active_scene(&mut self, scene_handle: SceneHandle) -> Result<()> {
        self.scene_manager.set_active_scene(scene_handle).context(format!("Setting active {} failed", "Scene".gobj_style()))
    }

    /// Method returning handle to the active scene
    /// 
    /// If the operation is succesful, returns SceneHandle
    pub fn get_active_scene_handle(&mut self) -> Result<SceneHandle> {
        self.scene_manager.get_active_scene_handle().context(format!("Getting {} of active {} failed", "SceneHandle".sobj_style(), "Scene".gobj_style()))
    }

    /// Method removing scene by the specified SceneHandle
    /// 
    /// If the operation is succesful, returns removed Scene
    pub fn remove_scene(&mut self, scene_handle: SceneHandle) -> Result<Scene> {
        self.scene_manager.remove_scene(scene_handle).context(format!("Removing {} with usage of {} failed", "Scene".sobj_style(), "SceneHandle".gobj_style()))
    }

    /// Method returning scene by the specific SceneHandle
    /// 
    /// If the operation is succesful, returns mutable reference to the Scene
    fn get_active_scene(&mut self) -> Result<&Scene> {
        self.scene_manager.get_active_scene().context(format!("Getting active {} failed", "Scene".gobj_style()))
    }

    fn get_active_scene_mut(&mut self) -> Result<&mut Scene> {
        self.scene_manager.get_active_scene_mut().context(format!("Getting active {} as mutable failed", "Scene".gobj_style()))
    }

    // --- Resource API ---

    /// Method used for registering new resource type
    pub fn register_resource_type<T>(&mut self, max_resource_count: usize) -> Result<()> 
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        self.resource_manager.register_resource_type::<T>(max_resource_count)
    }

    /// Method used for addition of a new resource
    /// 
    /// If the operation is succesful, returns handle to the resource
    pub fn add_resource<T>(&mut self, mut resource: T) -> Result<T::Handle> 
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        debug!("Adding {} {} {}", "Resource".gobj_style(), get_type_name::<T>().sobj_style(), resource.get_name().name_style());

        // Check if resource has proper name
        let resource_name = resource.get_name();
        resource_name.starts_with(DEFAULT_RESOURCE_PREFIX).eq(&false).ok_or(Error::new(EngineError::WrongResourceName(resource_name.clone())))?;

        // Initialize resource
        resource.initialize(self).context(format!("Adding {} {} failed", "Resource".gobj_style(), get_type_name::<T>().sobj_style()))?;
        
        // Add resource and get it back
        let add_result = self.resource_manager.add_resource(resource)?;
        let resource_handle = add_result.0; 
        let resource = add_result.1;

        // Pass handle to resource to this resource so it can store it if needed
        resource.pass_handle::<T::Handle>(resource_handle);

        Ok(resource_handle)
    }

    /// Method used for fetching resource by specified handle
    /// 
    /// If the operation is succesfull, returns the resource
    pub fn get_resource<'a, T>(&'a self, resource_handle: &'a T::Handle) -> Result<&'a T> 
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        Ok(self.resource_manager.get_resource::<T>(resource_handle)?)
    }

    /// Method used for fetching resource by specified name
    /// 
    /// If the operation is succesfull, returns reference to the resource
    pub fn get_resource_by_name<T>(&self, name: &str) -> Result<&T> 
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        Ok(self.resource_manager.get_resource_by_name::<T>(name)?)
    }

    /// Method used for fetching resource by specified name
    /// 
    /// If the operation is succesfull, returns handle to the resource
    pub fn get_resource_handle<T>(&self, name: &str) -> Result<T::Handle> 
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        Ok(self.resource_manager.get_resource_handle::<T>(name)?)
    }

    /// Method used for fetching resource by specified handle
    /// 
    /// If the operation is succesfull, returns mutable reference to the resource
    pub fn get_resource_mut<'a, T>(&'a mut self, resource_handle: &'a T::Handle) -> Result<&'a mut T> 
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        Ok(self.resource_manager.get_resource_mut::<T>(resource_handle)?)
    }

    /// Method used for fetching resource by specified name
    /// 
    /// If the operation is succesfull, returns mutable reference to the resource
    pub fn get_resource_by_name_mut<T>(&mut self, name: &str) -> Result<&mut T> 
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        Ok(self.resource_manager.get_resource_by_name_mut::<T>(name)?)
    }

    /// Method used for removing resource by specified handle of the resource
    pub fn remove_resource<T>(&mut self, resource_handle: &T::Handle) -> Result<()> 
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        let error_message = format!("Removing {} {} failed", "Resource".gobj_style(), get_type_name::<T>().sobj_style());
      
        // Check if resource is not default
        let resource_name = self.resource_manager.get_resource::<T>(resource_handle).context(error_message.to_string())?.get_name();
        resource_name.starts_with(DEFAULT_RESOURCE_PREFIX).eq(&false).ok_or(Error::new(EngineError::RemoveDefaultResource(resource_name.clone()))).context(error_message.to_string())?;

        // Remove resource
        let mut remove_result = self.resource_manager.remove_resource::<T>(resource_handle).context(error_message.to_string())?;
        remove_result.1.destroy(self, *resource_handle)?;

        Ok(())
    }

    /// Method used for removing resource by specified name of the resource
    pub fn remove_resource_by_name<T>(&mut self, name: &str) -> Result<()> 
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        let error_message = format!("Removing {} {} {} failed", "Resource".gobj_style(), get_type_name::<T>().sobj_style(), name.to_string().name_style());

        // Check if resource exists
        self.resource_manager.get_resource_by_name::<T>(name).context(error_message.to_string())?;

        // Check if resource is not default
        name.starts_with(DEFAULT_RESOURCE_PREFIX).eq(&false).ok_or(Error::new(EngineError::RemoveDefaultResource(name.to_string()))).context(error_message.to_string())?;

        // Remove resource
        let mut remove_result = self.resource_manager.remove_resource_by_name::<T>(name).context(error_message.to_string())?;
        remove_result.1.destroy(self, remove_result.0)?;

        Ok(())
    }
}