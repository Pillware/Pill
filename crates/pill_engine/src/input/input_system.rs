use std::{collections::{vec_deque, VecDeque}, borrow::{Borrow, BorrowMut}};

use crate::game::Engine;
use anyhow::{Result, Context, Error};
use lazy_static::__Deref;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};

use super::{InputComponent, InputEvent, input_component::GlobalComponent};

pub fn input_system(engine: &mut Engine) -> Result<()> {

    while engine.input_queue.is_empty() == false {
        
        let front_event = engine.input_queue.pop_front().unwrap();
        let comp = engine.get_global_component_mut::<InputComponent>()?;
        comp.overwrite_prev_keys();
        
        match front_event {

            // - Keyboard Event
            InputEvent::KeyboardKey { key, state } => {
                match state {
                    ElementState::Pressed => { 
                        comp.press_key(key as usize); 
                    }
                    ElementState::Released => { 
                        comp.release_key(key as usize) 
                    }
                }
            }

            // - Mouse Button Event
            InputEvent::MouseKey {key, state} => {
                match key {

                    MouseButton::Left => {
                        match state {
                            ElementState::Pressed => comp.press_left_mouse_button(),
                            ElementState::Released => comp.release_left_mouse_button()
                        }
                    }
                    MouseButton::Middle => {
                        match state {
                            ElementState::Pressed => comp.press_middle_mouse_button(),
                            ElementState::Released => comp.release_middle_mouse_button()
                        }
                    }

                    MouseButton::Right => {
                        match state {
                            ElementState::Pressed => comp.press_right_mouse_button(),
                            ElementState::Released => comp.release_right_mouse_button()
                        }
                    }
                    _ => ()
                }
            }

            InputEvent::MouseMotion { position} => {

                comp.set_current_mouse_pos(position);

            }

            InputEvent::MouseWheel { delta } => {
                match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        comp.set_current_mouse_line_delta(x, y);
                    },

                    MouseScrollDelta::PixelDelta(pos) => {
                        comp.set_current_mouse_pos(pos);
                    },
                }
            }
        }

    }
    
    Ok(())
}