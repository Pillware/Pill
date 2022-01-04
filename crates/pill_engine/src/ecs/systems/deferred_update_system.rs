use crate::{
    engine::Engine,
    ecs::{ DeferredUpdateRequest }
};

use std::collections::VecDeque;
use anyhow::{Result, Context, Error};

pub fn deferred_update_system(engine: &mut Engine) -> Result<()> {
    // Get deferred update component
    let deferred_update_component = &mut engine.TEMP_deferred_component; // engine.get_global_component_mut::<DeferredUpdateComponent>();
    
    // Get deferred update manager
    let mut deferred_update_manager = deferred_update_component.manager.0.lock().expect("Critical: Mutex is blocked");
   
    // Create new empty queue
    let new_request_queue = VecDeque::<Box<dyn DeferredUpdateRequest + 'static>>::new();
    
    // Swap new queue with queue in component
    let mut request_queue = deferred_update_manager.request_queue.replace(new_request_queue).expect("Critical: Queue is None");
    
    // Drop mutex lock
    drop(deferred_update_manager);
    
    // Process all requests
    while !request_queue.is_empty() {
        let mut request = request_queue.pop_front().unwrap();
        request.process(engine)?;
    }

    Ok(())
}