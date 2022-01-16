use crate::{ 
    engine::Engine,
    ecs::{ InputComponent, InputEvent },
};

use pill_core::{ Vector2f };

use anyhow::{ Result, Context, Error };
use winit::event::{ ElementState, MouseButton, MouseScrollDelta };

pub fn input_system(engine: &mut Engine) -> Result<()> {
    let input_component = engine.get_global_component_mut::<InputComponent>()?;
    input_component.set_keys();
    input_component.set_mouse_buttons();
    input_component.set_mouse_motion();

    while engine.input_queue.is_empty() == false {
        let front_event = engine.input_queue.pop_front().unwrap();
        let input_component = engine.get_global_component_mut::<InputComponent>()?;
    
        match front_event {
            // Keyboard keys
            InputEvent::KeyboardKey { key, state } => {
                input_component.set_key(key, state); 
            },

            // Mouse buttons
            InputEvent::MouseButton {key, state} => {
                input_component.set_mouse_button(key, state);
            },

            // Mouse scroll
            InputEvent::MouseWheel { delta } => {
                match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        input_component.set_mouse_scroll_delta(Vector2f::new(x, y));
                    },

                    MouseScrollDelta::PixelDelta(delta) => {
                        input_component.set_mouse_scroll_pixel_delta(Vector2f::new(delta.x as f32, delta.y as f32));
                    },
                }
            },

            // Mouse delta
            InputEvent::MouseDelta { delta } => {
                input_component.set_mouse_delta(delta);
            },

            // Mouse position
            InputEvent::MousePosition { position} => {
                input_component.set_mouse_position(position);
            },
        }
    }
    
    Ok(())
}