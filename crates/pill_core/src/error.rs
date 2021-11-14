use anyhow::{Context, Result, Error};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("There is no current scene set in the engine \n\nSource: ")]
    CurrentSceneNotSet,
    #[error("Scene for that handle not found \n\nSource: ")]
    InvalidSceneHandle,
    #[error("Scene with this name already exists \n\nSource: ")]
    SceneWithThisNameAlreadyExists,
    #[error("Component {0} is already registered for scene {1} \n\nSource: ")]
    ComponentAlreadyRegistered(String, String),
}