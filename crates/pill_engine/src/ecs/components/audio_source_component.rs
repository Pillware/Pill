use crate::{
    engine::Engine,
    resources::{ Sound, SoundHandle }, 
    ecs::{ 
        AudioManagerComponent, 
        EntityHandle, 
        Component, 
        ComponentStorage, 
        SceneHandle, 
        DeferredUpdateManagerPointer, 
        DeferredUpdateComponentRequest,
        DeferredUpdateComponent,
        SoundType
    },
};

use pill_core::{ PillTypeMapKey, get_type_name, PillStyle, Vector3f, get_enum_variant_type_name };

use log::warn;
use anyhow::{ Result, Context, Error };

const DEFERRED_REQUEST_VARIANT_SET_SOUND: usize = 0;
const DEFERRED_REQUEST_VARIANT_SET_VOLUME: usize = 1;
const DEFERRED_REQUEST_VARIANT_PLAY_SOUND: usize = 2;
const DEFERRED_REQUEST_VARIANT_PAUSE_SOUND: usize = 3;
const DEFERRED_REQUEST_VARIANT_STOP_SOUND: usize = 4;

// --- Builder ---

pub struct AudioSourceComponentBuilder {
    component: AudioSourceComponent,
}

impl AudioSourceComponentBuilder {
    pub fn default() -> Self {
        Self {
            component: AudioSourceComponent::new(),
        }
    }

    pub fn sound_type(mut self, sound_type: SoundType) -> Self {
        self.component.sound_type = sound_type;
        self
    }

    pub fn sound(mut self, sound_handle: SoundHandle) -> Self {
        self.component.sound_handle = Some(sound_handle);
        self
    }

    pub fn volume(mut self, volume: f32) -> Self {
        self.component.volume = volume;
        self
    }

    pub fn build(self) -> AudioSourceComponent {
        self.component
    }
}

// --- Audio Source Component ---

#[readonly::make]
pub struct AudioSourceComponent {
    #[readonly]
    pub sound_type: SoundType,
    #[readonly]
    pub volume: f32,
    #[readonly]
    pub sound_handle: Option<SoundHandle>,
    #[readonly]
    pub is_playing: bool,
    pub(crate) sink_handle: Option<usize>,

    entity_handle: Option<EntityHandle>,
    scene_handle: Option<SceneHandle>,
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
}

impl AudioSourceComponent {
    pub fn builder() -> AudioSourceComponentBuilder {
        AudioSourceComponentBuilder::default()
    }

    pub fn new() -> Self {
        Self {
            sound_type: SoundType::Sound3D,
            volume: 1.0,
            sound_handle: None,
            sink_handle: None,
            is_playing: false,

            entity_handle: None,
            scene_handle: None,
            deferred_update_manager: None,
        }
    }

    pub fn set_sound(&mut self, sound_handle: SoundHandle) {
        self.sound_handle = Some(sound_handle);
        if self.sink_handle.is_some() {
            self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_SET_VOLUME);
        }
    }

    pub fn remove_sound(&mut self) {
        self.sound_handle = None;
        if self.sink_handle.is_some() {
            self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_STOP_SOUND);
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        if self.sink_handle.is_some() {
            self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_SET_VOLUME);
        }
    }

    pub fn play(&mut self) {
        if self.sound_handle.is_some() {
            self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PLAY_SOUND);
        }
    }

    pub fn pause(&mut self) {
        if self.sink_handle.is_some() {
            self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PAUSE_SOUND);
        }
    }

    pub fn stop(&mut self) {
        if self.sink_handle.is_some() {
            self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_STOP_SOUND);
        }
    }

    // Give the handle (index) back to the AudioManagerComponent
    pub(crate) fn return_sink(&mut self) -> Option<usize> {
        self.is_playing = false;
        self.sink_handle.take()
    }

    // Stop playing and return sink to pool
    fn stop_playing(&mut self, engine: &mut Engine) -> Result<()> {
        let audio_manager = engine.get_global_component_mut::<AudioManagerComponent>()?;
        self.is_playing = false;
        match self.sound_type {
            SoundType::Sound3D => {
                audio_manager.get_spatial_sink(self.sink_handle.unwrap()).stop();
            },
            SoundType::Sound2D => {
                audio_manager.get_ambient_sink(self.sink_handle.unwrap()).stop();
            }
        }
        audio_manager.return_sink(self.sink_handle.take().unwrap(), &self.sound_type);

        Ok(())
    }

    // Post deferred update request
    fn post_deferred_update_request(&mut self, request_variant: usize) {
        if self.deferred_update_manager.is_some() {
            let entity_handle = self.entity_handle.expect("Critical: Cannot post deferred update request. No EntityHandle set in Component");
            let scene_handle = self.scene_handle.expect("Critical: Cannot post deferred update request. No SceneHandle set in Component");
            let request = DeferredUpdateComponentRequest::<AudioSourceComponent>::new(entity_handle, scene_handle, request_variant);
            self.deferred_update_manager.as_mut().expect("Critical: No DeferredUpdateManager").post_update_request(request);
        }
    }
}

impl PillTypeMapKey for AudioSourceComponent {
    type Storage = ComponentStorage<AudioSourceComponent>; 
}

impl Component for AudioSourceComponent { 
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // This component is using DeferredUpdateSystem so keep DeferredUpdateManager
        let deferred_update_component = engine.get_global_component_mut::<DeferredUpdateComponent>().expect("Critical: No DeferredUpdateComponent");
        self.deferred_update_manager = Some(deferred_update_component.borrow_deferred_update_manager());

        // Check if sound handle is valid
        if self.sound_handle.is_some() {
            engine.get_resource::<Sound>(&self.sound_handle.unwrap())
                .context(format!("Creating {} {} failed", "Component".gobj_style(), get_type_name::<Self>().sobj_style()))?;
        }

        Ok(())
    }

    fn pass_handles(&mut self, entity_handle: EntityHandle, scene_handle: SceneHandle) {
        self.entity_handle = Some(entity_handle);
        self.scene_handle = Some(scene_handle);
    }

    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> { 
        match request {
            DEFERRED_REQUEST_VARIANT_SET_SOUND => 
            {
                // Check if sound handle is valid  
                engine.get_resource::<Sound>(&self.sound_handle.unwrap())
                    .context(format!("Setting {} {} failed", "Component".gobj_style(), "Sound".sobj_style()))?;

                // Stop playing
                self.stop_playing(engine)?;
            },
            DEFERRED_REQUEST_VARIANT_SET_VOLUME => 
            {
                let audio_manager = (&*engine).get_global_component::<AudioManagerComponent>()?;
                match self.sound_type {
                    SoundType::Sound3D => audio_manager.get_spatial_sink(self.sink_handle.unwrap()).set_volume(self.volume),
                    SoundType::Sound2D => audio_manager.get_ambient_sink(self.sink_handle.unwrap()).set_volume(self.volume),
                } 
            },
            DEFERRED_REQUEST_VARIANT_PLAY_SOUND => 
            {
                // Get data from sound resource
                let sound_handle = self.sound_handle.unwrap();
                let sound = (&*engine).get_resource::<Sound>(&sound_handle)?;
                let sound_data = sound.sound_data.as_ref().unwrap().get_source_sound();

                // Get free sink, set its volume and play
                let audio_manager = engine.get_global_component_mut::<AudioManagerComponent>()?;
                if let Some(sink_handle) = audio_manager.get_free_sink_handle(&self.sound_type) {
                    self.sink_handle = Some(sink_handle);
                 
                    match self.sound_type {
                        SoundType::Sound2D => {
                            let sink = audio_manager.get_ambient_sink(sink_handle);
                            sink.append(sound_data);
                            sink.set_volume(self.volume);
                            sink.play();
                        },
                        SoundType::Sound3D => {
                            let sink = audio_manager.get_spatial_sink(sink_handle);
                            sink.append(sound_data);
                            sink.set_volume(self.volume);
                            sink.play();
                        }
                    }
                    self.is_playing = true;
                } 
                else {
                    warn!("Cannot play sound, max concurrent {} sound count reached", get_enum_variant_type_name(&self.sound_type));
                }
            },
            DEFERRED_REQUEST_VARIANT_PAUSE_SOUND  => 
            {
                let audio_manager = (&*engine).get_global_component::<AudioManagerComponent>()?;
                match self.sound_type {
                    SoundType::Sound3D => audio_manager.get_spatial_sink(self.sink_handle.unwrap()).pause(),
                    SoundType::Sound2D => audio_manager.get_ambient_sink(self.sink_handle.unwrap()).pause(),
                } 
            },
            DEFERRED_REQUEST_VARIANT_STOP_SOUND  => 
            {
                self.stop_playing(engine)?;
            },
            _ => 
            {
                panic!("Critical: Processing deferred update request with value {} in {} failed. Handling is not implemented", request, get_type_name::<Self>().sobj_style());
            }
        }

        Ok(()) 
    }

    fn destroy(&mut self, engine: &mut Engine, self_entity_handle: EntityHandle, self_scene_handle: SceneHandle) -> Result<()> {
        if self.sink_handle.is_some() {
            self.stop_playing(engine)?;
        }

        Ok(())
    }
}