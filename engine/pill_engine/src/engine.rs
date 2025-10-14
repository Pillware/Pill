use crate::{config::*, ecs::*, graphics::*, resources::*};

use pill_core::{
    get_enum_variant_type_name, get_game_error_message, get_type_name, get_value_type_name,
    EngineError, PillSlotMapKey, PillStyle, PillTypeMap, Timer, Vector2f,
};

use anyhow::{Context, Error, Result};
use boolinator::Boolinator;
use log::{debug, error, info};
use std::{
    any::type_name, any::Any, any::TypeId, cell::RefCell, collections::VecDeque, ops::RangeBounds,
};
use winit::{dpi::PhysicalPosition, event::KeyEvent};

// -------------------------------------------------------------------------------

pub type Game = Box<dyn PillGame>;
pub type KeyboardKey = winit::keyboard::KeyCode;
pub type MouseButton = winit::event::MouseButton;

/// Engine <-> Game interface
///
/// Entry point of the game project. Mandatory to implement.
pub trait PillGame {
    fn start(&self, engine: &mut Engine) -> Result<()>;
}

/// Heart of Pill Engine
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
    pub(crate) game_resources_directory_path: std::path::PathBuf,
    pub(crate) frame_delta_time: f32, // In milliseconds
}

// ---- INTERNAL API -----------------------------------------------------------------
struct EguiUI<'a> {
    frame_delta_time: &'a f32, // Use a reference to frame_delta_time
}

impl<'a> EguiUI<'a> {
    fn render(&self, ui: &egui::Context) {
        egui::Window::new("PillEngine")
            .default_open(true)
            .resizable(true)
            .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
            .show(ui, |ui| {
                if ui.add(egui::Button::new("Click me")).clicked() {
                    println!("PRESSED");
                    println!("{}", self.frame_delta_time);
                }
            });
    }
}
/// Pill Engine internal API
#[cfg(feature = "internal")]
impl Engine {
    pub fn new(
        game: Box<dyn PillGame>,
        game_resources_directory_path: std::path::PathBuf,
        renderer: Box<dyn PillRenderer>,
        config: config::Config,
    ) -> Self {
        let max_entity_count = config
            .get_int("MAX_ENTITIES")
            .unwrap_or(MAX_ENTITIES as i64) as usize;

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
            game_resources_directory_path,
            frame_delta_time: 0.0.into(),
        }
    }

    fn create_default_resources(&mut self) -> Result<()> {
        // limits ---------------------------------------------------------------
        let max_texture_count = self
            .config
            .get_int("MAX_TEXTURES")
            .unwrap_or(MAX_TEXTURES as i64) as usize;
        let max_mesh_count = self
            .config
            .get_int("MAX_MESHES")
            .unwrap_or(MAX_MESHES as i64) as usize;
        let max_material_count = self
            .config
            .get_int("MAX_MATERIALS")
            .unwrap_or(MAX_MATERIALS as i64) as usize;
        let max_sound_count = self
            .config
            .get_int("MAX_SOUNDS")
            .unwrap_or(MAX_SOUNDS as i64) as usize;

        // register resource types --------------------------------------------
        self.register_resource_type::<Texture>(max_texture_count)?;
        self.register_resource_type::<Mesh>(max_mesh_count)?;
        self.register_resource_type::<Material>(max_material_count)?;
        self.register_resource_type::<Sound>(max_sound_count)?;

        // master shader & defaults -------------------------------------------
        // Legacy master pipeline removed; PSOs are created upfront by renderer

        let default_color = Box::new(*include_bytes!("../res/textures/default_color.png"));
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

    fn start_game(&mut self) -> Result<()> {
        info!("Starting {}", "Game".mobj_style());

        let game = self
            .game
            .take()
            .ok_or(EngineError::Other("Cannot get game".to_string()))?;
        let stop_on_game_errors = self
            .config
            .get_bool("PANIC_ON_GAME_ERRORS")
            .unwrap_or(PANIC_ON_GAME_ERRORS);
        let result = game.start(self);
        match stop_on_game_errors {
            true => result.context(format!("{} error", "Game".mobj_style()))?,
            false => {
                if let Some(message) = get_game_error_message(result) {
                    error!("{}", message);
                }
            }
        }
        self.game = Some(game);

        Ok(())
    }

    /// Initializes Pill Engine
    ///
    /// Creates default global components, adds default systems, creates default resources, initializes game
    pub fn initialize(&mut self, window_size: winit::dpi::PhysicalSize<u32>) -> Result<()> {
        info!("Initializing {}", "Engine".mobj_style());

        // Set window size
        self.window_size = window_size;

        // Register global components
        self.add_global_component(InputComponent::new())?;
        self.add_global_component(TimeComponent::new())?;
        self.add_global_component(DeferredUpdateComponent::new())?;
        self.add_global_component(EguiManagerComponent::new())?;

        let max_ambient_sink_count =
            self.config
                .get_int("MAX_CONCURRENT_2D_SOUNDS")
                .unwrap_or(MAX_CONCURRENT_2D_SOUNDS as i64) as usize;
        let max_spatial_sink_count =
            self.config
                .get_int("MAX_CONCURRENT_3D_SOUNDS")
                .unwrap_or(MAX_CONCURRENT_3D_SOUNDS as i64) as usize;
        self.add_global_component(AudioManagerComponent::new(
            max_ambient_sink_count,
            max_spatial_sink_count,
        ))?;

        // Add built-in systems
        self.system_manager.add_system(
            INPUT_SYSTEM.name,
            INPUT_SYSTEM.system_function,
            INPUT_SYSTEM.update_phase,
        )?;
        self.system_manager.add_system(
            TIME_SYSTEM.name,
            TIME_SYSTEM.system_function,
            TIME_SYSTEM.update_phase,
        )?;
        self.system_manager.add_system(
            AUDIO_SYSTEM.name,
            AUDIO_SYSTEM.system_function,
            AUDIO_SYSTEM.update_phase,
        )?;
        self.system_manager.add_system(
            DEFERRED_UPDATE_SYSTEM.name,
            DEFERRED_UPDATE_SYSTEM.system_function,
            DEFERRED_UPDATE_SYSTEM.update_phase,
        )?;
        self.system_manager.add_system(
            RENDERING_SYSTEM.name,
            RENDERING_SYSTEM.system_function,
            RENDERING_SYSTEM.update_phase,
        )?;

        // Create default resources
        self.create_default_resources()
            .context("Failed to create default resources")?;

        // Start game
        self.start_game()?;

        Ok(())
    }

    /// Main engine update function
    ///
    /// Runs all systems in order: PreGame -> Game -> PostGame
    pub fn update(&mut self, delta_time: std::time::Duration) {
        let stop_on_game_errors = self
            .config
            .get_bool("PANIC_ON_GAME_ERRORS")
            .unwrap_or(PANIC_ON_GAME_ERRORS);

        // Run systems
        for update_phase_index in 0..self.system_manager.update_phases.len() {
            for system_index in 0..self.system_manager.update_phases[update_phase_index].len() {
                let (system_name, update_phase, system_function);
                {
                    let system =
                        &mut self.system_manager.update_phases[update_phase_index][system_index];
                    if !system.enabled {
                        continue;
                    }
                    system_name = system.name.to_string();
                    update_phase = system.update_phase.clone();
                    system_function = system.system_function;
                }

                // Create new time and asign it to system so it can be accessed inside the system function
                // For rendering system we can't clean its timer here,
                // because it has to render its own timer data in the UI
                // (and since the frame in which it renders is not yet finished when it renders UI, it has to use previous frame timer data)
                if system_name != RENDERING_SYSTEM.name {
                    let mut timer = Timer::new();
                    timer.begin_context(&format!("{} system update", system_name));
                    self.system_manager
                        .update_system_timer(system_name.as_str(), update_phase.clone(), timer)
                        .unwrap();
                }

                {
                    // Run system update and handle errors based on configuration
                    let result = (system_function)(self).context(EngineError::SystemUpdateFailed(
                        system_name.clone(),
                        format!("{:?}", update_phase.clone()),
                    ));

                    if update_phase == UpdatePhase::Game && stop_on_game_errors {
                        result.unwrap(); // Panic on error if configured
                    } else if let Err(err) = result {
                        if let Some(message) = get_game_error_message(Err(err)) {
                            error!("{}", message);
                        }
                    }
                }

                // Update system timer with the final timer state
                let mut timer = match self
                    .system_manager
                    .get_system_timer(system_name.as_str(), update_phase.clone())
                {
                    Ok(Some(timer)) => timer,
                    Ok(None) => {
                        panic!(
                            "{}",
                            Error::new(EngineError::NonReturnedSystemTimer(system_name.clone()))
                        );
                    }
                    Err(e) => {
                        panic!("{}", Error::new(EngineError::Other(e.to_string())));
                    }
                };

                timer
                    .end_context()
                    .context(format!(
                        "Failed to end timer context for {}",
                        system_name.clone()
                    ))
                    .unwrap(); // End system update context
                self.system_manager
                    .update_system_timer(system_name.as_str(), update_phase.clone(), timer)
                    .unwrap();
            }
        }

        // Update FPS counter
        let new_frame_time = delta_time.as_secs_f32() * 1000.0;
        self.frame_delta_time = new_frame_time.into();
    }

    pub fn shutdown(&mut self) {
        info!("Shutting down {}", "Engine".mobj_style());
    }

    pub fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        debug!(
            "{} resized to {}x{}",
            "Window".mobj_style(),
            new_window_size.width,
            new_window_size.height
        );
        self.window_size = new_window_size;
        self.renderer.resize(new_window_size);
    }

    pub fn pass_keyboard_key_input(&mut self, keyboard_input: &KeyEvent) {
        let state: winit::event::ElementState = keyboard_input.state;
        match keyboard_input.physical_key {
            winit::keyboard::PhysicalKey::Code(key_code) => {
                let input_event = InputEvent::KeyboardKey {
                    key: key_code,
                    state: state,
                };
                self.input_queue.push_back(input_event);
                debug!("Got new keyboard key input: {:?} {:?}", key_code, state);
            }
            winit::keyboard::PhysicalKey::Unidentified(_) => {
                debug!("Unidentified key input: {:?}", keyboard_input.physical_key);
            }
        }
    }

    pub fn pass_mouse_key_input(&mut self, key: &MouseButton, state: &winit::event::ElementState) {
        let input_event = InputEvent::MouseButton {
            key: *key,
            state: *state,
        };
        self.input_queue.push_back(input_event);
        debug!("Got new mouse key input");
    }

    pub fn pass_mouse_wheel_input(&mut self, delta: &winit::event::MouseScrollDelta) {
        let input_event = InputEvent::MouseWheel { delta: *delta };
        self.input_queue.push_back(input_event);
        debug!("Got new mouse wheel input");
    }

    pub fn pass_mouse_delta_input(&mut self, delta: &(f64, f64)) {
        let input_event = InputEvent::MouseDelta {
            delta: Vector2f::new(delta.0 as f32, delta.1 as f32),
        };
        self.input_queue.push_back(input_event);
        debug!("Got new mouse motion input");
    }

    pub fn pass_mouse_position_input(&mut self, position: &PhysicalPosition<f64>) {
        let input_event = InputEvent::MousePosition {
            position: Vector2f::new(position.x as f32, position.y as f32),
        };
        self.input_queue.push_back(input_event);
        debug!("Got new mouse position input");
    }

    pub fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) {
        self.renderer.pass_input_to_egui(event).unwrap();
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
    pub fn add_system(
        &mut self,
        name: &str,
        system_function: fn(engine: &mut Engine) -> Result<()>,
    ) -> Result<()> {
        debug!(
            "Adding {} {} to {} {}",
            "System".gobj_style(),
            name.name_style(),
            "UpdatePhase".sobj_style(),
            "Game".name_style()
        );

        self.system_manager
            .add_system(name, system_function, UpdatePhase::Game)
            .context(format!("Adding {} failed", "System".gobj_style()))
    }

    /// Removes game-defined system
    pub fn remove_system(&mut self, name: &str) -> Result<()> {
        debug!(
            "Removing {} {} from {} {}",
            "System".gobj_style(),
            name.name_style(),
            "UpdatePhase".sobj_style(),
            "Game".name_style()
        );

        self.system_manager
            .remove_system(name, UpdatePhase::Game)
            .context(format!("Removing {} failed", "System".gobj_style()))
    }

    /// Toggles game-defined system
    pub fn toggle_system(
        &mut self,
        name: &str,
        update_phase: UpdatePhase,
        enabled: bool,
    ) -> Result<()> {
        debug!(
            "Toggling {} {} from {} {} to {} state",
            "System".gobj_style(),
            name.name_style(),
            "UpdatePhase".sobj_style(),
            update_phase.to_string().name_style(),
            if enabled { "Enabled" } else { "Disabled" }
        );

        self.system_manager
            .toggle_system(name, update_phase, enabled)
            .context(format!("Toggling {} failed", "System".gobj_style()))
    }

    /// Returns system timer. It has to be returned back using update_system_timer function, otherwise engine will panic.
    pub fn get_system_timer(&mut self, name: &str) -> Timer {
        debug!(
            "Getting {} {} timer from {} {}",
            "System".gobj_style(),
            name.name_style(),
            "UpdatePhase".sobj_style(),
            "Game".name_style()
        );

        self.system_manager
            .get_system_timer(name, UpdatePhase::Game)
            .unwrap()
            .unwrap()
    }

    // --- Entity API ---

    /// Returns EntityBuilder, allowing for handy entity creation
    pub fn build_entity(&mut self, scene_handle: SceneHandle) -> EntityBuilder<'_> {
        let entity_handle = self.create_entity(scene_handle).unwrap();
        EntityBuilder {
            engine: self,
            entity_handle,
            scene_handle,
        }
    }

    // Creates new entity to scene specified with scene handle
    pub fn create_entity(&mut self, scene_handle: SceneHandle) -> Result<EntityHandle> {
        debug!(
            "Creating {} in {} {}",
            "Entity".gobj_style(),
            "Scene".gobj_style(),
            self.scene_manager
                .get_scene(scene_handle)
                .unwrap()
                .name
                .name_style()
        );

        self.scene_manager
            .create_entity(scene_handle)
            .context(format!("Creating {} failed", "Entity".gobj_style()))
    }

    // Removes entity specified with entity handle from scene specified with scene handle
    pub fn remove_entity(
        &mut self,
        entity_handle: EntityHandle,
        scene_handle: SceneHandle,
    ) -> Result<()> {
        debug!(
            "Removing {} from {} {}",
            "Entity".gobj_style(),
            "Scene".gobj_style(),
            self.scene_manager
                .get_scene(scene_handle)
                .unwrap()
                .name
                .name_style()
        );

        let component_destroyers = self
            .scene_manager
            .remove_entity(scene_handle, entity_handle)
            .context(format!("Creating {} failed", "Entity".gobj_style()))?;

        // Destroy components using destroyers
        for mut component_destroyer in component_destroyers {
            component_destroyer.destroy(self, scene_handle, entity_handle)?;
        }

        Ok(())
    }

    // --- Component API ---

    /// Registers new component type in scene specified with scene handle
    pub fn register_component<T>(&mut self, scene_handle: SceneHandle) -> Result<()>
    where
        T: Component<Storage = ComponentStorage<T>>,
    {
        debug!(
            "Registering {} {} in {} {}",
            "Component".gobj_style(),
            get_type_name::<T>().sobj_style(),
            "Scene".sobj_style(),
            self.scene_manager
                .get_scene(scene_handle)
                .unwrap()
                .name
                .name_style()
        );

        self.scene_manager
            .register_component::<T>(scene_handle)
            .context(format!("Registering {} failed", "Component".gobj_style()))
    }

    /// Adds new component to the entity specified with scene and entity handle
    pub fn add_component_to_entity<T>(
        &mut self,
        scene_handle: SceneHandle,
        entity_handle: EntityHandle,
        mut component: T,
    ) -> Result<()>
    where
        T: Component<Storage = ComponentStorage<T>>,
    {
        debug!(
            "Adding {} {} to {} {} in {} {}",
            "Component".gobj_style(),
            get_type_name::<T>().sobj_style(),
            "Entity".gobj_style(),
            entity_handle.data().index,
            "Scene".gobj_style(),
            self.scene_manager
                .get_scene(scene_handle)
                .unwrap()
                .name
                .name_style()
        );

        // Check if already added
        let target_scene = self.scene_manager.get_scene(scene_handle)?;

        if target_scene.entity_has_component::<T>(entity_handle)? {
            return Err(Error::new(EngineError::ComponentAlreadyExists(
                get_type_name::<T>(),
            )));
        }

        // Initialize component
        component.initialize(self).context(format!(
            "Adding {} {} failed",
            "Component".gobj_style(),
            get_type_name::<T>().sobj_style()
        ))?;

        // Add component
        self.scene_manager
            .add_component_to_entity::<T>(scene_handle, entity_handle, component)
            .context(format!(
                "Adding {} to {} failed",
                "Component".gobj_style(),
                "Entity".gobj_style()
            ))?;
        let component = self
            .scene_manager
            .get_entity_component::<T>(entity_handle, scene_handle)?;

        // Pass handles to entity and scene to this component so it can store it if needed
        component.pass_handles(scene_handle, entity_handle);

        Ok(())
    }

    /// Removes component from the entity specified with scene and entity handle
    pub fn remove_component_from_entity<T>(
        &mut self,
        scene_handle: SceneHandle,
        entity_handle: EntityHandle,
    ) -> Result<()>
    where
        T: Component<Storage = ComponentStorage<T>>,
    {
        debug!(
            "Removing {} {} from {} {} in {} {}",
            "Component".gobj_style(),
            get_type_name::<T>().sobj_style(),
            "Entity".gobj_style(),
            entity_handle.data().index,
            "Scene".gobj_style(),
            self.scene_manager
                .get_scene(scene_handle)
                .unwrap()
                .name
                .name_style()
        );

        let mut component = self
            .scene_manager
            .remove_component_from_entity::<T>(scene_handle, entity_handle)
            .context("Removing component from entity failed")
            .unwrap();

        // Destroy component
        component.destroy(self, scene_handle, entity_handle)?;

        Ok(())
    }

    // --- Global Component API ---

    /// Adds global component to engine
    pub fn add_global_component<T>(&mut self, mut component: T) -> Result<()>
    where
        T: GlobalComponent<Storage = GlobalComponentStorage<T>>,
    {
        // Check if component of this type is not already added
        if self.global_components.contains_key::<T>() {
            return Err(Error::new(EngineError::GlobalComponentAlreadyExists(
                get_type_name::<T>(),
            )));
        }

        // Initialize component
        component.initialize(self)?;

        // Add component
        self.global_components
            .insert::<T>(GlobalComponentStorage::<T>::new(component));

        Ok(())
    }

    /// Returns global component
    pub fn get_global_component<T>(&self) -> Result<&T>
    where
        T: GlobalComponent<Storage = GlobalComponentStorage<T>>,
    {
        // Get component
        let component = self
            .global_components
            .get::<T>()
            .ok_or(Error::new(EngineError::GlobalComponentNotFound(
                get_type_name::<T>(),
            )))?
            .data
            .as_ref()
            .unwrap();

        Ok(component)
    }

    /// Returns global mutable component
    pub fn get_global_component_mut<T>(&mut self) -> Result<&mut T>
    where
        T: GlobalComponent<Storage = GlobalComponentStorage<T>>,
    {
        // Get component
        let component = self
            .global_components
            .get_mut::<T>()
            .ok_or(Error::new(EngineError::GlobalComponentNotFound(
                get_type_name::<T>(),
            )))?
            .data
            .as_mut()
            .unwrap();

        Ok(component)
    }

    /// Removes global component from the engine
    pub fn remove_global_component<T>(&mut self) -> Result<()>
    where
        T: GlobalComponent<Storage = GlobalComponentStorage<T>>,
    {
        // Check if the type of the component is the same as of the ones, which cannot be removed
        if ENGINE_GLOBAL_COMPONENTS.contains(&TypeId::of::<T>()) {
            return Err(Error::new(EngineError::GlobalComponentCannotBeRemoved(
                get_type_name::<T>(),
            )));
        }

        // Remove and destroy component
        let global_component_storage = self
            .global_components
            .remove::<T>()
            .ok_or(EngineError::GlobalComponentNotFound(get_type_name::<T>()))?;
        let mut global_component = global_component_storage.data.unwrap();
        global_component.destroy(self)?;

        Ok(())
    }

    // --- Iterator API ---

    /// Returns iterator for specified component
    ///
    /// Additionally returns entity handle to matching entities
    pub fn iterate_one_component<A>(&self) -> Result<impl Iterator<Item = (EntityHandle, &A)>>
    where
        A: Component<Storage = ComponentStorage<A>>,
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager
            .get_one_component_iterator::<A>(scene_handle)
    }

    /// Returns iterator for specified component mutable
    ///
    /// Additionally returns entity handle to matching entities
    pub fn iterate_one_component_mut<A>(
        &mut self,
    ) -> Result<impl Iterator<Item = (EntityHandle, &mut A)>>
    where
        A: Component<Storage = ComponentStorage<A>>,
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager
            .get_one_component_iterator_mut::<A>(scene_handle)
    }

    /// Returns iterator for specified component pair
    ///
    /// Iterator fetches specified components only for those entities which have them all
    /// Additionally returns entity handle to matching entities
    pub fn iterate_two_components<A, B>(
        &self,
    ) -> Result<impl Iterator<Item = (EntityHandle, &A, &B)>>
    where
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager
            .get_two_component_iterator::<A, B>(scene_handle)
    }

    /// Returns iterator for specified component pair mutable
    ///
    /// Iterator fetches specified components only for those entities which have them all
    /// Additionally returns entity handle to matching entities
    pub fn iterate_two_components_mut<A, B>(
        &mut self,
    ) -> Result<impl Iterator<Item = (EntityHandle, &mut A, &mut B)>>
    where
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager
            .get_two_component_iterator_mut::<A, B>(scene_handle)
    }

    /// Returns iterator for specified component triple
    ///
    /// Iterator fetches specified components only for those entities which have them all
    /// Additionally returns entity handle to matching entities
    pub fn iterate_three_components<A, B, C>(
        &self,
    ) -> Result<impl Iterator<Item = (EntityHandle, &A, &B, &C)>>
    where
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
        C: Component<Storage = ComponentStorage<C>>,
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager
            .get_three_component_iterator::<A, B, C>(scene_handle)
    }

    /// Returns iterator for specified component triple mutable
    ///
    /// Iterator fetches specified components only for those entities which have them all
    /// Additionally returns entity handle to matching entities
    pub fn iterate_three_components_mut<A, B, C>(
        &mut self,
    ) -> Result<impl Iterator<Item = (EntityHandle, &mut A, &mut B, &mut C)>>
    where
        A: Component<Storage = ComponentStorage<A>>,
        B: Component<Storage = ComponentStorage<B>>,
        C: Component<Storage = ComponentStorage<C>>,
    {
        // Get scene handle and iterator
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        self.scene_manager
            .get_three_component_iterator_mut::<A, B, C>(scene_handle)
    }

    // --- Scene API ---

    // Creates scene
    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        info!("Creating scene: {}", name);
        self.scene_manager
            .create_scene(name)
            .context(format!("Creating new {} failed", "Scene".gobj_style()))
    }

    /// Returns handle to the scene specified by its name
    pub fn get_scene_handle(&self, name: &str) -> Result<SceneHandle> {
        self.scene_manager
            .get_scene_handle(name)
            .context(format!("Getting {} failed", "SceneHandle".sobj_style()))
    }

    pub fn set_active_scene(&mut self, scene_handle: SceneHandle) -> Result<()> {
        self.scene_manager
            .set_active_scene(scene_handle)
            .context(format!("Setting active {} failed", "Scene".gobj_style()))
    }

    /// Returns handle to the active scene
    pub fn get_active_scene_handle(&self) -> Result<SceneHandle> {
        self.scene_manager
            .get_active_scene_handle()
            .context(format!(
                "Getting {} of active {} failed",
                "SceneHandle".sobj_style(),
                "Scene".gobj_style()
            ))
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
        self.scene_manager
            .remove_scene(scene_handle)
            .context(format!(
                "Removing {} with usage of {} failed",
                "Scene".sobj_style(),
                "SceneHandle".gobj_style()
            ))?;

        Ok(())
    }

    // --- Resource API ---

    // Registers new resource type in the engine
    pub fn register_resource_type<T>(&mut self, max_resource_count: usize) -> Result<()>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        self.resource_manager
            .register_resource_type::<T>(max_resource_count)
    }

    // Adds resource to the engine
    pub fn add_resource<T>(&mut self, mut resource: T) -> Result<T::Handle>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        debug!(
            "Adding {} {} {}",
            "Resource".gobj_style(),
            get_type_name::<T>().sobj_style(),
            resource.get_name().name_style()
        );

        // Check if resource has proper name
        let resource_name = resource.get_name();
        if resource_name.starts_with(DEFAULT_RESOURCE_PREFIX) {
            return Err(Error::new(EngineError::WrongResourceName(
                resource_name.clone(),
            )));
        }

        // Initialize resource
        resource.initialize(self).context(format!(
            "Adding {} {} failed",
            "Resource".gobj_style(),
            get_type_name::<T>().sobj_style()
        ))?;

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
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        Ok(self.resource_manager.get_resource::<T>(resource_handle)?)
    }

    /// Returns resource specified by its name
    pub fn get_resource_by_name<T>(&self, name: &str) -> Result<&T>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        Ok(self.resource_manager.get_resource_by_name::<T>(name)?)
    }

    /// Returns handle to resource specified by the name of this resource
    pub fn get_resource_handle<T>(&self, name: &str) -> Result<T::Handle>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        Ok(self.resource_manager.get_resource_handle::<T>(name)?)
    }

    // Returns mutable resource associated with resource handle
    pub fn get_resource_mut<'a, T>(
        &'a mut self,
        resource_handle: &'a T::Handle,
    ) -> Result<&'a mut T>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        Ok(self
            .resource_manager
            .get_resource_mut::<T>(resource_handle)?)
    }

    /// Returns mutable resource specified by its name
    pub fn get_resource_by_name_mut<T>(&mut self, name: &str) -> Result<&mut T>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        Ok(self.resource_manager.get_resource_by_name_mut::<T>(name)?)
    }

    // Removes resource associated with resource handle from the engine
    pub fn remove_resource<T>(&mut self, resource_handle: &T::Handle) -> Result<()>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        let error_message = format!(
            "Removing {} {} failed",
            "Resource".gobj_style(),
            get_type_name::<T>().sobj_style()
        );

        // Check if resource is not default
        let resource_name = self
            .resource_manager
            .get_resource::<T>(resource_handle)
            .context(error_message.to_string())?
            .get_name();
        if resource_name.starts_with(DEFAULT_RESOURCE_PREFIX) {
            return Err(Error::new(EngineError::RemoveDefaultResource(
                resource_name.clone(),
            )))
            .context(error_message.to_string());
        }

        // Remove and destroy resource
        let mut remove_result = self
            .resource_manager
            .remove_resource::<T>(resource_handle)
            .context(error_message.to_string())?;
        remove_result.1.destroy(self, *resource_handle)?;

        Ok(())
    }

    // Removes resource specified with its name from the engine
    pub fn remove_resource_by_name<T>(&mut self, name: &str) -> Result<()>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        let error_message = format!(
            "Removing {} {} {} failed",
            "Resource".gobj_style(),
            get_type_name::<T>().sobj_style(),
            name.to_string().name_style()
        );

        // Check if resource exists
        self.resource_manager
            .get_resource_by_name::<T>(name)
            .context(error_message.to_string())?;

        // Check if resource is not default
        if name.starts_with(DEFAULT_RESOURCE_PREFIX) {
            return Err(Error::new(EngineError::RemoveDefaultResource(
                name.to_string(),
            )))
            .context(error_message.to_string());
        }

        // Remove resource
        let mut remove_result = self
            .resource_manager
            .remove_resource_by_name::<T>(name)
            .context(error_message.to_string())?;
        remove_result.1.destroy(self, remove_result.0)?;

        Ok(())
    }
}
