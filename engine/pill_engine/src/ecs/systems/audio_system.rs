use crate::{
    engine::Engine,
    ecs::{ EntityHandle, TransformComponent, AudioListenerComponent, AudioSourceComponent, scene, AudioManagerComponent, SoundType },
};

use glam::{Vec3, Mat3};

use anyhow::{Result, Context, Error};
use std::f32::consts::PI;

fn get_rotation_matrix(angles: Vec3) -> Result<Mat3> {

    // Get the angles from the vector and convert them to radians
    let alfa = angles.x.to_radians();
    let beta = angles.y.to_radians();
    let gamma = angles.z.to_radians();

    // Prepare rotation matrices
    let rot_z = Mat3::from_rotation_z(alfa);
    let rot_y = Mat3::from_rotation_y(beta);
    let rot_x = Mat3::from_rotation_x(gamma);

    // Return rotation matrix
    Ok(rot_z * rot_y * rot_x)
}

pub fn audio_system(engine: &mut Engine) -> Result<()> {

    // --- Update ear positions
    let mut left_ear_position = Vec3::new(-1.0, 0.0, 0.0);
    let mut right_ear_position = Vec3::new(1.0, 0.0, 0.0);

    // Update ear positions
    for (entity_handle, audio_listener_component, transform_component) in engine.iterate_two_components::<AudioListenerComponent, TransformComponent>()? {

        if audio_listener_component.enabled {

            // Get the retotation matrix
            let left_rotation_matrix = get_rotation_matrix(transform_component.rotation)?;
            let right_rotation_matrix = get_rotation_matrix(-transform_component.rotation)?;

            // Get two points for left and right ear relative to the origin multiplied to rotation matrix
            left_ear_position = left_rotation_matrix * left_ear_position;
            right_ear_position = right_rotation_matrix * right_ear_position;

            // Add the original position
            left_ear_position += transform_component.position;
            right_ear_position += transform_component.position;

            break;
        }
    }

    // Update the sinks with new positions for left and right ear
    let audio_manager = engine.get_global_component_mut::<AudioManagerComponent>()?;
    for sink in audio_manager.spatial_sink_pool.iter_mut() {
        sink.set_left_ear_position(left_ear_position.into());
        sink.set_right_ear_position(right_ear_position.into());
    }

    // Update emitter position in all sinks based on transform components of entities to which audio source components are added
    let active_scene = engine.scene_manager.get_active_scene()?;
    for (entity_handle, audio_source_component, transform_component) in active_scene.get_two_component_iterator::<AudioSourceComponent, TransformComponent>()? {
        let audio_manager = engine.get_global_component::<AudioManagerComponent>()?;
        if let Some(index) = audio_source_component.sink_handle {
            audio_manager.get_spatial_sink(index).set_emitter_position(transform_component.position.clone().into());
        }
    }

    // --- Return free sinks to AudioManager

    // Iterate over each audio source and find sinks that stopped playing
    let audio_manager = engine.global_components.get_mut::<AudioManagerComponent>().unwrap().data.as_mut().unwrap();
    let active_scene = engine.scene_manager.get_active_scene_mut()?;
    for (entity_handle, audio_source_component) in active_scene.get_one_component_iterator_mut::<AudioSourceComponent>()? {
        // Check if the audio source has sink handle assigned
        if let Some(sink_handle) = audio_source_component.sink_handle {
            // Check if is playing
            let sound_type = audio_source_component.sound_type.clone();
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

            // Return sink to pool if stopped playing
            if !playing {

                let sink_handle = audio_source_component.return_sink().unwrap();
                audio_manager.return_sink(sink_handle, &sound_type);
            }
        }
    }

    Ok(())
}
