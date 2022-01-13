use crate::{
    engine::Engine,
    ecs::{ EntityHandle, TransformComponent, AudioListenerComponent, AudioSourceComponent, scene, AudioManagerComponent, SoundType }, 
};

use pill_core::Vector3f;

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

    // --- Update ear positions
    let mut left_ear_position = Vector3f::new(-1.0, 0.0, 0.0);
    let mut right_ear_position = Vector3f::new(1.0, 0.0, 0.0);

    // Update ear positions
    for (audio_listener, transform) in (&*engine).iterate_two_components::<AudioListenerComponent, TransformComponent>()? {

        if audio_listener.borrow_mut().as_mut().unwrap().enabled {
            
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
    // Update spatial sinks ear positions
    {
        let audio_manager = engine.get_global_component_mut::<AudioManagerComponent>()?;
        for sink in audio_manager.spatial_sink_pool.iter_mut() {
            sink.set_left_ear_position(left_ear_position.into());
            sink.set_right_ear_position(right_ear_position.into());
        }
    }
   
    // Update emitter position in all sinks based on transform components of entities to which audio source components are added
    for (audio_source, transform) in (&*engine).iterate_two_components::<AudioSourceComponent, TransformComponent>()? {
        let audio_manager = (&*engine).get_global_component::<AudioManagerComponent>()?;
        if let Some(index) = audio_source.borrow_mut().as_mut().unwrap().sink_handle {
            audio_manager.get_spatial_sink(index).set_emitter_position(transform.borrow().as_ref().unwrap().position.clone().into());
        }
    } 

    // --- Return free sinks to AudioManager
    let mut sinks_to_return = Vec::<(usize, SoundType)>::new();

    // Iterate over each audio source and find sinks that stopped playing
    let audio_manager = (&*engine).get_global_component::<AudioManagerComponent>()?;
    for audio_source in (&*engine).iterate_one_component::<AudioSourceComponent>()? {
        // Check if the audio source has sink handle assigned
        if let Some(sink_handle) = audio_source.borrow().as_ref().unwrap().sink_handle {
            // Check if is playing
            let sound_type = audio_source.borrow().as_ref().unwrap().sound_type.clone();
            let playing = match sound_type {
                SoundType::Sound2D => {
                    let sink = audio_manager.get_ambient_sink(sink_handle);
                    !sink.is_paused()
                },
                SoundType::Sound3D => {
                    let sink = audio_manager.get_spatial_sink(sink_handle);
                    !sink.is_paused()
                },
            };

            if !playing {
                // Return sink handle
                let sink_handle = audio_source.borrow_mut().as_mut().unwrap().return_sink().unwrap();
                sinks_to_return.push((sink_handle, sound_type));

            }
        }
    }

    // Return sink handles
    let audio_manager = engine.get_global_component_mut::<AudioManagerComponent>()?;
    for sink in sinks_to_return {
        audio_manager.return_sink(sink.0, &sink.1);
    }

    Ok(())
}