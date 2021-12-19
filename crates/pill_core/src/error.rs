use anyhow::{Context, Result, Error};
use thiserror::Error;
use colored::*;
use crate::utils::PillStyle;

#[derive(Error, Debug)]
pub enum EngineError {

    // Scene
    #[error("There is no active {} set \n\nSource: ", "Scene".gobj_style())]
    NoActiveScene,
    #[error("{} for that handle not found \n\nSource: ", "Scene".gobj_style())]
    InvalidSceneHandle,
    #[error("{} {} already exists \n\nSource: ", "Scene".gobj_style(), .0.name_style())]
    SceneAlreadyExists(String),
    #[error("{} {} does not exist \n\nSource: ", "Scene".gobj_style(), .0.name_style())]
    InvalidSceneName(String),

    // Camera
    #[error("There is no active {} set in active {} \n\nSource: ",  "Camera".gobj_style(), "Scene".gobj_style())]
    NoActiveCamera,

    // Component
    #[error("{} {} is already registered for {} {} \n\nSource: ", "Component".gobj_style(), .0.sobj_style(), "Scene".gobj_style(), .1.name_style())]
    ComponentAlreadyRegistered(String, String),
    #[error("{} {} is not registered for {} {} \n\nSource: ", "Component".gobj_style(), .0.sobj_style(), "Scene".gobj_style(), .1.name_style())]
    ComponentNotRegistered(String, String),


    // System
    #[error("Failed to update {} {} in update phase {} \n\nSource: ", "System".gobj_style(), .0.sobj_style(), .1.sobj_style())]
    SystemUpdateFailed(String, String),
    #[error("{} {} is already registered for update phase {} \n\nSource: ", "System".gobj_style(), .0.name_style(), .1.name_style())]
    SystemAlreadyExists(String, String),
    #[error("{} {} is not registered for update phase {} \n\nSource: ", "System".gobj_style(), .0.name_style(), .1.name_style())]
    SystemNotFound(String, String),
    #[error("{} update phase {} not found \n\nSource: ", "System".gobj_style(), .0.name_style())]
    SystemUpdatePhaseNotFound(String),
    
    // Resource
    #[error("{} {} is not registered \n\nSource: ", "Resource".gobj_style(), .0.sobj_style())]
    ResourceNotRegistered(String),
    #[error("{} {} {} already exists \n\nSource: ", "Resource".gobj_style(), .0.sobj_style(), .1.name_style())]
    ResourceAlreadyExists(String, String),
    #[error("{} {} for that handle not found \n\nSource: ", "Resource".gobj_style(), .0.sobj_style())]
    InvalidResourceHandle(String),
    #[error("Invalid .obj file {}\nFiles with multiple meshes are not supported \n\nSource: ", .0.name_style())]
    InvalidModelFile(String)
}

pub fn err_prefix() -> ColoredString {
    "\nERROR".err_style()
}
