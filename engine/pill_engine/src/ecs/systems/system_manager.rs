use crate::engine::Engine;

use pill_core::{EngineError, Timer};

use pill_core::Result;
use core::fmt;
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
            .find(|(p, _)| p == update_phase)
            .map(|(_, v)| v)
            .ok_or_else(|| EngineError::SystemUpdatePhaseNotFound(format!("{}", update_phase)).into())
    }

    pub fn get_system(&mut self, name: &str, update_phase: UpdatePhase) -> Result<&mut System> {
        let phase_str = format!("{}", update_phase);
        let col = self.phase_systems_mut(&update_phase)?;
        col.iter_mut()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
            .ok_or_else(|| EngineError::SystemNotFound(name.to_string(), phase_str).into())
    }

    pub fn add_system(
        &mut self,
        name: &str,
        system_function: SystemFunction,
        update_phase: UpdatePhase,
    ) -> Result<()> {
        let phase_str = format!("{}", update_phase);
        let col = self.phase_systems_mut(&update_phase)?;

        if col.iter().any(|(k, _)| k == name) {
            return Err(EngineError::SystemAlreadyExists(
                name.to_string(),
                phase_str,
            ).into());
        }

        col.push((name.to_string(), System {
            name: name.to_string(),
            update_phase,
            system_function,
            enabled: true,
            timer: Some(Timer::new()),
        }));

        Ok(())
    }

    pub fn remove_system(&mut self, name: &str, update_phase: UpdatePhase) -> Result<()> {
        let phase_str = format!("{}", update_phase);
        let col = self.phase_systems_mut(&update_phase)?;

        if !col.iter().any(|(k, _)| k == name) {
            return Err(EngineError::SystemNotFound(
                name.to_string(),
                phase_str,
            ).into());
        }

        col.retain(|(k, _)| k != name);
        Ok(())
    }

    pub fn toggle_system(
        &mut self,
        name: &str,
        update_phase: UpdatePhase,
        enabled: bool,
    ) -> Result<()> {
        let system = self.get_system(name, update_phase)?;
        system.enabled = enabled;
        Ok(())
    }

    pub fn get_system_timer(
        &mut self,
        name: &str,
        update_phase: UpdatePhase,
    ) -> Result<Option<Timer>> {
        let system: &mut System = self.get_system(name, update_phase)?;
        Ok(system.timer.take())
    }

    pub fn update_system_timer(
        &mut self,
        name: &str,
        update_phase: UpdatePhase,
        timer: Timer,
    ) -> Result<()> {
        let system = self.get_system(name, update_phase)?;
        system.timer = Some(timer);
        Ok(())
    }
}
