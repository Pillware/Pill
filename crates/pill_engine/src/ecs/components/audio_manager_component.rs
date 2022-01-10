use crate::{
    ecs::{ GlobalComponent, ComponentStorage, GlobalComponentStorage }, 
};

use pill_core::PillTypeMapKey;

use std::{ 
    any::Any,
    cell::RefCell,
    collections::HashMap,
};

use rodio::{OutputStream, OutputStreamHandle, Sink, SpatialSink};

const DEFAULT_LEFT_EAR_POSITION : [f32; 3] = [-1.0, 0.0, 0.0];
const DEFAULT_RIGHT_EAR_POSITION : [f32; 3] = [1.0, 0.0, 0.0];
const DEFAULT_SOUND_SOURCE_POSITION : [f32; 3] = [0.0, 0.0, 0.0];

pub struct AudioManagerComponent {

    pub(crate) audio_stream: OutputStream,
    pub(crate) audio_stream_handle: OutputStreamHandle,
    pub(crate) ambient_sink_pool: Vec<Sink>,
    pub(crate) spatial_sink_pool: Vec<SpatialSink>, 
    pub(crate) free_ambient_sink_indexes: Vec<usize>,
    pub(crate) busy_ambient_sink_indexes: Vec<usize>,
    pub(crate) free_spatial_sink_indexes: Vec<usize>,
    pub(crate) busy_spatial_sink_indexes: Vec<usize>,

}

impl AudioManagerComponent {

    pub fn new(ambient_sink_pool_capacity: usize, spatial_sink_pool_capacity: usize) -> Self {

        // Get the output stream and its handle
        let (audio_stream, audio_stream_handle) = OutputStream::try_default().unwrap();

        // Create sink pools as vector of the certain capacity
        let mut ambient_sink_pool = Vec::<Sink>::with_capacity(ambient_sink_pool_capacity);
        let mut spatial_sink_pool = Vec::<SpatialSink>::with_capacity(spatial_sink_pool_capacity);

        // Create the sinks and push them into vectors
        for _ in 0..ambient_sink_pool_capacity {
            let new_sink = Sink::try_new(&audio_stream_handle).unwrap();
            ambient_sink_pool.push(new_sink);
        }

        for _ in 0..spatial_sink_pool_capacity {
            let new_sink = SpatialSink::try_new(&audio_stream_handle, DEFAULT_SOUND_SOURCE_POSITION.clone(), DEFAULT_LEFT_EAR_POSITION.clone(), DEFAULT_RIGHT_EAR_POSITION.clone()).unwrap();
            spatial_sink_pool.push(new_sink);
        }

        // Create vectors for free and busy indexes used for providing sinks to correct audio source components
        let mut free_ambient_sink_indexes = Vec::<usize>::new();
        let busy_ambient_sink_indexes = Vec::<usize>::new();
        let mut free_spatial_sink_indexes = Vec::<usize>::new();
        let busy_spatial_sink_indexes = Vec::<usize>::new();

        for i in 0..ambient_sink_pool_capacity {
            free_ambient_sink_indexes.push(i);
        }

        for i in 0..spatial_sink_pool_capacity {
            free_spatial_sink_indexes.push(i);
        }

        Self {
            audio_stream,
            audio_stream_handle,
            ambient_sink_pool,
            spatial_sink_pool,
            free_ambient_sink_indexes,
            busy_ambient_sink_indexes,
            free_spatial_sink_indexes,
            busy_spatial_sink_indexes,
        }
    } 

    // --- Getters
    
    // Get sink for ambient sound by index
    pub fn get_ambient_sink(&self, index: usize) -> &Sink {
        &self.ambient_sink_pool[index]
    }

    // Get sink for spatial sound by index
    pub fn get_spatial_sink(&self, index: usize) -> &SpatialSink {
        &self.spatial_sink_pool[index]
    }

    // Get free index for spatial sound sink if there is any
    pub fn get_spatial_sink_handle(&mut self) -> Option<usize> {
        if self.free_spatial_sink_indexes.len() == 0 {
            return None
        }
        else {
            let index = self.free_spatial_sink_indexes.pop().unwrap();
            self.busy_spatial_sink_indexes.push(index.clone());
            return Some(index)
        }
    }

    // Get free index for ambient sound sink if there is any
    pub fn get_ambient_sink_handle(&mut self) -> Option<usize> {
        if self.free_ambient_sink_indexes.len() == 0 {
            return None
        }
        else {
            let index = self.free_ambient_sink_indexes.pop().unwrap();
            self.busy_ambient_sink_indexes.push(index.clone());
            return Some(index)
        }
    }

    // --- Other functionalities

    // Give back 
    pub fn return_spatial_sink_handle(&mut self, index: usize) {
        if self.busy_spatial_sink_indexes.contains(&index) {
            if let Some(pos) = self.busy_spatial_sink_indexes.iter().position(|x| *x == index) {
                self.busy_spatial_sink_indexes.remove(pos);
            }
            self.free_spatial_sink_indexes.push(index);
        }
    }

    // Get free index for ambient sound sink if there is any
    pub fn return_ambient_sink_handle(&mut self, index: usize) {
        if self.busy_ambient_sink_indexes.contains(&index) {
            if let Some(pos) = self.busy_ambient_sink_indexes.iter().position(|x| *x == index) {
                self.busy_ambient_sink_indexes.remove(pos);
            }
            self.free_ambient_sink_indexes.push(index);
        }
    }

}

impl Default for AudioManagerComponent {

    fn default() -> Self {

        // Default capacity
        let ambient_sink_pool_capacity = 10;
        let spatial_sink_pool_capacity = 10;

        // Get the output stream and its handle
        let (audio_stream, audio_stream_handle) = OutputStream::try_default().unwrap();

        // Create the sinks and push them into vectors
        let mut ambient_sink_pool = Vec::<Sink>::with_capacity(ambient_sink_pool_capacity);
        let mut spatial_sink_pool = Vec::<SpatialSink>::with_capacity(spatial_sink_pool_capacity);

        for _ in 0..ambient_sink_pool_capacity {
            let new_sink = Sink::try_new(&audio_stream_handle).unwrap();
            ambient_sink_pool.push(new_sink);
        }

        for _ in 0..spatial_sink_pool_capacity {
            let new_sink = SpatialSink::try_new(&audio_stream_handle, DEFAULT_SOUND_SOURCE_POSITION.clone(), DEFAULT_LEFT_EAR_POSITION.clone(), DEFAULT_RIGHT_EAR_POSITION.clone()).unwrap();
            spatial_sink_pool.push(new_sink);
        }

        // Create vectors for free and busy indexes used for providing sinks to correct audio source components
        let mut free_ambient_sink_indexes = Vec::<usize>::new();
        let busy_ambient_sink_indexes = Vec::<usize>::new();
        let mut free_spatial_sink_indexes = Vec::<usize>::new();
        let busy_spatial_sink_indexes = Vec::<usize>::new();

        for i in 0..ambient_sink_pool_capacity {
            free_ambient_sink_indexes.push(i);
        }

        for i in 0..spatial_sink_pool_capacity {
            free_spatial_sink_indexes.push(i);
        }

        Self {
            audio_stream,
            audio_stream_handle,
            ambient_sink_pool,
            spatial_sink_pool,
            free_ambient_sink_indexes,
            busy_ambient_sink_indexes,
            free_spatial_sink_indexes,
            busy_spatial_sink_indexes,
        }
    }
}

impl PillTypeMapKey for AudioManagerComponent {
    type Storage = GlobalComponentStorage<AudioManagerComponent>; 
}

unsafe impl Send for AudioManagerComponent { }

impl GlobalComponent for AudioManagerComponent { }

