use crate::{
    resources::*,
    ecs::*,
    config::*,
};

#[cfg(feature = "rendering")]
use crate::graphics::*;

use pill_core::{
    EngineError, PillSlotMapKey, PillStyle, PillTypeMap,
    get_type_name, get_value_type_name, get_enum_variant_type_name,
    get_game_error_message, Vector2f,                    // core math
    Timer,                  // only with internal tools
};

use std::{
    any::{Any, TypeId, type_name},
    collections::VecDeque,
    ops::RangeBounds,
    cell::RefCell,
};

use anyhow::{Context, Result, Error};
use boolinator::Boolinator;
use log::{debug, info, error};

#[cfg(feature = "rendering")]
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{KeyEvent, ElementState, MouseScrollDelta},
    keyboard::{KeyCode, PhysicalKey},
};

// -----------------------------------------------------------------------------
// Public aliases -----------------------------------------------------------------
pub type Game = Box<dyn PillGame>;

#[cfg(feature = "rendering")]
pub type KeyboardKey = KeyCode;
#[cfg(feature = "rendering")]
pub use winit::event::MouseButton;

/// Engine <-> Game interface (user implements this in `start`)
pub trait PillGame {
    fn start(&self, engine: &mut Engine) -> Result<()>;
}

// -----------------------------------------------------------------------------
// Engine struct -----------------------------------------------------------------
pub struct Engine {
    pub(crate) config: config::Config,
    pub(crate) game:   Option<Game>,

    // renderer and windows exist only with the `rendering` feature -------------
    #[cfg(feature = "rendering")]
    pub(crate) renderer: Option<Renderer>,
    #[cfg(feature = "rendering")]
    pub window_size: PhysicalSize<u32>,
    #[cfg(feature = "rendering")]
    pub(crate) input_queue: VecDeque<InputEvent>,
    #[cfg(feature = "rendering")]
    pub render_queue: Vec<RenderQueueItem>,
    #[cfg(feature = "rendering")]
    pub(crate) game_resources_directory_path: std::path::PathBuf,

    // always present -----------------------------------------------------------
    pub scene_manager:   SceneManager,
    pub system_manager:  SystemManager,
    pub resource_manager: ResourceManager,
    pub global_components: PillTypeMap,
    pub frame_delta_time: f32, // ms
}

// -----------------------------------------------------------------------------
// Constructors -----------------------------------------------------------------
impl Engine {
    // ---------- with RENDERING -------------------------------------------------
    #[cfg(feature = "rendering")]
    pub fn new(
        game: Game,
        game_resources_directory_path: std::path::PathBuf,
        renderer: Box<dyn PillRenderer>,
        config: config::Config,
    ) -> Self {
        let max_entities = config
            .get_int("MAX_ENTITIES")
            .unwrap_or(MAX_ENTITIES as i64) as usize;

        Self {
            config,
            game: Some(game),
            renderer: Some(renderer),
            scene_manager: SceneManager::new(max_entities),
            system_manager: SystemManager::new(),
            resource_manager: ResourceManager::new(),
            global_components: PillTypeMap::new(),
            input_queue: VecDeque::new(),
            render_queue: Vec::with_capacity(max_entities),
            window_size: PhysicalSize::default(),
            game_resources_directory_path,
            frame_delta_time: 0.0,
        }
    }

    // ---------- HEADLESS (no rendering) ---------------------------------------
    #[cfg(not(feature = "rendering"))]
    pub fn new(game: Game, config: config::Config) -> Self {
        let max_entities = config
            .get_int("MAX_ENTITIES")
            .unwrap_or(MAX_ENTITIES as i64) as usize;

        Self {
            config,
            game: Some(game),
            scene_manager: SceneManager::new(max_entities),
            system_manager: SystemManager::new(),
            resource_manager: ResourceManager::new(),
            global_components: PillTypeMap::new(),
            frame_delta_time: 0.0,
        }
    }
}

// -----------------------------------------------------------------------------
// INTERNAL helpers (default resources, egui demo window, etc.) -----------------
#[cfg(feature = "rendering")]
impl Engine {
    fn create_default_resources(&mut self) -> Result<()> {
        // limits ---------------------------------------------------------------
        let max_texture_count  = self.config.get_int("MAX_TEXTURES").unwrap_or(MAX_TEXTURES as i64)  as usize;
        let max_mesh_count     = self.config.get_int("MAX_MESHES").unwrap_or(MAX_MESHES as i64)     as usize;
        let max_material_count = self.config.get_int("MAX_MATERIALS").unwrap_or(MAX_MATERIALS as i64) as usize;
        let max_sound_count    = self.config.get_int("MAX_SOUNDS").unwrap_or(MAX_SOUNDS as i64)     as usize;

        // register resource types --------------------------------------------
        self.register_resource_type::<Texture>(max_texture_count)?;
        self.register_resource_type::<Mesh>(max_mesh_count)?;
        self.register_resource_type::<Material>(max_material_count)?;
        self.register_resource_type::<Sound>(max_sound_count)?;

        // master shader & defaults -------------------------------------------
        let master_vert = include_bytes!("../res/shaders/master.vert.glsl");
        let master_frag = include_bytes!("../res/shaders/master.frag.glsl");
        self.renderer
            .as_mut()
            .unwrap()
            .set_master_pipeline(master_vert, master_frag)?;

        let default_color  = Box::new(*include_bytes!("../res/textures/default_color.png"));
        let default_normal = Box::new(*include_bytes!("../res/textures/default_normal.png"));

        let mut color = Texture::new(
            DEFAULT_COLOR_TEXTURE_NAME,
            TextureType::Color,
            ResourceLoadType::Bytes(default_color),
        );
        color.initialize(self)?;
        self.resource_manager.add_resource(color)?;

        let mut normal = Texture::new(
            DEFAULT_NORMAL_TEXTURE_NAME,
            TextureType::Normal,
            ResourceLoadType::Bytes(default_normal),
        );
        normal.initialize(self)?;
        self.resource_manager.add_resource(normal)?;

        let mut mat = Material::new(DEFAULT_MATERIAL_NAME);
        mat.initialize(self)?;
        self.resource_manager.add_resource(mat)?;

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// INITIALIZATION ---------------------------------------------------------------
impl Engine {
    #[cfg(feature = "rendering")]
    pub fn initialize(&mut self, window_size: PhysicalSize<u32>) -> Result<()> {
        self.window_size = window_size;

        // --- Global components ----------------------------------------------
        self.add_global_component(InputComponent::new())?;
        self.add_global_component(TimeComponent::new())?;
        self.add_global_component(DeferredUpdateComponent::new())?;
        self.add_global_component(EguiManagerComponent::new())?;
        //self.add_global_component(PhysicsWorldComponent::new())?;

        let max_ambient = self
            .config
            .get_int("MAX_CONCURRENT_2D_SOUNDS")
            .unwrap_or(MAX_CONCURRENT_2D_SOUNDS as i64)
            as usize;
        let max_spatial = self
            .config
            .get_int("MAX_CONCURRENT_3D_SOUNDS")
            .unwrap_or(MAX_CONCURRENT_3D_SOUNDS as i64)
            as usize;
        self.add_global_component(AudioManagerComponent::new(max_ambient, max_spatial))?;

        // --- Built-in systems ----------------------------------------------
        self.system_manager.add_system("InputSystem", input_system, UpdatePhase::PreGame)?;
        self.system_manager.add_system("TimeSystem",  time_system,  UpdatePhase::PostGame)?;
        //self.system_manager.add_system("PhysicsSystem", physics_system, UpdatePhase::PostGame)?;
        self.system_manager.add_system("AudioSystem", audio_system, UpdatePhase::PostGame)?;
        self.system_manager.add_system("DeferredUpdateSystem", deferred_update_system, UpdatePhase::PostGame)?;
        self.system_manager.add_system("RenderingSystem", rendering_system, UpdatePhase::PostGame)?;

        // --- Resources ------------------------------------------------------
        self.create_default_resources()?;

        // --- Game start ------------------------------------------------------
        self.start_game()
    }

    #[cfg(not(feature = "rendering"))]
    pub fn initialize(&mut self) -> Result<()> {
        self.add_global_component(TimeComponent::new())?;
        self.add_global_component(DeferredUpdateComponent::new())?;

        self.system_manager.add_system("TimeSystem", time_system, UpdatePhase::PostGame)?;

        self.start_game()
    }

    fn start_game(&mut self) -> Result<()> {
        let mut game = self.game.take().ok_or(EngineError::Other("Cannot get game".into()))?;
        let stop_on_game_errors = self
            .config
            .get_bool("PANIC_ON_GAME_ERRORS")
            .unwrap_or(PANIC_ON_GAME_ERRORS);

        let result = game.start(self);
        if stop_on_game_errors {
            result?;
        } else if let Some(msg) = get_game_error_message(result) {
            error!("{}", msg);
        }
        self.game = Some(game);
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// UPDATE / SHUTDOWN (identical for all builds) ---------------------------------
impl Engine {
      pub fn update(&mut self, delta_time: std::time::Duration) {
        let stop_on_game_errors = self.config.get_bool("PANIC_ON_GAME_ERRORS").unwrap_or(PANIC_ON_GAME_ERRORS);

        // Run systems
        for update_phase_index in 0..self.system_manager.update_phases.len() {
            for system_index in 0..self.system_manager.update_phases[update_phase_index].len() {

                // Scope the mutable borrow to system so it ends before calling system_function


                let (system_name, update_phase, system_function);
                {
                    let system = &mut self.system_manager.update_phases[update_phase_index][system_index];
                    if !system.enabled { continue; }
                    system_name = system.name.to_string();
                    update_phase = system.update_phase.clone();
                    system_function = system.system_function;
                }

                // Create new time and asign it to system so it can be accessed inside the system function
                // For rendering system we can't clean its timer here,
                // because it has to render its own timer data in the UI
                // (and since the frame in which it renders is not yet finished when it renders UI, it has to use previous frame timer data)
				#[cfg(feature = "rendering")]
                if system_name != RENDERING_SYSTEM.name  {
                    let mut timer = Timer::new();
                    timer.record_new_context(&format!("{} update", system_name)).unwrap();
                    self.system_manager.update_system_timer(system_name.as_str(), update_phase.clone(), timer).unwrap();
                }

                #[cfg(not(feature = "rendering"))]
                {
                    let mut timer = Timer::new();
                    timer.record_new_context(&format!("{} update", system_name)).unwrap();
                    self.system_manager.update_system_timer(system_name.as_str(), update_phase.clone(), timer).unwrap();
                }

                {
                    // Run system update and handle errors based on configuration
                    let result = (system_function)(self)
                        .context(EngineError::SystemUpdateFailed(system_name.clone(), format!("{:?}", update_phase.clone())));

                    if update_phase == UpdatePhase::Game && stop_on_game_errors {
                        result.unwrap(); // Panic on error if configured
                    } else if let Err(err) = result {
                        if let Some(message) = get_game_error_message(Err(err)) {
                            error!("{}", message);
                        }
                    }
                }

                // Update system timer with the final timer state
                let mut timer = match self.system_manager.get_system_timer(system_name.as_str(), update_phase.clone()) {
                    Ok(Some(timer)) => timer,
                    Ok(None) => {
                        panic!("{}", Error::new(EngineError::NonReturnedSystemTimer(system_name.clone())));
                    }
                    Err(e) => {
                        panic!("{}", Error::new(EngineError::Other(e.to_string())));
                    }
                };

                timer.end_context().context(format!("Failed to end timer context for {}", system_name.clone())).unwrap(); // End system update context
                self.system_manager.update_system_timer(system_name.as_str(), update_phase.clone(), timer).unwrap();
            }
        }

        // Update FPS counter
        let new_frame_time = delta_time.as_secs_f32() * 1000.0;
        let fps = 1000.0 / new_frame_time;
        self.frame_delta_time = new_frame_time.into();
        debug!("Frame finished (Time: {:.3}ms, FPS {:.0})", new_frame_time, fps);
    }


    pub fn shutdown(&mut self) {
        info!("Shutting down {}", "Engine".mobj_style());
    }
}

// -----------------------------------------------------------------------------
// RENDER-ONLY window/input helpers ---------------------------------------------
#[cfg(feature = "rendering")]
impl Engine {
    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        debug!("Window resized to {new_size:?}");
        self.window_size = new_size;
        self.renderer.as_mut().unwrap().resize(new_size);
    }

    pub fn pass_keyboard_key_input(&mut self, ev: &KeyEvent) {
        if let PhysicalKey::Code(code) = ev.physical_key {
            self.input_queue.push_back(InputEvent::KeyboardKey { key: code, state: ev.state });
        }
    }
    pub fn pass_mouse_key_input(&mut self, btn: &MouseButton, st: &ElementState) {
        self.input_queue.push_back(InputEvent::MouseButton { key: *btn, state: *st });
    }
    pub fn pass_mouse_wheel_input(&mut self, d: &MouseScrollDelta) {
        self.input_queue.push_back(InputEvent::MouseWheel { delta: *d });
    }
    pub fn pass_mouse_delta_input(&mut self, d: &(f64, f64)) {
        self.input_queue.push_back(InputEvent::MouseDelta { delta: Vector2f::new(d.0 as f32, d.1 as f32) });
    }
    pub fn pass_mouse_position_input(&mut self, p: &PhysicalPosition<f64>) {
        self.input_queue.push_back(InputEvent::MousePosition { position: Vector2f::new(p.x as f32, p.y as f32) });
    }

    pub fn pass_input_to_egui(&mut self, e: &winit::event::WindowEvent) {
        self.renderer.as_mut().unwrap().pass_input_to_egui(e).unwrap();
    }
    pub fn get_input_queue(&self) -> &VecDeque<InputEvent> {
        &self.input_queue
    }
}

// --- API ------------------------------------------------------------------

/// Pill Engine game API
impl Engine {

    // --- System API ---

    /// Adds game-defined system to the game update phase
    pub fn add_system(&mut self, name: &str, system_function: fn(engine: &mut Engine) -> Result<()>) -> Result<()> {
        debug!("Adding {} {} to {} {}", "System".gobj_style(), name.name_style(), "UpdatePhase".sobj_style(), "Game".name_style());

        self.system_manager.add_system(name, system_function, UpdatePhase::Game).context(format!("Adding {} failed", "System".gobj_style()))
    }

    /// Removes game-defined system
    pub fn remove_system(&mut self, name: &str) -> Result<()> {
        debug!("Removing {} {} from {} {}", "System".gobj_style(), name.name_style(), "UpdatePhase".sobj_style(), "Game".name_style());

        self.system_manager.remove_system(name, UpdatePhase::Game).context(format!("Removing {} failed", "System".gobj_style()))
    }

    /// Toggles game-defined system
    pub fn toggle_system(&mut self, name: &str, enabled: bool) -> Result<()> {
        debug!("Toggling {} {} from {} {} to {} state", "System".gobj_style(), name.name_style(), "UpdatePhase".sobj_style(), "Game".name_style(), if enabled { "Enabled" } else { "Disabled" });

        self.system_manager.toggle_system(name, UpdatePhase::Game, enabled).context(format!("Toggling {} failed", "System".gobj_style()))
    }

    /// Returns system timer. It has to be returned back using update_system_timer function, otherwise engine will panic.
    pub fn get_system_timer(&mut self, name: &str) -> Timer {
        debug!("Getting {} {} timer from {} {}", "System".gobj_style(), name.name_style(), "UpdatePhase".sobj_style(), "Game".name_style());

        self.system_manager.get_system_timer(name, UpdatePhase::Game).unwrap().unwrap()
    }

    // --- Entity API ---

    /// Returns EntityBuilder, allowing for handy entity creation
    pub fn build_entity(&mut self, scene_handle: SceneHandle) -> EntityBuilder {
        let entity_handle = self.create_entity(scene_handle).unwrap();
        EntityBuilder {
            engine: self,
            entity_handle,
            scene_handle,
        }
    }

    // Creates new entity to scene specified with scene handle
    pub fn create_entity(&mut self, scene_handle: SceneHandle) -> Result<EntityHandle> {
        debug!("Creating {} in {} {}", "Entity".gobj_style(), "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        self.scene_manager.create_entity(scene_handle).context(format!("Creating {} failed", "Entity".gobj_style()))
    }

	pub fn create_entity_with_handle(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle) -> Result<EntityHandle> {
        debug!("Creating {} with handle {} in {} {}", "Entity".gobj_style(), entity_handle.data().index, "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        self.scene_manager.create_entity_with_handle(scene_handle, entity_handle).context(format!("Creating {} failed", "Entity".gobj_style()))
    }

    pub fn get_entity_by_handle(&self, scene_handle: SceneHandle, entity_handle: EntityHandle) -> Result<&Entity> {
        debug!("Getting {} with handle {} in {} {}", "Entity".gobj_style(), entity_handle.data().index, "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        self.scene_manager.get_entity_by_handle(scene_handle, entity_handle).context(format!("Getting {} failed", "Entity".gobj_style()))
    }

     // Removes entity specified with entity handle from scene specified with scene handle
    pub fn remove_entity(&mut self, entity_handle: EntityHandle, scene_handle: SceneHandle) -> Result<()> {
        debug!("Removing {} from {} {}", "Entity".gobj_style(), "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        let component_destroyers = self.scene_manager.remove_entity(scene_handle, entity_handle).context(format!("Creating {} failed", "Entity".gobj_style()))?;

        // Destroy components using destroyers
        for mut component_destroyer in component_destroyers {
            component_destroyer.destroy(self, scene_handle, entity_handle)?;
        }

        Ok(())
    }

    // --- Component API ---

    /// Registers new component type in scene specified with scene handle
    pub fn register_component<T>(&mut self, scene_handle: SceneHandle) -> Result<()>
        where T: Component<Storage = ComponentStorage::<T>>
    {
        debug!("Registering {} {} in {} {}", "Component".gobj_style(), get_type_name::<T>().sobj_style(), "Scene".sobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        self.scene_manager.register_component::<T>(scene_handle).context(format!("Registering {} failed", "Component".gobj_style()))
    }

    /// Adds new component to the entity specified with scene and entity handle
    pub fn add_component_to_entity<T>(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle, mut component: T) -> Result<()>
        where T : Component<Storage = ComponentStorage::<T>>
    {
        debug!("Adding {} {} to {} {} in {} {}", "Component".gobj_style(), get_type_name::<T>().sobj_style(), "Entity".gobj_style(), entity_handle.data().index, "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        // Check if already added
        let target_scene = self.scene_manager.get_scene(scene_handle)?;

        if target_scene.entity_has_component::<T>(entity_handle)? {
            return Err(Error::new(EngineError::ComponentAlreadyExists(get_type_name::<T>())))
        }

        // Initialize component
        component.initialize(self).context(format!("Adding {} {} failed", "Component".gobj_style(), get_type_name::<T>().sobj_style()))?;

        // Add component
        self.scene_manager.add_component_to_entity::<T>(scene_handle, entity_handle, component).context(format!("Adding {} to {} failed", "Component".gobj_style(), "Entity".gobj_style()))?;
        let component = self.scene_manager.get_entity_component::<T>(entity_handle, scene_handle)?;

        // Pass handles to entity and scene to this component so it can store it if needed
        component.pass_handles(scene_handle, entity_handle);

        Ok(())
    }

    /// Removes component from the entity specified with scene and entity handle
    pub fn remove_component_from_entity<T>(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle) -> Result<()>
        where T : Component<Storage = ComponentStorage::<T>>
    {
        debug!("Removing {} {} from {} {} in {} {}", "Component".gobj_style(), get_type_name::<T>().sobj_style(), "Entity".gobj_style(), entity_handle.data().index, "Scene".gobj_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        let mut component = self.scene_manager.remove_component_from_entity::<T>(scene_handle, entity_handle).context("Removing component from entity failed").unwrap();

        // Destroy component
        component.destroy(self, scene_handle, entity_handle)?;

        Ok(())
    }

    // --- Global Component API ---

    /// Adds global component to engine
    pub fn add_global_component<T>(&mut self, mut component: T) -> Result<()>
        where T: GlobalComponent<Storage = GlobalComponentStorage::<T>>
    {
        // Check if component of this type is not already added
        if self.global_components.contains_key::<T>() {
            return Err(Error::new(EngineError::GlobalComponentAlreadyExists(get_type_name::<T>())));
        }

        // Initialize component
        component.initialize(self)?;

        // Add component
        self.global_components.insert::<T>(GlobalComponentStorage::<T>::new(component));

        Ok(())
    }

    /// Returns global component
    pub fn get_global_component<T>(&self) -> Result<&T>
        where T: GlobalComponent<Storage = GlobalComponentStorage::<T>>
    {
        // Get component
        let component = self.global_components.get::<T>().ok_or(Error::new(EngineError::GlobalComponentNotFound(get_type_name::<T>())))?.data.as_ref().unwrap();

        Ok(component)
    }

    /// Returns global mutable component
    pub fn get_global_component_mut<T>(&mut self) -> Result<&mut T>
        where T: GlobalComponent<Storage = GlobalComponentStorage::<T>>
    {
        // Get component
        let component = self.global_components.get_mut::<T>().ok_or(Error::new(EngineError::GlobalComponentNotFound(get_type_name::<T>())))?.data.as_mut().unwrap();

        Ok(component)
    }

    /// Removes global component from the engine
    pub fn remove_global_component<T>(&mut self) -> Result<()>
        where T: GlobalComponent<Storage = GlobalComponentStorage::<T>>
    {
        // Check if the type of the component is the same as of the ones, which cannot be removed
        if ENGINE_GLOBAL_COMPONENTS.contains(&TypeId::of::<T>()) {
            return Err(Error::new(EngineError::GlobalComponentCannotBeRemoved(get_type_name::<T>())));
        }

        // Remove and destroy component
        let global_component_storage = self.global_components.remove::<T>().ok_or(EngineError::GlobalComponentNotFound(get_type_name::<T>()))?;
        let mut global_component = global_component_storage.data.unwrap();
        global_component.destroy(self)?;

        Ok(())
    }

    // --- Iterator API ---

    /// Returns iterator for specified component
    ///
    /// Additionally returns entity handle to matching entities
    pub fn iterate_one_component<A>(&self) -> Result<impl Iterator<Item = (EntityHandle, &A)>>
        where A: Component<Storage = ComponentStorage<A>>
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager.get_one_component_iterator::<A>(scene_handle)
    }

    /// Returns iterator for specified component mutable
    ///
    /// Additionally returns entity handle to matching entities
    pub fn iterate_one_component_mut<A>(&mut self) -> Result<impl Iterator<Item = (EntityHandle, &mut A)>>
        where A: Component<Storage = ComponentStorage<A>>
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager.get_one_component_iterator_mut::<A>(scene_handle)
    }

    /// Returns iterator for specified component pair
    ///
    /// Iterator fetches specified components only for those entities which have them all
    /// Additionally returns entity handle to matching entities
    pub fn iterate_two_components<A, B>(&self) -> Result<impl Iterator<Item = (EntityHandle, &A, &B)>>
        where
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager.get_two_component_iterator::<A, B>(scene_handle)
    }

    /// Returns iterator for specified component pair mutable
    ///
    /// Iterator fetches specified components only for those entities which have them all
    /// Additionally returns entity handle to matching entities
    pub fn iterate_two_components_mut<A, B>(&mut self) -> Result<impl Iterator<Item = (EntityHandle, &mut A, &mut B)>>
        where
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager.get_two_component_iterator_mut::<A, B>(scene_handle)
    }

    /// Returns iterator for specified component triple
    ///
    /// Iterator fetches specified components only for those entities which have them all
    /// Additionally returns entity handle to matching entities
    pub fn iterate_three_components<A, B, C>(&self) -> Result<impl Iterator<Item = (EntityHandle, &A, &B, &C)>>
        where
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
        C: Component<Storage = ComponentStorage<C>>
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager.get_three_component_iterator::<A, B, C>(scene_handle)
    }

    /// Returns iterator for specified component triple mutable
    ///
    /// Iterator fetches specified components only for those entities which have them all
    /// Additionally returns entity handle to matching entities
    pub fn iterate_three_components_mut<A, B, C>(&mut self) -> Result<impl Iterator<Item = (EntityHandle, &mut A, &mut B, &mut C)>>
        where
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
        C: Component<Storage = ComponentStorage<C>>
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager.get_three_component_iterator_mut::<A, B, C>(scene_handle)
    }

    pub fn par_for_each2_with<A, B, C, F>(&mut self, chunk: usize, f: F) -> Result<()>
        where
        A: Component<Storage = ComponentStorage<A>> + Send,
        B: Component<Storage = ComponentStorage<B>> + Send,
        C: Component<Storage = ComponentStorage<C>> + Send + Sync,
        F: Fn(&mut A, &mut B) + Send + Sync
    {
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager.par_for_each2_with::<A, B, C, F>(scene_handle, chunk, f)
    }

    /// Returns iterator for specified component triple
    ///
    /// Iterator fetches specified components only for those entities which have them all
    /// Additionally returns entity handle to matching entities
    pub fn iterate_four_components<A, B, C, D>(&self) -> Result<impl Iterator<Item = (EntityHandle, &A, &B, &C, &D)>>
        where
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
        C: Component<Storage = ComponentStorage<C>>,
        D: Component<Storage = ComponentStorage<D>>,
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager.get_four_component_iterator::<A, B, C, D>(scene_handle)
    }

    /// Returns iterator for specified component triple mutable
    ///
    /// Iterator fetches specified components only for those entities which have them all
    /// Additionally returns entity handle to matching entities
    pub fn iterate_four_components_mut<A, B, C, D>(&mut self) -> Result<impl Iterator<Item = (EntityHandle, &mut A, &mut B, &mut C, &mut D)>>
        where
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
        C: Component<Storage = ComponentStorage<C>>,
        D: Component<Storage = ComponentStorage<D>>,
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager.get_four_component_iterator_mut::<A, B, C, D>(scene_handle)
    }

    // --- Scene API ---

    // Creates scene
    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        info!("Creating scene: {}", name);
        self.scene_manager.create_scene(name).context(format!("Creating new {} failed", "Scene".gobj_style()))
    }

    /// Returns handle to the scene specified by its name
    pub fn get_scene_handle(&self, name: &str) -> Result<SceneHandle> {
        self.scene_manager.get_scene_handle(name).context(format!("Getting {} failed", "SceneHandle".sobj_style()))
    }

    pub fn set_active_scene(&mut self, scene_handle: SceneHandle) -> Result<()> {
        self.scene_manager.set_active_scene(scene_handle).context(format!("Setting active {} failed", "Scene".gobj_style()))
    }

    /// Returns handle to the active scene
    pub fn get_active_scene_handle(&self) -> Result<SceneHandle> {
        self.scene_manager.get_active_scene_handle().context(format!("Getting {} of active {} failed", "SceneHandle".sobj_style(), "Scene".gobj_style()))
    }

    // Removes scene deleting all data in it
    pub fn remove_scene(&mut self, scene_handle: SceneHandle) -> Result<()> {
        // Get scene
        let scene = self.scene_manager.get_scene(scene_handle)?;

        // Get entity handles
        let mut entity_handles = Vec::<EntityHandle>::new();
        for (entity_handle, _) in scene.entities.iter() {
            entity_handles.push(entity_handle.clone());
        }

        // Remove entities
        for entity_handle in entity_handles {
            self.remove_entity(entity_handle, scene_handle)?;
        }

        // Remove scene
        self.scene_manager.remove_scene(scene_handle).context(format!("Removing {} with usage of {} failed", "Scene".sobj_style(), "SceneHandle".gobj_style()))?;

        Ok(())
    }

    // --- Resource API ---

    // Registers new resource type in the engine
    pub fn register_resource_type<T>(&mut self, max_resource_count: usize) -> Result<()>
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        self.resource_manager.register_resource_type::<T>(max_resource_count)
    }

    // Adds resource to the engine
    pub fn add_resource<T>(&mut self, mut resource: T) -> Result<T::Handle>
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        debug!("Adding {} {} {}", "Resource".gobj_style(), get_type_name::<T>().sobj_style(), resource.get_name().name_style());

        // Check if resource has proper name
        let resource_name = resource.get_name();
        if resource_name.starts_with(DEFAULT_RESOURCE_PREFIX) {
            return Err(Error::new(EngineError::WrongResourceName(resource_name.clone())))
        }

        // Initialize resource
        resource.initialize(self).context(format!("Adding {} {} failed", "Resource".gobj_style(), get_type_name::<T>().sobj_style()))?;

        // Add resource and get it back
        let add_result = self.resource_manager.add_resource(resource)?;
        let resource_handle = add_result.0;
        let resource = add_result.1;

        // Pass handle to this resource so it can store it if needed
        resource.pass_handle(resource_handle);

        Ok(resource_handle)
    }

    // Returns resource associated with resource handle
    pub fn get_resource<'a, T>(&'a self, resource_handle: &'a T::Handle) -> Result<&'a T>
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        Ok(self.resource_manager.get_resource::<T>(resource_handle)?)
    }

    /// Returns resource specified by its name
    pub fn get_resource_by_name<T>(&self, name: &str) -> Result<&T>
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        Ok(self.resource_manager.get_resource_by_name::<T>(name)?)
    }

    /// Returns handle to resource specified by the name of this resource
    pub fn get_resource_handle<T>(&self, name: &str) -> Result<T::Handle>
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        Ok(self.resource_manager.get_resource_handle::<T>(name)?)
    }

    // Returns mutable resource associated with resource handle
    pub fn get_resource_mut<'a, T>(&'a mut self, resource_handle: &'a T::Handle) -> Result<&'a mut T>
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        Ok(self.resource_manager.get_resource_mut::<T>(resource_handle)?)
    }

    /// Returns mutable resource specified by its name
    pub fn get_resource_by_name_mut<T>(&mut self, name: &str) -> Result<&mut T>
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        Ok(self.resource_manager.get_resource_by_name_mut::<T>(name)?)
    }

    // Removes resource associated with resource handle from the engine
    pub fn remove_resource<T>(&mut self, resource_handle: &T::Handle) -> Result<()>
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        let error_message = format!("Removing {} {} failed", "Resource".gobj_style(), get_type_name::<T>().sobj_style());

        // Check if resource is not default
        let resource_name = self.resource_manager.get_resource::<T>(resource_handle).context(error_message.to_string())?.get_name();
        if resource_name.starts_with(DEFAULT_RESOURCE_PREFIX) {
            return Err(Error::new(EngineError::RemoveDefaultResource(resource_name.clone()))).context(error_message.to_string())
        }

        // Remove and destroy resource
        let mut remove_result = self.resource_manager.remove_resource::<T>(resource_handle).context(error_message.to_string())?;
        remove_result.1.destroy(self, *resource_handle)?;

        Ok(())
    }

    // Removes resource specified with its name from the engine
    pub fn remove_resource_by_name<T>(&mut self, name: &str) -> Result<()>
        where T: Resource<Storage = ResourceStorage::<T>>
    {
        let error_message = format!("Removing {} {} {} failed", "Resource".gobj_style(), get_type_name::<T>().sobj_style(), name.to_string().name_style());

        // Check if resource exists
        self.resource_manager.get_resource_by_name::<T>(name).context(error_message.to_string())?;

        // Check if resource is not default
        if name.starts_with(DEFAULT_RESOURCE_PREFIX) {
            return Err(Error::new(EngineError::RemoveDefaultResource(name.to_string()))).context(error_message.to_string())
        }

        // Remove resource
        let mut remove_result = self.resource_manager.remove_resource_by_name::<T>(name).context(error_message.to_string())?;
        remove_result.1.destroy(self, remove_result.0)?;

        Ok(())
    }
}
