use crate::{
    ecs::{ GlobalComponent, ComponentStorage, GlobalComponentStorage }, 
};

use pill_core::{PillTypeMapKey, Vector3f};

use std::{ 
    any::Any,
    cell::RefCell,
    collections::{HashMap, VecDeque}, ops::IndexMut,
};
use rodio::{ OutputStream, OutputStreamHandle, Sink, SpatialSink };

const DEFAULT_LEFT_EAR_POSITION: Vector3f = Vector3f::new(-1.0, 0.0, 0.0);
const DEFAULT_RIGHT_EAR_POSITION: Vector3f = Vector3f::new(1.0, 0.0, 0.0);
const DEFAULT_SOUND_SOURCE_POSITION: Vector3f = Vector3f::new(0.0, 0.0, 0.0);

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SoundType {
    Sound2D,
    Sound3D
}

pub struct AudioManagerComponent {
    pub(crate) audio_stream: OutputStream,
    pub(crate) audio_stream_handle: OutputStreamHandle,
    pub(crate) ambient_sink_pool: Vec<Sink>,
    pub(crate) spatial_sink_pool: Vec<SpatialSink>, 
    pub(crate) free_ambient_sink_handles: VecDeque<usize>,
    pub(crate) busy_ambient_sink_handles: VecDeque<usize>,
    pub(crate) free_spatial_sink_handles: VecDeque<usize>,
    pub(crate) busy_spatial_sink_handles: VecDeque<usize>,
}

impl AudioManagerComponent {
    pub fn new(ambient_sink_pool_capacity: usize, spatial_sink_pool_capacity: usize) -> Self {
        // Get output audio stream and its handle
        let (audio_stream, audio_stream_handle) = OutputStream::try_default().unwrap();

        // Create sink pools
        let mut ambient_sink_pool = Vec::<Sink>::with_capacity(ambient_sink_pool_capacity);
        let mut spatial_sink_pool = Vec::<SpatialSink>::with_capacity(spatial_sink_pool_capacity);

        // Create sinks and push them into vectors
        for _ in 0..ambient_sink_pool_capacity {
            let new_sink = Sink::try_new(&audio_stream_handle).unwrap();
            ambient_sink_pool.push(new_sink);
        }

        for _ in 0..spatial_sink_pool_capacity {
            let new_sink = SpatialSink::try_new(
                &audio_stream_handle, 
                DEFAULT_SOUND_SOURCE_POSITION.into(), 
                DEFAULT_LEFT_EAR_POSITION.into(), 
                DEFAULT_RIGHT_EAR_POSITION.into(),
            ).unwrap();
            spatial_sink_pool.push(new_sink);
        }

        // Create vectors for free and busy handles used for providing sinks to correct audio source components
        let mut free_ambient_sink_handles = VecDeque::<usize>::new();
        let busy_ambient_sink_handles = VecDeque::<usize>::new();
        let mut free_spatial_sink_handles = VecDeque::<usize>::new();
        let busy_spatial_sink_handles = VecDeque::<usize>::new();

        for i in 0..ambient_sink_pool_capacity {
            free_ambient_sink_handles.push_back(i);
        }

        for i in 0..spatial_sink_pool_capacity {
            free_spatial_sink_handles.push_back(i);
        }

        Self {
            audio_stream,
            audio_stream_handle,
            ambient_sink_pool,
            spatial_sink_pool,
            free_ambient_sink_handles,
            busy_ambient_sink_handles,
            free_spatial_sink_handles,
            busy_spatial_sink_handles,
        }
    } 

    // Get sink for ambient sound by handle
    pub(crate) fn get_ambient_sink(&self, sink_handle: usize) -> &Sink {
        &self.ambient_sink_pool[sink_handle]
    }

    // Get sink for spatial sound by handle
    pub(crate) fn get_spatial_sink(&self, sink_handle: usize) -> &SpatialSink {
        &self.spatial_sink_pool[sink_handle]
    }

    // Get handle for sound sink if there is any free
    pub(crate) fn get_free_sink_handle(&mut self, sound_type: &SoundType) -> Option<usize> {
        match self.get_free_sink_handle_queue(sound_type).pop_front() {
            Some(v) => {
                self.get_busy_sink_handle_queue(sound_type).push_back(v);
                return Some(v);
            },
            None => return None,
        }
    }

    // Give back the free handle
    pub(crate) fn return_sink(&mut self, sink_handle: usize, sound_type: &SoundType) {
        if let Some(handle) = self.get_busy_sink_handle_queue(sound_type).iter().position(|x| *x == sink_handle) {
            self.get_busy_sink_handle_queue(sound_type).remove(sink_handle);
            self.get_free_sink_handle_queue(sound_type).push_back(sink_handle);
        }
    }

    fn get_free_sink_handle_queue(&mut self, sound_type: &SoundType) -> &mut VecDeque::<usize> {
        match sound_type {
            SoundType::Sound2D => &mut self.free_ambient_sink_handles,
            SoundType::Sound3D => &mut self.free_spatial_sink_handles,
        }
    }

    fn get_busy_sink_handle_queue(&mut self, sound_type: &SoundType) -> &mut VecDeque::<usize> {
        match sound_type {
            SoundType::Sound2D => &mut self.busy_ambient_sink_handles,
            SoundType::Sound3D => &mut self.busy_spatial_sink_handles,
        }
    }
}

impl PillTypeMapKey for AudioManagerComponent {
    type Storage = GlobalComponentStorage<AudioManagerComponent>; 
}

unsafe impl Send for AudioManagerComponent { }

impl GlobalComponent for AudioManagerComponent { }

