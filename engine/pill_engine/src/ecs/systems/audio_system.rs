use crate::{
    ecs::{
        AudioListenerComponent, AudioManagerComponent, AudioSourceComponent, SoundType,
        TransformComponent,
    },
    engine::Engine,
};
use pill_core::{Matrix3f, Vector3f};

use pill_core::Result;

fn get_rotation_matrix(angles: Vector3f) -> Result<Matrix3f> {
    // Get the angles from the vector and convert them to radians
    let angle_x = angles.x.to_radians();
    let angle_y = angles.y.to_radians();
    let angle_z = angles.z.to_radians();

    // Prepare rotation matrices
    let rot_z = Matrix3f::from_rotation_z(angle_x);
    let rot_y = Matrix3f::from_rotation_y(angle_y);
    let rot_x = Matrix3f::from_rotation_x(angle_z);

    // Return rotation matrix
    Ok(rot_z * rot_y * rot_x)
}

pub fn audio_system(engine: &mut Engine) -> Result<()> {
    // Skip if no audio manager (headless or no audio device).
    if engine
        .get_global_component::<AudioManagerComponent>()
        .is_err()
    {
        return Ok(());
    }

    // --- Update ear positions
    let mut left_ear_position = Vector3f::new(-1.0, 0.0, 0.0);
    let mut right_ear_position = Vector3f::new(1.0, 0.0, 0.0);

    // Update ear positions
    if let Ok(iter) = engine.iterate_two_components::<AudioListenerComponent, TransformComponent>()
    {
        for (_entity_handle, audio_listener_component, transform_component) in iter {
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
    }

    // Update the sinks with new positions for left and right ear
    let audio_manager = engine.get_global_component_mut::<AudioManagerComponent>()?;
    for sink in audio_manager.spatial_sink_pool.iter_mut() {
        sink.set_left_ear_position(left_ear_position.into());
        sink.set_right_ear_position(right_ear_position.into());
    }

    // Update emitter position in all sinks based on transform components of entities to which audio source components are added
    if let Ok(active_scene) = engine.scene_manager.get_active_scene() {
        if let Ok(iter) =
            active_scene.get_two_component_iterator::<AudioSourceComponent, TransformComponent>()
        {
            for (_entity_handle, audio_source_component, transform_component) in iter {
                let audio_manager = engine.get_global_component::<AudioManagerComponent>()?;
                if let Some(index) = audio_source_component.sink_handle {
                    audio_manager
                        .get_spatial_sink(index)
                        .set_emitter_position(transform_component.position.into());
                }
            }
        }
    }

    // --- Return free sinks to AudioManager

    // Iterate over each audio source and find sinks that stopped playing
    let audio_manager = engine
        .global_components
        .get_mut::<AudioManagerComponent>()
        .unwrap()
        .data
        .as_mut()
        .unwrap();
    if let Ok(active_scene) = engine.scene_manager.get_active_scene_mut() {
        if let Ok(iter) = active_scene.get_one_component_iterator_mut::<AudioSourceComponent>() {
            for (_entity_handle, audio_source_component) in iter {
                // Check if the audio source has sink handle assigned
                if let Some(sink_handle) = audio_source_component.sink_handle {
                    // Check if is playing
                    let playing = match audio_source_component.sound_type {
                        SoundType::Sound2D => {
                            let sink = audio_manager.get_ambient_sink(sink_handle);
                            !sink.is_paused()
                        }
                        SoundType::Sound3D => {
                            let sink = audio_manager.get_spatial_sink(sink_handle);
                            !sink.is_paused()
                        }
                    };

                    // Return sink to pool if stopped playing
                    if !playing {
                        let sink_handle = audio_source_component.return_sink().unwrap();
                        audio_manager.return_sink(sink_handle, &audio_source_component.sound_type);
                    }
                }
            }
        }
    }

    Ok(())
}
