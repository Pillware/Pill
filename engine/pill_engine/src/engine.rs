use crate::{app_config::EngineConfig, config::*, ecs::*, graphics::*, resources::*};

use pill_core::{
    debug, error, get_game_error_message, get_type_name, info, EngineError, LogContext,
    PillSlotMapKey, PillStyle, PillTypeMap, Timer, Vector2f,
};

use pill_core::{ErrorContext, Result};
use std::{any::TypeId, collections::VecDeque};
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
    pub(crate) config: EngineConfig,
    pub(crate) game: Option<Game>,
    pub(crate) renderer: Box<dyn PillRenderer>,
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

// ---- INTERNAL -----------------------------------------------------------------

/// Pill Engine internal API
#[cfg(feature = "internal")]
impl Engine {
    #[cfg(not(feature = "headless"))]
    pub fn new(
        game: Box<dyn PillGame>,
        game_resources_directory_path: std::path::PathBuf,
        renderer: Box<dyn PillRenderer>,
        config: EngineConfig,
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
            frame_delta_time: 0.0,
        }
    }

    #[cfg(feature = "headless")]
    pub fn new(game: Box<dyn PillGame>, config: EngineConfig) -> Self {
        let max_entity_count = config
            .get_int("MAX_ENTITIES")
            .unwrap_or(MAX_ENTITIES as i64) as usize;
        let dummy_renderer = Box::new(DummyRenderer) as Box<dyn PillRenderer>;

        Self {
            config,
            game: Some(game),
            renderer: dummy_renderer,
            scene_manager: SceneManager::new(max_entity_count),
            system_manager: SystemManager::new(),
            resource_manager: ResourceManager::new(),
            global_components: PillTypeMap::new(),
            input_queue: VecDeque::new(),
            render_queue: Vec::<RenderQueueItem>::with_capacity(max_entity_count),
            window_size: winit::dpi::PhysicalSize::<u32>::default(),
            game_resources_directory_path: std::path::PathBuf::new(),
            frame_delta_time: 0.0.into(),
        }
    }

    fn create_default_resources(&mut self) -> Result<()> {
        // Register resources and their limits

        use pill_core::Color;
        let max_shader_count = self
            .config
            .get_int("MAX_SHADERS")
            .unwrap_or(MAX_SHADERS as i64) as usize;
        let max_material_count = self
            .config
            .get_int("MAX_MATERIALS")
            .unwrap_or(MAX_MATERIALS as i64) as usize;
        let max_texture_count = self
            .config
            .get_int("MAX_TEXTURES")
            .unwrap_or(MAX_TEXTURES as i64) as usize;
        let max_mesh_count = self
            .config
            .get_int("MAX_MESHES")
            .unwrap_or(MAX_MESHES as i64) as usize;
        #[cfg(not(target_arch = "wasm32"))]
        let max_sound_count = self
            .config
            .get_int("MAX_SOUNDS")
            .unwrap_or(MAX_SOUNDS as i64) as usize;

        self.register_resource_type::<Shader>(max_shader_count)?;
        self.register_resource_type::<Material>(max_material_count)?;
        self.register_resource_type::<Texture>(max_texture_count)?;
        self.register_resource_type::<Mesh>(max_mesh_count)?;
        #[cfg(not(target_arch = "wasm32"))]
        self.register_resource_type::<Sound>(max_sound_count)?;

        debug!(LogContext::Engine => "Resource types registered");

        debug!(LogContext::Engine => "Creating default shader {}...", DEFAULT_LIT_SHADER_NAME.name_style());

        // Create default resources
        // Load default lit shader data to executable
        let default_lit_shader_handle = self.add_default_resource(Shader::new(
            DEFAULT_LIT_SHADER_NAME,
            ResourceLoader::Bytes(Box::new(*include_bytes!(
                "../res/shaders/default_vertex.wgsl"
            ))),
            ResourceLoader::Bytes(Box::new(*include_bytes!(
                "../res/shaders/default_lit_fragment.wgsl"
            ))),
            vec![
                (
                    DEFAULT_LIT_SHADER_TINT_PARAMETER_SLOT_NAME.to_string(),
                    ShaderParameterSlot::new(ShaderParameterType::Color),
                ),
                (
                    DEFAULT_LIT_SHADER_SPECULARITY_PARAMETER_SLOT_NAME.to_string(),
                    ShaderParameterSlot::new(ShaderParameterType::Scalar),
                ),
            ]
            .into_iter()
            .collect(),
            vec![
                (
                    DEFAULT_LIT_SHADER_COLOR_TEXTURE_SLOT_NAME.to_string(),
                    ShaderTextureSlot::new(
                        TextureType::Color,
                        DEFAULT_LIT_SHADER_COLOR_TEXTURE_SLOT_BINDINGS,
                    ),
                ),
                (
                    DEFAULT_LIT_SHADER_NORMAL_TEXTURE_SLOT_NAME.to_string(),
                    ShaderTextureSlot::new(
                        TextureType::Normal,
                        DEFAULT_LIT_SHADER_NORMAL_TEXTURE_SLOT_BINDINGS,
                    ),
                ),
            ]
            .into_iter()
            .collect(),
            true,
            true,
        ))?;

        let default_unlit_shader_handle = self.add_default_resource(Shader::new(
            DEFAULT_UNLIT_SHADER_NAME,
            ResourceLoader::Bytes(Box::new(*include_bytes!(
                "../res/shaders/default_vertex.wgsl"
            ))),
            ResourceLoader::Bytes(Box::new(*include_bytes!(
                "../res/shaders/default_unlit_fragment.wgsl"
            ))),
            vec![(
                DEFAULT_UNLIT_SHADER_TINT_PARAMETER_SLOT_NAME.to_string(),
                ShaderParameterSlot::new(ShaderParameterType::Color),
            )]
            .into_iter()
            .collect(),
            vec![(
                DEFAULT_UNLIT_SHADER_COLOR_TEXTURE_SLOT_NAME.to_string(),
                ShaderTextureSlot::new(
                    TextureType::Color,
                    DEFAULT_UNLIT_SHADER_COLOR_TEXTURE_SLOT_BINDINGS,
                ),
            )]
            .into_iter()
            .collect(),
            true,
            true,
        ))?;

        debug!(LogContext::Engine => "Default unlit shader {} created", DEFAULT_UNLIT_SHADER_NAME.name_style());
        debug!(LogContext::Engine => "Creating default color texture {}...", DEFAULT_COLOR_TEXTURE_NAME.name_style());

        // Create texture data to executable
        let default_color_texture_handle = self.add_default_resource(Texture::new(
            DEFAULT_COLOR_TEXTURE_NAME,
            TextureType::Color,
            ResourceLoader::Bytes(Box::new(DEFAULT_COLOR_TEXTURE_BYTES)),
        ))?;

        debug!(LogContext::Engine => "Default color texture {} created", DEFAULT_COLOR_TEXTURE_NAME.name_style());
        debug!(LogContext::Engine => "Creating default normal texture {}...", DEFAULT_NORMAL_TEXTURE_NAME.name_style());

        let default_normal_texture_handle = self.add_default_resource(Texture::new(
            DEFAULT_NORMAL_TEXTURE_NAME,
            TextureType::Normal,
            ResourceLoader::Bytes(Box::new(DEFAULT_NORMAL_TEXTURE_BYTES)),
        ))?;

        debug!(LogContext::Engine => "Default normal texture {} created", DEFAULT_NORMAL_TEXTURE_NAME.name_style());
        debug!(LogContext::Engine => "Creating default material {}...", DEFAULT_LIT_MATERIAL_NAME.name_style());

        // Create default lit material
        self.add_default_resource(
            Material::builder(DEFAULT_LIT_MATERIAL_NAME)
                .shader(default_lit_shader_handle)?
                .color_parameter(
                    DEFAULT_LIT_SHADER_TINT_PARAMETER_SLOT_NAME,
                    Color::new(1.0, 1.0, 1.0),
                )?
                .scalar_parameter(DEFAULT_LIT_SHADER_SPECULARITY_PARAMETER_SLOT_NAME, 0.5)?
                .texture(
                    DEFAULT_LIT_SHADER_COLOR_TEXTURE_SLOT_NAME,
                    default_color_texture_handle,
                )?
                .texture(
                    DEFAULT_LIT_SHADER_NORMAL_TEXTURE_SLOT_NAME,
                    default_normal_texture_handle,
                )?
                .build(),
        )?;

        debug!(LogContext::Engine => "Default lit material {} created", DEFAULT_LIT_MATERIAL_NAME.name_style());
        debug!(LogContext::Engine => "Creating default material {}...", DEFAULT_UNLIT_MATERIAL_NAME.name_style());

        // Create default unlit material
        self.add_default_resource(
            Material::builder(DEFAULT_UNLIT_MATERIAL_NAME)
                .shader(default_unlit_shader_handle)?
                .color_parameter(
                    DEFAULT_LIT_SHADER_TINT_PARAMETER_SLOT_NAME,
                    Color::new(1.0, 1.0, 1.0),
                )?
                .texture(
                    DEFAULT_LIT_SHADER_COLOR_TEXTURE_SLOT_NAME,
                    default_color_texture_handle,
                )?
                .build(),
        )?;

        debug!(LogContext::Engine => "Default unlit material {} created", DEFAULT_UNLIT_MATERIAL_NAME.name_style());

        Ok(())
    }

    fn start_game(&mut self) -> Result<()> {
        info!(LogContext::Engine => "Starting {}", "Game".module_object_style());

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
            true => result.context(format!("{} error", "Game".module_object_style()))?,
            false => {
                if let Some(message) = get_game_error_message(result) {
                    error!(LogContext::Engine => "{}", message);
                }
            }
        }
        self.game = Some(game);
        Ok(())
    }

    /// Initializes Pill Engine
    ///
    /// Creates default global components, adds default systems, creates default resources, initializes game
    pub fn initialize(&mut self, window_size: Option<winit::dpi::PhysicalSize<u32>>) -> Result<()> {
        info!(LogContext::Engine => "Initializing {}", "Engine".module_object_style());

        // Set window size
        self.window_size = window_size.unwrap_or(winit::dpi::PhysicalSize::<u32>::new(800, 600));

        // Register global components
        self.add_global_component(TimeComponent::new())?;
        self.add_global_component(DeferredUpdateComponent::new())?;
        #[cfg(feature = "debug_ui")]
        self.add_global_component(EguiManagerComponent::new())?;

        #[cfg(not(feature = "headless"))]
        {
            self.add_global_component(InputComponent::new())?;
        }

        #[cfg(all(not(feature = "headless"), not(target_arch = "wasm32")))]
        {
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
        }

        // Add built-in systems
        self.system_manager.add_system(
            TIME_SYSTEM.name,
            TIME_SYSTEM.system_function,
            TIME_SYSTEM.update_phase,
        )?;
        self.system_manager.add_system(
            DEFERRED_UPDATE_SYSTEM.name,
            DEFERRED_UPDATE_SYSTEM.system_function,
            DEFERRED_UPDATE_SYSTEM.update_phase,
        )?;

        #[cfg(not(feature = "headless"))]
        {
            self.system_manager.add_system(
                INPUT_SYSTEM.name,
                INPUT_SYSTEM.system_function,
                INPUT_SYSTEM.update_phase,
            )?;
            #[cfg(not(target_arch = "wasm32"))]
            self.system_manager.add_system(
                AUDIO_SYSTEM.name,
                AUDIO_SYSTEM.system_function,
                AUDIO_SYSTEM.update_phase,
            )?;
            self.system_manager.add_system(
                RENDERING_SYSTEM.name,
                RENDERING_SYSTEM.system_function,
                RENDERING_SYSTEM.update_phase,
            )?;
        }

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
            let (_, phase_systems) = &self.system_manager.update_phases[update_phase_index];
            let phase_len = phase_systems.len();
            for system_index in 0..phase_len {
                let (system_name, update_phase, system_function);
                {
                    let (_, system) =
                        &mut self.system_manager.update_phases[update_phase_index].1[system_index];
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
                    timer.begin_context(format!("{} system update", system_name));
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
                            EngineError::NonReturnedSystemTimer(system_name.clone())
                        );
                    }
                    Err(e) => {
                        panic!("{}", EngineError::Other(e.to_string()));
                    }
                };

                if system_name != RENDERING_SYSTEM.name {
                    timer
                        .end_context()
                        .context(format!(
                            "Failed to end timer context for {}",
                            system_name.clone()
                        ))
                        .unwrap(); // End system update context
                }
                self.system_manager
                    .update_system_timer(system_name.as_str(), update_phase.clone(), timer)
                    .unwrap();
            }
        }

        // Update FPS counter
        let new_frame_time = delta_time.as_secs_f32() * 1000.0;
        let fps = 1000.0 / new_frame_time;
        self.frame_delta_time = new_frame_time;
        debug!(
            "Frame finished (Time: {:.3}ms, FPS {:.0})",
            new_frame_time, fps
        );
    }

    pub fn shutdown(&mut self) {
        // TODO: pass the memory to serialize to
        info!(LogContext::Engine => "Shutting down {}", "Engine".module_object_style());
        // TODO: can we serialize to some memory here?
    }

    pub fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        debug!(LogContext::Engine => "{} resized to {}x{}", "Window".module_object_style(), new_window_size.width, new_window_size.height);
        self.window_size = new_window_size;
        self.renderer.resize(new_window_size);
    }

    pub fn pass_keyboard_key_input(&mut self, keyboard_input: &KeyEvent) {
        let state: winit::event::ElementState = keyboard_input.state;
        match keyboard_input.physical_key {
            winit::keyboard::PhysicalKey::Code(key_code) => {
                let input_event = InputEvent::Keyboard(KeyboardEvent::Key {
                    key: key_code,
                    state,
                });
                self.input_queue.push_back(input_event);
                debug!(LogContext::Input => "Got new keyboard key input: {:?} {:?}", key_code, state);
            }
            winit::keyboard::PhysicalKey::Unidentified(_) => {
                debug!(LogContext::Input => "Unidentified key input: {:?}", keyboard_input.physical_key);
            }
        }
    }

    pub fn pass_mouse_key_input(&mut self, key: &MouseButton, state: &winit::event::ElementState) {
        let input_event = InputEvent::Mouse(MouseEvent::Button {
            key: *key,
            state: *state,
        });
        self.input_queue.push_back(input_event);
        debug!(LogContext::Input => "Got new mouse key input");
    }

    pub fn pass_mouse_wheel_input(&mut self, delta: &winit::event::MouseScrollDelta) {
        let input_event = InputEvent::Mouse(MouseEvent::Wheel { delta: *delta });
        self.input_queue.push_back(input_event);
        debug!(LogContext::Input => "Got new mouse wheel input");
    }

    pub fn pass_mouse_delta_input(&mut self, delta: &(f64, f64)) {
        let input_event = InputEvent::Mouse(MouseEvent::Delta {
            delta: Vector2f::new(delta.0 as f32, delta.1 as f32),
        });
        self.input_queue.push_back(input_event);
        debug!(LogContext::Input => "Got new mouse motion input");
    }

    pub fn pass_mouse_position_input(&mut self, position: &PhysicalPosition<f64>) {
        let input_event = InputEvent::Mouse(MouseEvent::Position {
            position: Vector2f::new(position.x as f32, position.y as f32),
        });
        self.input_queue.push_back(input_event);
        debug!(LogContext::Input => "Got new mouse position input");
    }

    pub fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) {
        #[cfg(feature = "debug_ui")]
        self.renderer.pass_input_to_egui(event).unwrap();
        #[cfg(not(feature = "debug_ui"))]
        let _ = event;
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
            "System".general_object_style(),
            name.name_style(),
            "UpdatePhase".specific_object_style(),
            "Game".name_style()
        );

        self.system_manager
            .add_system(name, system_function, UpdatePhase::Game)
            .context(format!("Adding {} failed", "System".general_object_style()))
    }

    /// Removes game-defined system
    pub fn remove_system(&mut self, name: &str) -> Result<()> {
        debug!(
            "Removing {} {} from {} {}",
            "System".general_object_style(),
            name.name_style(),
            "UpdatePhase".specific_object_style(),
            "Game".name_style()
        );

        self.system_manager
            .remove_system(name, UpdatePhase::Game)
            .context(format!(
                "Removing {} failed",
                "System".general_object_style()
            ))
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
            "System".general_object_style(),
            name.name_style(),
            "UpdatePhase".specific_object_style(),
            update_phase.to_string().name_style(),
            if enabled { "Enabled" } else { "Disabled" }
        );

        self.system_manager
            .toggle_system(name, update_phase, enabled)
            .context(format!(
                "Toggling {} failed",
                "System".general_object_style()
            ))
    }

    /// Returns system timer. It has to be returned back using update_system_timer function, otherwise engine will panic.
    pub fn get_system_timer(&mut self, name: &str) -> Timer {
        debug!(
            "Getting {} {} timer from {} {}",
            "System".general_object_style(),
            name.name_style(),
            "UpdatePhase".specific_object_style(),
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
            "Entity".general_object_style(),
            "Scene".general_object_style(),
            self.scene_manager
                .get_scene(scene_handle)
                .unwrap()
                .name
                .name_style()
        );

        self.scene_manager
            .create_entity(scene_handle)
            .context(format!(
                "Creating {} failed",
                "Entity".general_object_style()
            ))
    }

    // Removes entity specified with entity handle from scene specified with scene handle
    pub fn remove_entity(
        &mut self,
        entity_handle: EntityHandle,
        scene_handle: SceneHandle,
    ) -> Result<()> {
        debug!(
            "Removing {} from {} {}",
            "Entity".general_object_style(),
            "Scene".general_object_style(),
            self.scene_manager
                .get_scene(scene_handle)
                .unwrap()
                .name
                .name_style()
        );

        let component_destroyers = self
            .scene_manager
            .remove_entity(scene_handle, entity_handle)
            .context(format!(
                "Creating {} failed",
                "Entity".general_object_style()
            ))?;

        // Destroy components using destroyers
        for mut component_destroyer in component_destroyers {
            component_destroyer.destroy(self, scene_handle, entity_handle)?;
        }

        Ok(())
    }

    // Removes entity specified with entity handle from its scene
    pub fn remove_entity_default_scene(&mut self, entity_handle: EntityHandle) -> Result<()> {
        let scene_handle = self.scene_manager.get_active_scene_handle()?;
        debug!(
            "Removing {} from {} {}",
            "Entity".general_object_style(),
            "Scene".general_object_style(),
            self.scene_manager
                .get_scene(scene_handle)
                .unwrap()
                .name
                .name_style()
        );

        let component_destroyers = self
            .scene_manager
            .remove_entity(scene_handle, entity_handle)
            .context(format!(
                "Creating {} failed",
                "Entity".general_object_style()
            ))?;

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
        debug!(LogContext::ECS => "Registering {} {} in {} {}", "Component".general_object_style(), get_type_name::<T>().specific_object_style(), "Scene".specific_object_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        self.scene_manager
            .register_component::<T>(scene_handle)
            .context(format!(
                "Registering {} failed",
                "Component".general_object_style()
            ))
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
        debug!(LogContext::ECS => "Adding {} {} to {} {} in {} {}", "Component".general_object_style(), get_type_name::<T>().specific_object_style(), "Entity".general_object_style(), entity_handle.data().index, "Scene".general_object_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

        // Check if already added
        let target_scene = self.scene_manager.get_scene(scene_handle)?;

        if target_scene.entity_has_component::<T>(entity_handle)? {
            return Err(EngineError::ComponentAlreadyExists(get_type_name::<T>()).into());
        }

        // Initialize component
        component.initialize(self).context(format!(
            "Adding {} {} failed",
            "Component".general_object_style(),
            get_type_name::<T>().specific_object_style()
        ))?;

        // Add component
        self.scene_manager
            .add_component_to_entity::<T>(scene_handle, entity_handle, component)
            .context(format!(
                "Adding {} to {} failed",
                "Component".general_object_style(),
                "Entity".general_object_style()
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
        debug!(LogContext::ECS => "Removing {} {} from {} {} in {} {}", "Component".general_object_style(), get_type_name::<T>().specific_object_style(), "Entity".general_object_style(), entity_handle.data().index, "Scene".general_object_style(), self.scene_manager.get_scene(scene_handle).unwrap().name.name_style());

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
            return Err(EngineError::GlobalComponentAlreadyExists(get_type_name::<T>()).into());
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
            .ok_or_else(|| -> pill_core::PillError {
                EngineError::GlobalComponentNotFound(get_type_name::<T>()).into()
            })?
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
            .ok_or_else(|| -> pill_core::PillError {
                EngineError::GlobalComponentNotFound(get_type_name::<T>()).into()
            })?
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
            return Err(EngineError::GlobalComponentCannotBeRemoved(get_type_name::<T>()).into());
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
        info!(LogContext::ECS => "Creating scene: {}", name);
        self.scene_manager.create_scene(name).context(format!(
            "Creating new {} failed",
            "Scene".general_object_style()
        ))
    }

    /// Returns handle to the scene specified by its name
    pub fn get_scene_handle(&self, name: &str) -> Result<SceneHandle> {
        self.scene_manager.get_scene_handle(name).context(format!(
            "Getting {} failed",
            "SceneHandle".specific_object_style()
        ))
    }

    pub fn set_active_scene(&mut self, scene_handle: SceneHandle) -> Result<()> {
        self.scene_manager
            .set_active_scene(scene_handle)
            .context(format!(
                "Setting active {} failed",
                "Scene".general_object_style()
            ))
    }

    /// Returns handle to the active scene
    pub fn get_active_scene_handle(&self) -> Result<SceneHandle> {
        self.scene_manager
            .get_active_scene_handle()
            .context(format!(
                "Getting {} of active {} failed",
                "SceneHandle".specific_object_style(),
                "Scene".general_object_style()
            ))
    }

    // Removes scene deleting all data in it
    pub fn remove_scene(&mut self, scene_handle: SceneHandle) -> Result<()> {
        // Get scene
        let scene = self.scene_manager.get_scene(scene_handle)?;

        // Get entity handles
        let mut entity_handles = Vec::<EntityHandle>::new();
        for (entity_handle, _) in scene.entities.iter() {
            entity_handles.push(entity_handle);
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
                "Scene".specific_object_style(),
                "SceneHandle".general_object_style()
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
    pub fn add_resource<T>(&mut self, resource: T) -> Result<T::Handle>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        // TODO: Doesnt work when called from game
        info!(LogContext::Resources => "Adding {} {} {}", "Resource".general_object_style(), get_type_name::<T>().specific_object_style(), resource.get_name().name_style());

        self.add_resource_internal(resource, true)
    }

    // Allows to add resource without checking its name to be a valid one (not starting with DEFAULT_RESOURCE_PREFIX)
    fn add_default_resource<T>(&mut self, resource: T) -> Result<T::Handle>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        self.add_resource_internal(resource, false)
    }

    fn add_resource_internal<T>(
        &mut self,
        mut resource: T,
        enforce_name_check: bool,
    ) -> Result<T::Handle>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        debug!(LogContext::Resources => "Adding {} {} {}", "Resource".general_object_style(), get_type_name::<T>().specific_object_style(), resource.get_name().name_style());

        // Check if resource has proper name
        let resource_name = resource.get_name();
        if enforce_name_check && resource_name.starts_with(DEFAULT_RESOURCE_PREFIX) {
            return Err(EngineError::WrongResourceName(resource_name.clone()).into());
        }

        // Initialize resource
        debug!(LogContext::Resources => "Initializing {} {} {}", "Resource".general_object_style(), get_type_name::<T>().specific_object_style(), resource.get_name().name_style());
        resource.initialize(self).context(format!(
            "Adding {} {} failed",
            "Resource".general_object_style(),
            get_type_name::<T>().specific_object_style()
        ))?;

        // Add resource and get it back
        let (resource_handle, resource) = self.resource_manager.add_resource(resource)?;

        // Pass handle to this resource so it can store it if needed
        resource.pass_handle(resource_handle);

        Ok(resource_handle)
    }

    // Returns resource associated with resource handle
    pub fn get_resource<'a, T>(&'a self, resource_handle: &'a T::Handle) -> Result<&'a T>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        self.resource_manager.get_resource::<T>(resource_handle)
    }

    /// Returns resource specified by its name
    pub fn get_resource_by_name<T>(&self, name: &str) -> Result<&T>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        self.resource_manager.get_resource_by_name::<T>(name)
    }

    /// Returns handle to resource specified by the name of this resource
    pub fn get_resource_handle<T>(&self, name: &str) -> Result<T::Handle>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        self.resource_manager.get_resource_handle::<T>(name)
    }

    // Returns mutable resource associated with resource handle
    pub fn get_resource_mut<'a, T>(
        &'a mut self,
        resource_handle: &'a T::Handle,
    ) -> Result<&'a mut T>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        self.resource_manager.get_resource_mut::<T>(resource_handle)
    }

    /// Returns mutable resource specified by its name
    pub fn get_resource_by_name_mut<T>(&mut self, name: &str) -> Result<&mut T>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        self.resource_manager.get_resource_by_name_mut::<T>(name)
    }

    // Removes resource associated with resource handle from the engine
    pub fn remove_resource<T>(&mut self, resource_handle: &T::Handle) -> Result<()>
    where
        T: Resource<Storage = ResourceStorage<T>>,
    {
        let error_message = format!(
            "Removing {} {} failed",
            "Resource".general_object_style(),
            get_type_name::<T>().specific_object_style()
        );

        // Check if resource is not default
        let resource_name = self
            .resource_manager
            .get_resource::<T>(resource_handle)
            .context(error_message.to_string())?
            .get_name();
        if resource_name.starts_with(DEFAULT_RESOURCE_PREFIX) {
            return Err(EngineError::RemoveDefaultResource(resource_name.clone()))
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
            "Resource".general_object_style(),
            get_type_name::<T>().specific_object_style(),
            name.to_string().name_style()
        );

        // Check if resource exists
        self.resource_manager
            .get_resource_by_name::<T>(name)
            .context(error_message.to_string())?;

        // Check if resource is not default
        if name.starts_with(DEFAULT_RESOURCE_PREFIX) {
            return Err(EngineError::RemoveDefaultResource(name.to_string()))
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
