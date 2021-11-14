use anyhow::{Context, Result, Error};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("There is no active scene set in the engine \n\nSource: ")]
    NoActiveScene,
    #[error("Scene for that handle not found \n\nSource: ")]
    InvalidSceneHandle,
    #[error("Scene with this name already exists \n\nSource: ")]
    SceneAlreadyExists,
    #[error("Component {0} is already registered for scene {1} \n\nSource: ")]
    ComponentAlreadyRegistered(String, String),
    #[error("System with name {0} is already registered for update phase {1} \n\nSource: ")]
    SystemAlreadyExists(String, String),
    #[error("System with name {0} is not registered for update phase {1} \n\nSource: ")]
    SystemNotFound(String, String)

}