use anyhow::{Context, Result, Error};
use thiserror::Error;
use colored::*;
use crate::utils::PillStyle;

#[derive(Error, Debug)]
pub enum EngineError {

    // Scene
    #[error("There is no active {} set \n\nSource: ", "Scene".gobj_style())]
    NoActiveScene,
    #[error("{} for that {} not found \n\nSource: ", "Scene".gobj_style(), "SceneHandle".sobj_style())]
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
    #[error("Update phase {} not found \n\nSource: ", .0.name_style())]
    SystemUpdatePhaseNotFound(String),
    
    // Resource
    #[error("Path to {} is invalid: {} \n\nSource: ", "Asset".gobj_style(), .0.name_style())]
    InvalidAssetPath(String),
    #[error("{} format is not supported. Expected .{} but is .{} \n\nSource: ", "Asset".gobj_style(), .0.name_style(), .1.name_style())]
    InvalidAssetFormat(String, String),
    #[error("{} {} is not registered \n\nSource: ", "Resource".gobj_style(), .0.sobj_style())]
    ResourceNotRegistered(String),
    #[error("{} {} {} already exists \n\nSource: ", "Resource".gobj_style(), .0.sobj_style(), .1.name_style())]
    ResourceAlreadyExists(String, String),
    #[error("{} {} for that {} not found \n\nSource: ", "Resource".gobj_style(), .0.sobj_style(), "Handle".sobj_style())]
    InvalidResourceHandle(String),
    #[error("{} {} of type {} not found \n\nSource: ", "Resource".gobj_style(), .0.name_style(), .1.sobj_style(),)]
    InvalidResourceName(String, String),
    #[error("Invalid .obj file {}\nFiles with multiple meshes are not supported \n\nSource: ", .0.name_style())]
    InvalidModelFile(String),
    #[error("Cannot remove default {} {} \n\nSource: ", "Resource".gobj_style(), .0.name_style())]
    RemoveDefaultResource(String),
    #[error("Cannot add {} with name {}. This name is reserved only for default engine resources \n\nSource: ", "Resource".gobj_style(), .0.name_style())]
    WrongResourceName(String),


    // Material textures and parameters
    #[error("Cannot set {} to {}. Accepted range is {} \n\nSource: ", "RenderingOrder".sobj_style(), .0.name_style(), .1.name_style())]
    WrongRenderingOrder(String, String),
    #[error("Cannot set {} of type {} to slot of type {} \n\nSource: ", "Texture".sobj_style(), .0.name_style(), .1.name_style())]
    WrongTextureType(String, String),
    #[error("{} {} of type {} does not exist \n\nSource: ", "MaterialParameter".sobj_style(), .0.name_style(), .1.sobj_style())]
    MaterialParameterNotFound(String, String),
    #[error("{} {} does not exist \n\nSource: ", "MaterialTexture".sobj_style(), .0.name_style())]
    MaterialTextureNotFound(String),


    
}

pub fn err_prefix() -> ColoredString {
    "\nERROR".err_style()
}
