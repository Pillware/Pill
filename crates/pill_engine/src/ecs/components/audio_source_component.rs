use crate::{
    engine::Engine,
    ecs::{ Component, ComponentStorage, GlobalComponentStorage, SceneHandle, DeferredUpdateManagerPointer, DeferredUpdateComponentRequest, DeferredUpdateComponent,},
    internal::{EntityHandle,},
    resources::{Sound, SoundHandle, SoundType}, 
    game::AudioManagerComponent
};

use pill_core::{PillTypeMapKey, get_type_name, PillStyle};

use cgmath::Vector3;
use anyhow::{ Result, Context, Error };

const DEFAULT_SOUND_SOURCE_POSITION : [f32; 3] = [0.0, 0.0, 0.0];

const DEFERRED_REQUEST_VARIANT_ADD_SOUND: usize = 0;
const DEFERRED_REQUEST_VARIANT_PLAY_SOUND: usize = 1;
const DEFERRED_REQUEST_VARIANT_PAUSE_SOUND: usize = 2;
const DEFERRED_REQUEST_VARIANT_CHANGE_SOURCE_POSITION: usize = 3;
const DEFERRED_REQUEST_VARIANT_GET_IS_SOUND_QUEUE_EMPTY: usize = 4;
const DEFERRED_REQUEST_VARIANT_SET_VOLUME: usize = 5;

pub struct AudioSourceComponent {

    sink_index: Option<usize>,
    sound_type: SoundType,

    source_position: [f32; 3],
    sound_volume: f32,
    is_song_queue_empty: bool,

    sound_handle: Option<SoundHandle>,
    entity_handle: Option<EntityHandle>,
    scene_handle: Option<SceneHandle>,
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
}

impl AudioSourceComponent {
    pub fn new(source_position: [f32; 3]) -> Self {

        Self {
            source_position,
            sound_volume: 1.0,
            sink_index: None,
            is_song_queue_empty: false,
            sound_type: SoundType::Sound3D,
            sound_handle: None,
            entity_handle: None,
            scene_handle: None,
            deferred_update_manager: None,
        }
    }

    pub fn as_spatial() -> Self {
        Self {
            source_position: DEFAULT_SOUND_SOURCE_POSITION.clone(),
            sound_volume: 1.0,
            sink_index: None,
            is_song_queue_empty: false,
            sound_type: SoundType::Sound3D,
            sound_handle: None,
            entity_handle: None,
            scene_handle: None,
            deferred_update_manager: None,
        }
    }

    pub fn as_ambient() -> Self {
        Self {
            source_position: DEFAULT_SOUND_SOURCE_POSITION.clone(),
            sound_volume: 1.0,
            sink_index: None,
            is_song_queue_empty: false,
            sound_type: SoundType::Sound2D,
            sound_handle: None,
            entity_handle: None,
            scene_handle: None,
            deferred_update_manager: None,
        }
    }
    
    // --- Getters

    // Give the handle (index) back to the AudioManagerComponent
    pub(crate) fn get_back_sink_handle(&mut self) -> Option<usize> {
        self.sink_index.take()
    }

    // Set the emitter position for the sink
    pub(crate) fn set_source_position(&mut self, new_position: Vector3<f32>) {
        let mut new_source_position = [0.0; 3];

        new_source_position[0] = new_position[0];
        new_source_position[1] = new_position[1];
        new_source_position[2] = new_position[2];

        self.source_position = new_source_position;

        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_CHANGE_SOURCE_POSITION);
    }

    // Get the information whether the sink is empty or not
    pub fn get_is_sound_queue_empty(&mut self) -> bool {
        if self.sink_index.is_none() {
            return true
        }
        else {
            self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_GET_IS_SOUND_QUEUE_EMPTY);
            return self.is_song_queue_empty
        }
    }

    // --- Setters

    // Set the source as spatial - possible only when there is no song playing
    pub fn set_as_spatial(&mut self) {
        if self.get_is_sound_queue_empty() {
            self.sound_type = SoundType::Sound3D;
        }
    }
    
    // Set the source as ambient - possible only when there is no song playing
    pub fn set_as_ambient(&mut self) {
        if self.get_is_sound_queue_empty() {
            self.sound_type = SoundType::Sound2D;
        }
    }

    // Set the volume of the played song
    pub fn set_sound_volume(&mut self, sound_volume: f32) {
        self.sound_volume = sound_volume;
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_SET_VOLUME);
    }

    // --- Other functionalities

    // Play the sound
    pub fn play_sound(&mut self) {
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PLAY_SOUND);
    }

    // Pause the source
    pub fn pause_sound(&mut self) {
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PAUSE_SOUND);
    }

    // Check if source is spatial
    pub fn is_spatial(&self) -> bool {
        self.sound_type == SoundType::Sound3D
    }

    // Check if source is ambient
    pub fn is_ambient(&self) -> bool {
        self.sound_type == SoundType::Sound2D
    }

    // Append new sound to the sound sink
    pub fn add_new_sound(&mut self, sound_handle: SoundHandle)
    {
        self.sound_handle = Some(sound_handle);
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_ADD_SOUND);
    }

    // Check if the source has the handle (index) to the possibly assigned sink
    pub fn has_sink_handle(&self) -> bool {
        self.sink_index.is_some()
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

impl Default for AudioSourceComponent {
    fn default() -> Self {
        
        let source_position = DEFAULT_SOUND_SOURCE_POSITION.clone();

        Self {
            source_position,
            sound_volume: 1.0,
            is_song_queue_empty: false,
            sink_index: None,
            sound_type: SoundType::Sound3D,
            sound_handle: None,
            entity_handle: None,
            scene_handle: None,
            deferred_update_manager: None,   
        }
    }
}

impl PillTypeMapKey for AudioSourceComponent {
    type Storage = ComponentStorage<AudioSourceComponent>; 
}

unsafe impl Send for AudioSourceComponent { }

impl Component for AudioSourceComponent { 
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // This resource is using DeferredUpdateSystem so keep DeferredUpdateManager
        let deferred_update_component = engine.get_global_component_mut::<DeferredUpdateComponent>().expect("Critical: No DeferredUpdateComponent");
        self.deferred_update_manager = Some(deferred_update_component.borrow_deferred_update_manager());

        Ok(())
    }

    fn pass_handles(&mut self, entity_handle: EntityHandle, scene_handle: SceneHandle) {
        self.entity_handle = Some(entity_handle);
        self.scene_handle = Some(scene_handle);
    }

    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> { 
        match request {
            DEFERRED_REQUEST_VARIANT_ADD_SOUND => 
            {   
                if self.sink_index.is_none() {
                    let audio_manager = engine.get_global_component_mut::<AudioManagerComponent>()?;
                    let sink_handle = match self.sound_type {
                        SoundType::Sound2D => audio_manager.get_ambient_sink_handle(),
                        SoundType::Sound3D => audio_manager.get_spatial_sink_handle()
                    };
                    if sink_handle.is_some() {
                        self.sink_index = sink_handle;
                    }
                }
                if self.sink_index.is_some() && self.sound_handle.is_some() {
                    let sound_handle = self.sound_handle.unwrap().clone();
                    let sound = (&*engine).get_resource::<Sound>(&sound_handle)?.clone();
                    let audio_manager = engine.get_global_component::<AudioManagerComponent>()?;
                    match self.sound_type {
                        SoundType::Sound3D => audio_manager.get_spatial_sink(self.sink_index.unwrap()).append(sound.sound_data.as_ref().unwrap().get_source_sound()),
                        SoundType::Sound2D => audio_manager.get_ambient_sink(self.sink_index.unwrap()).append(sound.sound_data.as_ref().unwrap().get_source_sound())
                    }
                    
                }
            },
            DEFERRED_REQUEST_VARIANT_CHANGE_SOURCE_POSITION => {
                if self.sink_index.is_some() && self.sound_type == SoundType::Sound3D {
                    let audio_manager = (&*engine).get_global_component::<AudioManagerComponent>()?;
                    audio_manager.get_spatial_sink(self.sink_index.unwrap()).set_emitter_position(self.source_position);
                }
            },
            DEFERRED_REQUEST_VARIANT_SET_VOLUME => {
                if self.sink_index.is_some() {
                    let audio_manager = (&*engine).get_global_component::<AudioManagerComponent>()?;
                    match self.sound_type {
                        SoundType::Sound3D => audio_manager.get_spatial_sink(self.sink_index.unwrap()).set_volume(self.sound_volume),
                        SoundType::Sound2D => audio_manager.get_ambient_sink(self.sink_index.unwrap()).set_volume(self.sound_volume),
                    } 
                }
            },
            DEFERRED_REQUEST_VARIANT_GET_IS_SOUND_QUEUE_EMPTY => {
                if self.sink_index.is_some() {
                    let audio_manager = (&*engine).get_global_component::<AudioManagerComponent>()?;
                    match self.sound_type {
                        SoundType::Sound3D => { self.is_song_queue_empty = audio_manager.get_spatial_sink(self.sink_index.unwrap()).empty(); },
                        SoundType::Sound2D => { self.is_song_queue_empty = audio_manager.get_ambient_sink(self.sink_index.unwrap()).empty(); }
                    }
                }
            },
            DEFERRED_REQUEST_VARIANT_PLAY_SOUND => {
                if self.sink_index.is_some() {
                    let audio_manager = (&*engine).get_global_component::<AudioManagerComponent>()?;
                    match self.sound_type {
                        SoundType::Sound3D => audio_manager.get_spatial_sink(self.sink_index.unwrap()).play(),
                        SoundType::Sound2D => audio_manager.get_ambient_sink(self.sink_index.unwrap()).play(),
                    } 
                }
            },
            DEFERRED_REQUEST_VARIANT_PAUSE_SOUND => {
                if self.sink_index.is_some() {
                    let audio_manager = (&*engine).get_global_component::<AudioManagerComponent>()?;
                    match self.sound_type {
                        SoundType::Sound3D => audio_manager.get_spatial_sink(self.sink_index.unwrap()).pause(),
                        SoundType::Sound2D => audio_manager.get_ambient_sink(self.sink_index.unwrap()).pause(),
                    } 
                }
            },
            _ => 
            {
                panic!("Critical: Processing deferred update request with value {} in {} failed. Handling is not implemented", request, get_type_name::<Self>().sobj_style());
            }
        }

        Ok(()) 
    }
}