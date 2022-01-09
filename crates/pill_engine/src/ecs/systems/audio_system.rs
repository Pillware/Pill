use crate::{
    engine::Engine,
    ecs::{ ComponentStorage, Component, CameraComponent, EntityHandle, TransformComponent, AudioListenerComponent, AudioSourceComponent, WorldAudioComponent, scene}
};

use anyhow::{Result, Context, Error};

pub fn audio_system(engine: &mut Engine) -> Result<()> {

    let active_scene_handle = engine.scene_manager.get_active_scene_handle()?;

    // - Update the position for left and right ear in AudioListenerComponent

    // Fetch the entity, which contains both Transform and AudioListenerComponent 
    for (audio_listener, transform) in (&*engine).iterate_two_components::<AudioListenerComponent, TransformComponent>()? {

        if audio_listener.borrow_mut().as_mut().unwrap().get_enabled() {
            // Update the position for left and right ear
            audio_listener.borrow_mut().as_mut().unwrap().set_left_ear_position(transform.borrow().as_ref().unwrap().position.clone());
            audio_listener.borrow_mut().as_mut().unwrap().set_right_ear_position(transform.borrow().as_ref().unwrap().position.clone());
            break;
        }
    }

    // Update the position for each sound emitter represented by entity containing AudioSourceComponent

    // Fetch the entities, which contain both Tranform and AudioListenerComponent 
    for (audio_source, transform) in (&*engine).iterate_two_components::<AudioSourceComponent, TransformComponent>()? {

        // Update the position 
        audio_source.borrow_mut().as_mut().unwrap().set_source_position(transform.borrow().as_ref().unwrap().position.clone());
    } 

    Ok(())
}