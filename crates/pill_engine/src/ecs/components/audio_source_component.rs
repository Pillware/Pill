use crate::ecs::{ Component, ComponentStorage, GlobalComponentStorage };

use cgmath::Vector3;
use pill_core::PillTypeMapKey;

const DEFAULT_LEFT_EAR_POSITION : [f32; 3] = [-1.0, 0.0, 0.0];
const DEFAULT_RIGHT_EAR_POSITION : [f32; 3] = [1.0, 0.0, 0.0];
const DEFAULT_SOUND_SOURCE_POSITION : [f32; 3] = [0.0, 0.0, 0.0];

pub struct AudioSourceComponent {

    source_position: [f32; 3],
    audio_stream: rodio::OutputStream,
    audio_stream_handle: rodio::OutputStreamHandle,
    audio_spatial_sink: rodio::SpatialSink
}

impl AudioSourceComponent {
    pub fn new(source_position: [f32; 3], left_ear: Option<[f32; 3]>, right_ear: Option<[f32; 3]>) -> Self {

        let (audio_stream, audio_stream_handle) = rodio::OutputStream::try_default().unwrap();

        let left_ear_position = match left_ear {
            Some(position) => position,
            None => DEFAULT_LEFT_EAR_POSITION.clone()
        };

        let right_ear_position = match right_ear {
            Some(position) => position,
            None => DEFAULT_RIGHT_EAR_POSITION.clone()
        };

        let audio_spatial_sink = rodio::SpatialSink::try_new(&audio_stream_handle, source_position.clone(), left_ear_position, right_ear_position).unwrap();

        Self {
            source_position,
            audio_stream,
            audio_stream_handle, 
            audio_spatial_sink
        }
    }

    pub fn get_source_position(&self) -> [f32; 3] {
        self.source_position.clone()
    }

    pub fn set_source_position(&self, new_position: Vector3<f32>) {
        let mut new_source_position = [0.0; 3];

        new_source_position[0] = new_position[0];
        new_source_position[1] = new_position[1];
        new_source_position[2] = new_position[2];
        self.audio_spatial_sink.set_emitter_position(new_source_position);
    }

    pub fn set_listener_left_ear_position(&self, new_position: [f32; 3]) {
        self.audio_spatial_sink.set_left_ear_position(new_position);
    }

    pub fn set_listener_right_ear_position(&self, new_position: [f32; 3]) {
        self.audio_spatial_sink.set_right_ear_position(new_position);
    }

    pub fn get_source_volume(&self) -> f32 {
        self.audio_spatial_sink.volume()
    }

    pub fn set_source_volume(&self, new_volume: f32) {
        self.audio_spatial_sink.set_volume(new_volume);
    }

    pub fn add_new_sound<S>(&self, sound: S)
    where
        S: rodio::Source + Send + 'static,
        S::Item: rodio::Sample + Send + std::fmt::Debug,
    {
        self.audio_spatial_sink.append(sound);
    }

    pub fn toggle_sound(&self) {
        if self.audio_spatial_sink.is_paused() {
            self.audio_spatial_sink.play();
        }
        else {
            self.audio_spatial_sink.pause();
        }
    }

    pub fn set_sound_sink_empty(&self) {
        self.audio_spatial_sink.stop();
    }

    pub fn set_sound_paused(&self) {
        self.audio_spatial_sink.pause();
    }

    pub fn set_sound_playing(&self) {
        self.audio_spatial_sink.play();
    }
 
}

impl Default for AudioSourceComponent {

    fn default() -> Self {
        
        let source_position = DEFAULT_SOUND_SOURCE_POSITION.clone();
        let (audio_stream, audio_stream_handle) = rodio::OutputStream::try_default().unwrap();
        let audio_spatial_sink = rodio::SpatialSink::try_new(&audio_stream_handle, DEFAULT_SOUND_SOURCE_POSITION.clone(), DEFAULT_LEFT_EAR_POSITION.clone(), DEFAULT_RIGHT_EAR_POSITION.clone()).unwrap();

        Self {
            source_position,
            audio_stream,
            audio_stream_handle, 
            audio_spatial_sink
        }
    }
}

impl PillTypeMapKey for AudioSourceComponent {
    type Storage = ComponentStorage<AudioSourceComponent>; 
}

unsafe impl Send for AudioSourceComponent { }

impl Component for AudioSourceComponent { }