use crate::{
    ecs::{ GlobalComponent, ComponentStorage, GlobalComponentStorage }, 
};

use pill_core::PillTypeMapKey;

use std::{ 
    any::Any,
    cell::RefCell,
    collections::HashMap,
};

pub struct WorldAudioComponent {

    audio_stream: rodio::OutputStream,
    audio_stream_handle: rodio::OutputStreamHandle,
    audio_sink: rodio::Sink
}

impl WorldAudioComponent {

    pub fn new() -> Self {

        let (audio_stream, audio_stream_handle) = rodio::OutputStream::try_default().unwrap();
        let audio_sink = rodio::Sink::try_new(&audio_stream_handle).unwrap();
       
        Self {
            audio_stream,
            audio_stream_handle,
            audio_sink 
        }
    }

    pub fn add_new_sound<S>(&self, sound: S)
    where
        S: rodio::Source + Send + 'static,
        S::Item: rodio::Sample,
        S::Item: Send,
    {
        self.audio_sink.append(sound);
    }

    pub fn get_sound_volume(&self) -> f32 {
        self.audio_sink.volume()
    }

    pub fn set_sound_volume(&self, new_volume: f32) {
        self.audio_sink.set_volume(new_volume);
    }

    pub fn toggle_sound(&self) {
        if self.audio_sink.is_paused() {
            self.audio_sink.play();
        }
        else {
            self.audio_sink.pause();
        }
    }

    pub fn set_sound_sink_empty(&self) {
        self.audio_sink.stop();
    }

    pub fn set_sound_paused(&self) {
        self.audio_sink.pause();
    }

    pub fn set_sound_playing(&self) {
        self.audio_sink.play();
    }
    
}

impl Default for WorldAudioComponent {

    fn default() -> Self {
        
        let (audio_stream, audio_stream_handle) = rodio::OutputStream::try_default().unwrap();
        let audio_sink = rodio::Sink::try_new(&audio_stream_handle).unwrap();
       
        Self {
            audio_stream,
            audio_stream_handle,
            audio_sink 
        }
    }
}

impl PillTypeMapKey for WorldAudioComponent {
    type Storage = GlobalComponentStorage<WorldAudioComponent>; 
}

unsafe impl Send for WorldAudioComponent { }

impl GlobalComponent for WorldAudioComponent { }

