use crate::engine::Engine;

use pill_core::{EngineError, Timer};

use core::fmt;
use pill_core::Result;
use std::fmt::Display;

pub type SystemFunction = fn(engine: &mut Engine) -> Result<()>;

pub struct System {
    pub(crate) name: String,
    pub(crate) update_phase: UpdatePhase,
    pub(crate) system_function: SystemFunction,
    pub(crate) enabled: bool,
    pub(crate) timer: Option<Timer>,
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
    // Outer Vec preserves phase execution order (PreGame → Game → PostGame).
    // Inner Vec preserves system registration order within each phase.
    pub(crate) update_phases: Vec<(UpdatePhase, Vec<(String, System)>)>,
}

impl SystemManager {
    pub fn new() -> Self {
        // Register phases
        Self {
            update_phases: vec![
                (UpdatePhase::PreGame, Vec::new()),
                (UpdatePhase::Game, Vec::new()),
                (UpdatePhase::PostGame, Vec::new()),
            ],
        }
    }

    fn phase_systems_mut(
        &mut self,
        update_phase: &UpdatePhase,
    ) -> Result<&mut Vec<(String, System)>> {
        self.update_phases
            .iter_mut()
            .find(|(phase, _)| phase == update_phase)
            .map(|(_, systems)| systems)
            .ok_or_else(|| {
                EngineError::SystemUpdatePhaseNotFound(format!("{}", update_phase)).into()
            })
    }

    pub fn get_system(&mut self, name: &str, update_phase: UpdatePhase) -> Result<&mut System> {
        // Find collection of systems for given update phase
        let phase_str = format!("{}", update_phase);
        let system_collection = self.phase_systems_mut(&update_phase)?;
        // Get system by name
        system_collection
            .iter_mut()
            .find(|(system_name, _)| system_name == name)
            .map(|(_, system)| system)
            .ok_or_else(|| EngineError::SystemNotFound(name.to_string(), phase_str).into())
    }

    pub fn add_system(
        &mut self,
        name: &str,
        system_function: SystemFunction,
        update_phase: UpdatePhase,
    ) -> Result<()> {
        // Find collection of systems for given update phase
        let phase_str = format!("{}", update_phase);
        let system_collection = self.phase_systems_mut(&update_phase)?;

        // Check if system with that name already exists
        if system_collection.iter().any(|(k, _)| k == name) {
            return Err(EngineError::SystemAlreadyExists(name.to_string(), phase_str).into());
        }

        // Create system object
        // Add system
        system_collection.push((
            name.to_string(),
            System {
                name: name.to_string(),
                update_phase,
                system_function,
                enabled: true,
                timer: Some(Timer::new()),
            },
        ));

        Ok(())
    }

    pub fn remove_system(&mut self, name: &str, update_phase: UpdatePhase) -> Result<()> {
        // Find collection of systems for given update phase
        let phase_str = format!("{}", update_phase);
        let system_collection = self.phase_systems_mut(&update_phase)?;

        // Check if system with that name exists
        if !system_collection.iter().any(|(k, _)| k == name) {
            return Err(EngineError::SystemNotFound(name.to_string(), phase_str).into());
        }

        // Remove system
        system_collection.retain(|(k, _)| k != name);
        Ok(())
    }

    pub fn toggle_system(
        &mut self,
        name: &str,
        update_phase: UpdatePhase,
        enabled: bool,
    ) -> Result<()> {
        let system = self.get_system(name, update_phase)?;
        // Set system state
        system.enabled = enabled;
        Ok(())
    }

    // This function can be called in the system function to get the timer for the system
    // It will pass the ownership of the timer to the requsting scope.
    // This has to be returned back using update_system_timer function, otherwise engine will panic.
    // NOTE: Before system function is called, engine already starts "System update" context in the timer
    pub fn get_system_timer(
        &mut self,
        name: &str,
        update_phase: UpdatePhase,
    ) -> Result<Option<Timer>> {
        // Get system by name
        let system: &mut System = self.get_system(name, update_phase)?;

        // Return timer
        Ok(system.timer.take())
    }

    pub fn update_system_timer(
        &mut self,
        name: &str,
        update_phase: UpdatePhase,
        timer: Timer,
    ) -> Result<()> {
        // Get system by name
        let system = self.get_system(name, update_phase)?;
        // Update timer
        system.timer = Some(timer);
        Ok(())
    }
}
