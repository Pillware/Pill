use crate::utils::PillStyle;

use anyhow::{Context, Result, Error};
use thiserror::Error;
use colored::*;


#[derive(Error, Debug)]
pub enum EngineError<'a> {

    // Config
    #[error("Invalid {} config file \n\nSource: ", "Game".mobj_style())]
    InvalidGameConfig(),

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
    #[error("{} {} is already added to {} \n\nSource: ", "GlobalComponent".gobj_style(), .0.sobj_style(), "Engine".mobj_style())]
    GlobalComponentAlreadyExists(String),
    #[error("{} {} not found in {} \n\nSource: ", "GlobalComponent".gobj_style(), .0.sobj_style(), "Engine".mobj_style())]
    GlobalComponentNotFound(String),
    #[error("{} is {} that cannot be removed \n\nSource: ", .0.sobj_style(), "GlobalComponent".gobj_style())]
    GlobalComponentCannotBeRemoved(String),

    // System
    #[error("Failed to update {} {} in {} {} \n\nSource: ", "System".gobj_style(), .0.sobj_style(), "UpdatePhase".sobj_style(), .1.name_style())]
    SystemUpdateFailed(String, String),
    #[error("{} {} is already registered for {} {} \n\nSource: ", "System".gobj_style(), .0.name_style(), "UpdatePhase".sobj_style(), .1.name_style())]
    SystemAlreadyExists(String, String),
    #[error("{} {} is not registered for {} {} \n\nSource: ", "System".gobj_style(), .0.name_style(), "UpdatePhase".sobj_style(), .1.name_style())]
    SystemNotFound(String, String),
    #[error("{} {} not found \n\nSource: ", "UpdatePhase".sobj_style(), .0.name_style())]
    SystemUpdatePhaseNotFound(String),
    
    // Resource
    #[error("Path to {} is invalid: {} \n\nSource: ", "Asset".gobj_style(), .0.name_style())]
    InvalidAssetPath(String),
    #[error("{} format is not supported. Expected one of: {:?} but is .{} \n\nSource: ", "Asset".gobj_style(), .0, .1.name_style())]
    InvalidAssetFormat(&'a [&'a str], String),
    #[error("{} {} is not registered \n\nSource: ", "Resource".gobj_style(), .0.sobj_style())]
    ResourceNotRegistered(String),
    #[error("{} {} {} already exists \n\nSource: ", "Resource".gobj_style(), .0.sobj_style(), .1.name_style())]
    ResourceAlreadyExists(String, String),
    #[error("{} {} for that {} not found \n\nSource: ", "Resource".gobj_style(), .0.sobj_style(), "Handle".sobj_style())]
    InvalidResourceHandle(String),
    #[error("{} {} of type {} not found \n\nSource: ", "Resource".gobj_style(), .0.name_style(), .1.sobj_style(),)]
    InvalidResourceName(String, String),
    #[error("Invalid .obj file {} \n\nSource: ", .0.name_style())]
    InvalidModelFile(String),
    #[error("Invalid .obj file {}\nFiles with multiple meshes are not supported \n\nSource: ", .0.name_style())]
    InvalidModelFileMultipleMeshes(String),
    #[error("Cannot remove default {} {} \n\nSource: ", "Resource".gobj_style(), .0.name_style())]
    RemoveDefaultResource(String),
    #[error("Cannot add {} with name {}. This name is reserved only for default engine resources \n\nSource: ", "Resource".gobj_style(), .0.name_style())]
    WrongResourceName(String),

    // Material textures and parameters
    #[error("Cannot set {} to {}. Accepted range is {} \n\nSource: ", "RenderingOrder".sobj_style(), .0.name_style(), .1.name_style())]
    WrongRenderingOrder(String, String),
    #[error("Cannot set {} of type {} to slot {} of type {} \n\nSource: ", "Texture".sobj_style(), .0.name_style(), .1.name_style(), .2.name_style())]
    WrongTextureType(String, String, String),
    #[error("{} slot {} of type {} does not exist \n\nSource: ", "MaterialParameter".sobj_style(), .0.name_style(), .1.sobj_style())]
    MaterialParameterSlotNotFound(String, String),
    #[error("{} slot {} does not exist \n\nSource: ", "MaterialTexture".sobj_style(), .0.name_style())]
    MaterialTextureSlotNotFound(String),

    // Other
    #[error("{} error: {} \n\nSource: ", "Engine".mobj_style(), .0)]
    Other(String)

}

pub fn err_prefix() -> ColoredString {
    "\nERROR".err_style()
}
