use crate::{
    ecs::components::{DeferredOperation, DeferredOperationComponent}, engine::Engine
};

use std::collections::VecDeque;
use anyhow::{Result, Context, Error};

pub fn deferred_operation_system(engine: &mut Engine) -> Result<()> {
    // Get deferred operation component
    let deferred_operation_component = engine.get_global_component_mut::<DeferredOperationComponent>().expect("Critical: No DeferredOperationComponent");
    
    // Get deferred operation manager
    let mut deferred_operation_manager = deferred_operation_component.manager.0.lock().expect("Critical: Mutex is blocked");
   
    // Create new empty queue
    let new_request_queue = VecDeque::<Box<dyn DeferredOperation + 'static>>::new();
    
    // Swap new queue with queue in component
    let mut request_queue = deferred_operation_manager.operation_queue.replace(new_request_queue).expect("Critical: Queue is None");
    
    // Drop mutex lock
    drop(deferred_operation_manager);
    
    // Process all requests
    while !request_queue.is_empty() {
        let mut request = request_queue.pop_front().unwrap();
        request.process(engine)?;
    }

    Ok(())
}