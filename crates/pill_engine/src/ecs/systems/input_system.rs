use crate::{ 
    engine::Engine,
    ecs::{ InputComponent, InputEvent },
};

use std::{collections::{vec_deque, VecDeque}, borrow::{Borrow, BorrowMut}};
use anyhow::{Result, Context, Error};
use lazy_static::__Deref;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};

pub fn input_system(engine: &mut Engine) -> Result<()> {

    let input_component = engine.get_global_component_mut::<InputComponent>()?;
    input_component.overwrite_previous_positions();
    input_component.overwrite_buttons();

    while engine.input_queue.is_empty() == false {
        
        let front_event = engine.input_queue.pop_front().unwrap();
        let input_component = engine.get_global_component_mut::<InputComponent>()?;
    
        match front_event {

            // - Keyboard keys

            InputEvent::KeyboardKey { key, state } => {
                input_component.set_key(key, state); 
            }

            // - Mouse buttons

            InputEvent::MouseButton {key, state} => {
                input_component.set_mouse_button(key, state);
            }

            // - Mouse scroll

            InputEvent::MouseWheel { delta } => {
                match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        input_component.set_mouse_scroll_line_delta(x, y);
                    },

                    MouseScrollDelta::PixelDelta(delta) => {
                        input_component.set_mouse_scroll_delta(delta);
                    },
                }
            }

            // - Mouse motion

            InputEvent::MouseMotion { position} => {
                input_component.set_mouse_position(position);
            }
        }

    }
    
    Ok(())
}