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
    SystemNotFound(String, String),
    #[error("Component {0} is not registered for scene {1} \n\nSource: ")]
    ComponentNotRegistered(String, String),
    #[error("Resource {0} is not registered in Engine \n\nSource: ")]
    ResourceNotRegistered(String),

    #[error("Resource of type {0} and name {1} already exists \n\nSource: ")]
    ResourceAlreadyExists(String, String),

    #[error("Resource of type {0} for that handle not found \n\nSource: ")]
    InvalidResourceHandle(String),

    #[error("Invalid .obj file {0}\nFiles with multiple meshes are not supported \n\nSource: ")]
    InvalidModelFile(String)
}