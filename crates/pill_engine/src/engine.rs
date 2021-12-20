use std::{any::type_name, collections::VecDeque, cell::RefCell};
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
};

// ---------------------------------------------------------------------

pub type Game = Box<dyn PillGame>;
pub trait PillGame { 
    fn start(&self, engine: &mut Engine);
}
pub struct Engine { 
    game: Option<Game>,
    pub(crate) renderer: Renderer,
    pub(crate) scene_manager: SceneManager, // [TODO: What will happen with objects registered in renderer if we change the scene for which they were registered?]
    system_manager: SystemManager,
    pub(crate) resource_manager: ResourceManager,
    input_queue: VecDeque<InputEvent>,
    pub(crate) render_queue: Vec<RenderQueueItem>,
}

// ---- INTERNAL -----------------------------------------------------------------

impl Engine {

    #[cfg(feature = "internal")]
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
        };

        engine.create_default_resources();

        engine
    }

    #[cfg(feature = "internal")]
    pub fn initialize(&mut self) {
        info!("Pill Engine initializing");

        self.renderer.initialize(); // [TODO] Needed? Initialization should happen in constructor?
        
        // Add built-in systems
        self.system_manager.add_system("RenderingSystem", rendering_system, UpdatePhase::PostGame).unwrap();

        // Initialize game
        let game = self.game.take().unwrap(); 
        game.start(self); 
        self.game = Some(game);
    }

    #[cfg(feature = "internal")]
    pub fn update(&mut self, dt: std::time::Duration) {
        use pill_core::{PillStyle, get_value_type_name};


        // Run systems
        let update_phase_count = self.system_manager.update_phases.len();
        for i in (0..update_phase_count).rev() {
            let systems_count = self.system_manager.update_phases[i].len();
            for j in (0..systems_count).rev() {
                let system = &self.system_manager.update_phases[i][j];
                let system_name = system.name.to_string();
                (system.system_function)(self).context(format!("{}", EngineError::SystemUpdateFailed(
                    system_name, 
                    get_value_type_name(self.system_manager.update_phases.get_index(i).unwrap().0)
                ))).unwrap();
            }
        }
 
        let frame_time = dt.as_secs_f32() * 1000.0;
        let fps =  1000.0 / frame_time;
        info!("Frame finished (Time: {:.3}ms, FPS {:.0})", frame_time, fps);
    }

    #[cfg(feature = "internal")]
    pub fn shutdown(&mut self) {
        info!("Shutting down");
    }

    #[cfg(feature = "internal")]
    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        info!("Resizing");
        self.renderer.resize(new_size);
    }

    #[cfg(feature = "internal")]
    pub fn pass_keyboard_key_input(&mut self, keyboard_input: &KeyboardInput) {
        let key: VirtualKeyCode = keyboard_input.virtual_keycode.unwrap();
        let state: ElementState = keyboard_input.state;

        let input_event = InputEvent::KeyboardKey { key: key, state: state };
        self.input_queue.push_back(input_event);
        info!("Got new keyboard key input: {:?} {:?}", key, state);
    }

    #[cfg(feature = "internal")]
    pub fn pass_mouse_key_input(&mut self, key: &MouseButton, state: &ElementState) {
        let input_event = InputEvent::MouseKey { key: *key, state: *state }; // Here using * we actually are copying the value of key because MouseButton implements a Copy trait
        self.input_queue.push_back(input_event);
        info!("Got new mouse key input");
    }

    #[cfg(feature = "internal")]
    pub fn pass_mouse_wheel_input(&mut self, delta: &MouseScrollDelta) {
        let input_event = InputEvent::MouseWheel { delta: *delta };
        self.input_queue.push_back(input_event);
        info!("Got new mouse wheel input");
    }

    #[cfg(feature = "internal")]
    pub fn pass_mouse_motion_input(&mut self, position: &PhysicalPosition<f64>) {
        let input_event = InputEvent::MouseMotion { position: *position };
        self.input_queue.push_back(input_event);
        info!("Got new mouse motion input");
    }

    #[cfg(feature = "internal")]
    pub fn get_input_queue(&self) -> &VecDeque<InputEvent> {
        &self.input_queue
    }


    // -- Default resources
    pub fn create_default_resources(&mut self) {
        self.register_resource_type::<TextureHandle, Texture>().unwrap();
        self.register_resource_type::<MeshHandle, Mesh>().unwrap();
        self.register_resource_type::<MaterialHandle, Material>().unwrap();

        // Create default resources

        // Load default resource data to executable
        let default_color_texture_bytes = Box::new(*include_bytes!("../res/textures/default_color.png"));
        let default_normal_texture_bytes = Box::new(*include_bytes!("../res/textures/default_normal.png"));

        // Create default textures
        let default_color_texture = Texture::new("DefaultColor", TextureType::Color, ResourceLoadType::Bytes(default_color_texture_bytes));
        self.add_resource(default_color_texture).unwrap();
        let default_normal_texture = Texture::new("DefaultNormal", TextureType::Normal, ResourceLoadType::Bytes(default_normal_texture_bytes));
        self.add_resource(default_normal_texture).unwrap();

        // Create default material
        let default_material = Material::new("DefaultMaterial");
        self.add_resource(default_material).unwrap();
    }
}

// ---------------------------------------------------------------------

impl Engine { 

    // --- ECS

    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        info!("Creating scene: {}", name);
        self.scene_manager.create_scene(name).context("Scene creation failed")
    }

    pub fn get_active_scene_handle(&mut self) -> Result<SceneHandle> {
        self.scene_manager.get_active_scene_handle().context("Getting active scene handle failed")
    }

    fn get_active_scene(&mut self) -> Result<&Scene> {
        self.scene_manager.get_active_scene().context("Getting active scene failed")
    }

    pub fn set_active_scene(&mut self, scene_handle: SceneHandle) -> Result<()> {
        self.scene_manager.set_active_scene(scene_handle).context("Setting active scene failed")
    }


    // [TODO] Problem is that EntityHandle does not contain scene handle so we can put as parameters handle to scene and handle to entity that is not even in that scene
    // So we need to check if camera component exists for that entity in that scene but even that function will work in a wrong way because it may happen that entity in other scene 
    // will indeed have CameraComponent so this handle it will be assigned but it is actually a handle for entity in different scene...
    // [TODO] we can make "CameraComponent.set_active(engine);" but only when CameraComponent will store handle to entity (and this handle or component will store the scene)
    // [TODO] This may change if entity handles will not be available for game developer
    //pub fn set_active_camera_in_scene(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle) -> Result<()> { 
        //let x = self.scene_manager.get_active_scene().context("Getting active scene failed")?;
        //x.active_camera_entity_handle = entity_handle;
    //}
    // Temp solution:
    // CameraComponents have "enabled" field, rendering system finds first CameraComponent with enabled = true and uses it as active
    // Problem with this approach is that we need to iterate over all entities and if two are enabled we don't actually know which will be used because index of component
    // does not mean that this component is first, it could be created later than other and there was empty slot for this entity so now it is first.



    pub fn register_component<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene_handle: SceneHandle) -> Result<()> {
        self.scene_manager.register_component::<T>(scene_handle).context("Registering component failed")
    }

    pub fn add_system(&mut self, name: &str, system_function: fn(engine: &mut Engine) -> Result<()>) -> Result<()> {
        self.system_manager.add_system(name, system_function, UpdatePhase::Game).context("Adding system failed")
    }

    // [TODO] Implement remove_system

    pub fn create_entity(&mut self, scene_handle: SceneHandle) -> Result<EntityHandle> {
        self.scene_manager.create_entity(scene_handle).context(format!("Creating {} failed", "Entity".gobj_style()))
    }
    
    pub fn add_component_to_entity<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle, component: T) -> Result<()> {
        info!("Adding {} {} to {} {} in {} {}", "Component".gobj_style(), get_type_name::<T>().sobj_style(), "Entity".gobj_style(), entity_handle.index, "Scene".gobj_style(), scene_handle.index);
        self.scene_manager.add_component_to_entity::<T>(scene_handle, entity_handle, component).context("Adding component to entity failed")
    }

    pub fn delete_component_from_entity<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle) -> Result<()> {
        info!("Deleting {} {} from {} {} in {} {}", "Component".gobj_style(), get_type_name::<T>().sobj_style(), "Entity".gobj_style(), entity_handle.index, "Scene".gobj_style(), scene_handle.index);
        self.scene_manager.delete_component_from_entity::<T>(scene_handle, entity_handle).context("Deleting component from entity failed")
    }

    // Iterators

    pub fn fetch_one_component_storage<A: Component<Storage = ComponentStorage<A>>>(&mut self) -> Result<impl Iterator<Item = &RefCell<Option<A>>>> {
        // Get iterator
        let iterator = self.scene_manager.fetch_one_component_storage::<A>(self.scene_manager.get_active_scene_handle()?)?;

        Ok(iterator)
    }
    
    pub fn fetch_two_component_storages<A: Component<Storage = ComponentStorage::<A>>, 
                            B: Component<Storage = ComponentStorage<B>>>(&mut self) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>)>> {

        // Get iterator
        let iterator = self.scene_manager.fetch_two_component_storages::<A, B>(self.scene_manager.get_active_scene_handle()?)?;

        Ok(iterator) 
    }

    pub fn fetch_three_component_storages<A: Component<Storage = ComponentStorage::<A>>, 
                            B: Component<Storage = ComponentStorage<B>>,
                            C: Component<Storage = ComponentStorage<C>>>(&mut self) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>)>> {

        //Get iterator
        let iterator = self.scene_manager.fetch_three_component_storages::<A, B, C>(self.scene_manager.get_active_scene_handle()?)?;

        Ok(iterator)
    }

    pub fn fetch_four_component_storages<A: Component<Storage = ComponentStorage::<A>>, 
                            B: Component<Storage = ComponentStorage<B>>,
                            C: Component<Storage = ComponentStorage<C>>,
                            D: Component<Storage = ComponentStorage<D>>>(&mut self) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>, &RefCell<Option<D>>)>> {

        //Get iterator
        let iterator = self.scene_manager.fetch_four_component_storages::<A, B, C, D>(self.scene_manager.get_active_scene_handle()?)?;

        Ok(iterator) 
    }

    // [TODO] Implement get_component_from_entity


    
    // --- RESOURCES

    pub fn add_resource<H: PillSlotMapKey, T: Resource<Value = ResourceStorage::<H, T>>>(&mut self, mut resource: T) -> Result<H> {

        resource.initialize(self);
        
        let resource_handle = self.resource_manager.add_resource(resource)?;

        // [TODO]: In resource initialization it may happen that renderer resource will be created, and after that add_resource may fail, so resource will not be added to the engine 
        // but rendering resource will be left there without any user. This should be handled! 
        // (We can first check if there will be a place in resource_manager and then start procedure, or if add_resource will fail then deinitialize resource what will remove renderer resource)

        Ok(resource_handle)
    }

    pub fn register_resource_type<H: PillSlotMapKey, T: Resource<Value = ResourceStorage::<H, T>>>(&mut self) -> Result<()> {
        self.resource_manager.register_resource_type::<H, T>()
    }



    // pub fn load_resource<T: Resource>(&mut self, t: T, path: String, source: ResourceSource) {
    //     self.resource_manager.load_resource(t, path, source)
    // }

}