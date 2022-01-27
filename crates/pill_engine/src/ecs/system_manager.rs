use crate::engine::Engine;

use pill_core::EngineError;

use core::fmt;
use std::{collections::HashMap, fmt::Display};
use anyhow::{Result, Context, Error};
use boolinator::Boolinator;
use indexmap::IndexMap;

pub type SystemFunction = fn(engine: &mut Engine) -> Result<()>;

pub struct System {
    pub(crate) name: String,
    pub(crate) update_phase: UpdatePhase,
    pub(crate) system_function: SystemFunction,
    pub(crate) enabled: bool,
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum UpdatePhase {
    PreGame,
    Game,
    PostGame,
}

impl Display for UpdatePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub struct SystemManager {
    pub(crate) update_phases: IndexMap<UpdatePhase, IndexMap<String, System>>,
}

impl SystemManager {
    pub fn new() -> Self {
	    let mut update_phases = IndexMap::<UpdatePhase, IndexMap<String, System>>::new();

        // Register phases
        update_phases.insert(UpdatePhase::PreGame, IndexMap::<String, System>::new());
        update_phases.insert(UpdatePhase::Game, IndexMap::<String, System>::new());
        update_phases.insert(UpdatePhase::PostGame, IndexMap::<String, System>::new());

        Self { 
            update_phases
        }
    }

    pub fn add_system(&mut self, name: &str, system_function: SystemFunction, update_phase: UpdatePhase) -> Result<()> {
        // Find collection of systems for given update phase
        let system_collection = self.update_phases.get_mut(&update_phase).ok_or(Error::new(EngineError::SystemUpdatePhaseNotFound(format!("{}", update_phase))))?;

        // Check if system with that name already exists
        if system_collection.contains_key(name) {
            return Err(Error::new(EngineError::SystemAlreadyExists(name.to_string(), format!("{}", update_phase))))
        }

        // Create system object
        let system_object = System {
            name: name.to_string(),
            update_phase, 
            system_function,
            enabled: true,
        };

        // Add system
        system_collection.insert(name.to_string(), system_object);

        Ok(())
    }

    pub fn remove_system(&mut self, name: &str, update_phase: UpdatePhase) -> Result<()> { 
        // Find collection of systems for given update phase
        let system_collection = self.update_phases.get_mut(&update_phase).ok_or(Error::new(EngineError::SystemUpdatePhaseNotFound(format!("{}", update_phase))))?;

        // Check if system with that name exists
        if !system_collection.contains_key(name) {
            return Err(Error::new(EngineError::SystemNotFound(name.to_string(), format!("{}", update_phase))))
        }

        // Remove system
        system_collection.remove(name);

        Ok(())
    }

    pub fn toggle_system(&mut self, name: &str, update_phase: UpdatePhase, enabled: bool) -> Result<()> { 
        // Find collection of systems for given update phase
        let system_collection = self.update_phases.get_mut(&update_phase).ok_or(Error::new(EngineError::SystemUpdatePhaseNotFound(format!("{}", update_phase))))?;

        // Check if system with that name exists
        let system = system_collection.get_mut(name).ok_or(Error::new(EngineError::SystemNotFound(name.to_string(), format!("{}", update_phase))))?;

        // Set system state
        system.enabled = enabled;

        Ok(())
    }
}