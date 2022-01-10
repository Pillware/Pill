use crate::{
    engine::Engine,
    ecs::{ ComponentStorage, Component, CameraComponent, EntityHandle, TransformComponent, AudioListenerComponent, AudioSourceComponent, scene, AudioManagerComponent}
};

use anyhow::{Result, Context, Error};
use cgmath::Vector3;

pub fn audio_system(engine: &mut Engine) -> Result<()> {
    
    // --- Update the position for left and right ear in AudioListenerComponent

    let mut left_ear_position: [f32; 3] = [-1.0, 0.0, 0.0];
    let mut right_ear_position: [f32; 3] = [1.0, 0.0, 0.0];

    // Fetch the entity, which contains both Transform and AudioListenerComponent 
    for (audio_listener, transform) in (&*engine).iterate_two_components::<AudioListenerComponent, TransformComponent>()? {

        if audio_listener.borrow_mut().as_mut().unwrap().get_enabled() {
            
            // Get the positions and update with regards to real left/right ear position
            let transform_position = transform.borrow().as_ref().unwrap().position.clone();
            left_ear_position[0] += transform_position[0];
            left_ear_position[1] += transform_position[1];
            left_ear_position[2] += transform_position[2];
            right_ear_position[0] += transform_position[0];
            right_ear_position[1] += transform_position[1];
            right_ear_position[2] += transform_position[2];
            break;
        }
    }

    // Update the sinks with new positions for left and right ear 

    let audio_manager = engine.get_global_component_mut::<AudioManagerComponent>()?;
    for sink in audio_manager.spatial_sink_pool.iter_mut() {
        sink.set_left_ear_position(left_ear_position);
        sink.set_right_ear_position(right_ear_position);
    }

    // --- Update the position for each sound emitter represented by entity containing AudioSourceComponent

    // Fetch the entities, which contain both Tranform and AudioListenerComponent 
    for (audio_source, transform) in (&*engine).iterate_two_components::<AudioSourceComponent, TransformComponent>()? {

        // Update the position with the function within each audio source 
        audio_source.borrow_mut().as_mut().unwrap().set_source_position(transform.borrow().as_ref().unwrap().position.clone());
    } 

    // --- Return free sinks back to AudioManager

    // Prepare vectors for returning free indexes
    let mut ambient_sink_pool = Vec::<usize>::new();
    let mut spatial_sink_pool = Vec::<usize>::new();

    // Iterate over each audio source
    for audio_source in (&*engine).iterate_one_component::<AudioSourceComponent>()? {

        // Check if the audio source has sink handle assigned; if not, continue looking over other sources
        if !audio_source.borrow().as_ref().unwrap().has_sink_handle() {
            continue;
        }

        // Check if the audio source is playing any sound; if yes, continue looking over other sources
        if !audio_source.borrow_mut().as_mut().unwrap().get_is_sound_queue_empty() {
            continue;
        }

        // Return back the handle as the index to the pool of free indexes used for sink assignment
        let handle = audio_source.borrow_mut().as_mut().unwrap().get_back_sink_handle();
        match audio_source.borrow().as_ref().unwrap().is_spatial() {
            true => {
                spatial_sink_pool.push(handle.unwrap());
            },
            false => {
                ambient_sink_pool.push(handle.unwrap());
            }
        }
    }

    // Get the audio manager
    let audio_manager = engine.get_global_component_mut::<AudioManagerComponent>()?;

    // Return back the indexes
    for index in ambient_sink_pool {
        audio_manager.return_ambient_sink_handle(index);
    }

    for index in spatial_sink_pool {
        audio_manager.return_spatial_sink_handle(index);
    }

    // Success
    Ok(())
}