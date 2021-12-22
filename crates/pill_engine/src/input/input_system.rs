use std::collections::{vec_deque, VecDeque};

use crate::game::Engine;
use anyhow::{Result, Context, Error};
use lazy_static::__Deref;

use super::{InputComponent, InputEvent};

pub fn input_system(engine: &mut Engine) -> Result<()> {
    engine.add_global_component(InputComponent::default());

    while engine.input_queue.is_empty() == false {
        let front_event = engine.input_queue.pop_front().unwrap();
        match front_event {
            InputEvent::KeyboardKey { key, state } => {
                engine.get_global_component_mut::<InputComponent>()?.unwrap().press_key(key);
            }
            _ => ()
        }
    }
    Ok(())
}