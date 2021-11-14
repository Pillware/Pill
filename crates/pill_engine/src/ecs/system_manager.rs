use core::fmt;
use std::{collections::HashMap, fmt::Display};

use anyhow::{Result, Context, Error};
use boolinator::Boolinator;
use indexmap::IndexMap;
use pill_core::EngineError;

use crate::game::Engine;

type System = fn(engine: &mut Engine);

#[derive(Debug, PartialEq, Eq, Hash)]
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

    pub fn add_system(&mut self, name: &str, system_function: fn(engine: &mut Engine), update_phase: UpdatePhase) -> Result<()> {
        // Find collection of systems for given update phase
        let system_collection = self.update_phases.get_mut(&update_phase).unwrap();

        // Check if system with that name already exists
        system_collection.contains_key(name).eq(&false).ok_or(Error::new(EngineError::SystemAlreadyExists(name.to_string(), format!("{}", update_phase))))?;

        // Add system
        system_collection.insert(name.to_string(), system_function);
        Ok(())
    }

    // [TODO] Removing system may cause iterator to break, check that
    pub fn remove_system(&mut self, name: &str, update_phase: UpdatePhase) -> Result<()> { 
        // Find collection of systems for given update phase
        let system_collection = self.update_phases.get_mut(&update_phase).unwrap();

        // Check if system with that name already exists
        system_collection.contains_key(name).eq(&true).ok_or(Error::new(EngineError::SystemAlreadyExists(name.to_string(), format!("{}", update_phase))))?;

        // Remove system
        system_collection.remove(name);
        Ok(())
    }
}