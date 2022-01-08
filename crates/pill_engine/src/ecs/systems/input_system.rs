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

            // - Keyboard Event
            InputEvent::KeyboardKey { key, state } => {
                match state {
                    ElementState::Pressed => { 
                        input_component.set_key_pressed(key as usize); 
                    }
                    ElementState::Released => { 
                        input_component.set_key_released(key as usize) 
                    }
                }
            }

            // - Mouse Button Event
            InputEvent::MouseKey {key, state} => {
                match key {

                    MouseButton::Left => {
                        match state {
                            ElementState::Pressed => input_component.set_left_mouse_button_pressed(),
                            ElementState::Released => input_component.set_left_mouse_button_released()
                        }
                    }
                    MouseButton::Middle => {
                        match state {
                            ElementState::Pressed => input_component.set_middle_mouse_button_pressed(),
                            ElementState::Released => input_component.set_middle_mouse_button_released()
                        }
                    }

                    MouseButton::Right => {
                        match state {
                            ElementState::Pressed => input_component.set_right_mouse_button_pressed(),
                            ElementState::Released => input_component.set_right_mouse_button_released()
                        }
                    }
                    _ => ()
                }
            }

            InputEvent::MouseMotion { position} => {

                input_component.set_current_mouse_position(position);

            }

            InputEvent::MouseWheel { delta } => {
                match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        input_component.set_current_mouse_line_delta(x, y);
                    },

                    MouseScrollDelta::PixelDelta(pos) => {
                        input_component.set_current_mouse_position(pos);
                    },
                }
            }
        }

    }
    
    Ok(())
}