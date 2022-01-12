use crate::{
    engine::Engine,
    ecs::{ ComponentStorage, Component, CameraComponent, EntityHandle, TransformComponent, AudioListenerComponent, AudioSourceComponent, scene, AudioManagerComponent}
};

use anyhow::{Result, Context, Error};
use cgmath::{Vector3, Matrix3};
use std::f32::consts::PI;

fn get_rotation_matrix(angles: Vector3<f32>) -> Result<Matrix3<f32>> {
    
    // Get the angles from the vector 
    let mut alfa = angles[0];
    let mut beta = angles[1];
    let mut gamma = angles[2];

    // Convert from degrees to radians
    alfa = alfa * PI / 180.0;
    beta = beta * PI / 180.0;
    gamma = gamma * PI / 180.0;

    // Prepare rotation matrices
    let alfa_rotation_matrix = Matrix3::new(alfa.cos(), alfa.sin(), 0.0,
                                                        -alfa.sin(), alfa.cos(), 0.0,
                                                        0.0, 0.0, 1.0);

    let beta_rotation_matrix = Matrix3::new(beta.cos(), 0.0, -beta.sin(),
                                                        0.0, 1.0, 0.0,
                                                        beta.sin(), 0.0, beta.cos());

    let gamma_totation_matrix = Matrix3::new(1.0, 0.0, 0.0,
                                                        0.0, gamma.cos(), gamma.sin(),
                                                        0.0, -gamma.sin(), gamma.cos());

    // Get the final rotation matrix 
    let rotation_matrix = alfa_rotation_matrix * beta_rotation_matrix * gamma_totation_matrix;
    
    // Return rotation matrix
    Ok(rotation_matrix)
}

pub fn audio_system(engine: &mut Engine) -> Result<()> {
    
    // --- Update the position for left and right ear in AudioListenerComponent

    let mut left_ear_position = Vector3::<f32>::new(-1.0, 0.0, 0.0);
    let mut right_ear_position = Vector3::<f32>::new(1.0, 0.0, 0.0);

    // Fetch the entity, which contains both Transform and AudioListenerComponent 
    for (audio_listener, transform) in (&*engine).iterate_two_components::<AudioListenerComponent, TransformComponent>()? {

        if audio_listener.borrow_mut().as_mut().unwrap().get_enabled() {
            
            // Get the retotation matrix
            let left_rotation_matrix = get_rotation_matrix(transform.borrow().as_ref().unwrap().rotation)?;
            let right_rotation_matrix = get_rotation_matrix(-transform.borrow().as_ref().unwrap().rotation)?;
            
            // Get two points for left and right ear relative to the origin multiplied to rotation matrix
            left_ear_position = left_rotation_matrix * left_ear_position;
            right_ear_position = right_rotation_matrix * right_ear_position;

            // Add the original position
            left_ear_position += transform.borrow().as_ref().unwrap().position;
            right_ear_position += transform.borrow().as_ref().unwrap().position;

            break;
        }
    }

    // Update the sinks with new positions for left and right ear 

    let audio_manager = engine.get_global_component_mut::<AudioManagerComponent>()?;
    for sink in audio_manager.spatial_sink_pool.iter_mut() {
        sink.set_left_ear_position(left_ear_position.into());
        sink.set_right_ear_position(right_ear_position.into());
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