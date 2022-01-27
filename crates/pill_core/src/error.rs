use crate::utils::PillStyle;

use anyhow::{Context, Result, Error};
use thiserror::Error;
use colored::*;


#[derive(Error, Debug, Clone)]
pub enum EngineError<'a> {

    // Config
    #[error("Invalid {} config file", "Game".mobj_style())]
    InvalidGameConfig(),

    // Scene
    #[error("There is no active {} set", "Scene".gobj_style())]
    NoActiveScene,
    #[error("{} for that {} not found", "Scene".gobj_style(), "SceneHandle".sobj_style())]
    InvalidSceneHandle,
    #[error("{} {} already exists", "Scene".gobj_style(), .0.name_style())]
    SceneAlreadyExists(String),
    #[error("{} {} does not exist", "Scene".gobj_style(), .0.name_style())]
    InvalidSceneName(String),

    #[error("{} for that {} not found", "Entity".gobj_style(), "EntityHandle".sobj_style())]
    InvalidEntityHandle,

    // Camera
    #[error("There is no active {} set in active {}",  "Camera".gobj_style(), "Scene".gobj_style())]
    NoActiveCamera,

    // Entity
    #[error("New {} cannot be created. Maximum number of entities in {} reached. \n\nSource: ", "Entity".gobj_style(), "Scene".gobj_style())]
    EntityMaximumCountReached,

    // Component
    #[error("{} {} is already registered for {} {}", "Component".gobj_style(), .0.sobj_style(), "Scene".gobj_style(), .1.name_style())]
    ComponentAlreadyRegistered(String, String),
    #[error("{} {} is not registered for {} {}", "Component".gobj_style(), .0.sobj_style(), "Scene".gobj_style(), .1.name_style())]
    ComponentNotRegistered(String, String),
    #[error("{} {} is already added to {}", "Component".gobj_style(), .0.sobj_style(), "Entity".mobj_style())]
    ComponentAlreadyExists(String),
    #[error("{} {} is already added to {}", "GlobalComponent".gobj_style(), .0.sobj_style(), "Engine".mobj_style())]
    GlobalComponentAlreadyExists(String),
    #[error("{} {} not found in {}", "GlobalComponent".gobj_style(), .0.sobj_style(), "Engine".mobj_style())]
    GlobalComponentNotFound(String),
    #[error("{} is {} that cannot be removed", .0.sobj_style(), "GlobalComponent".gobj_style())]
    GlobalComponentCannotBeRemoved(String),

    // System
    #[error("Failed to update {} {} in {} {}", "System".gobj_style(), .0.sobj_style(), "UpdatePhase".sobj_style(), .1.name_style())]
    SystemUpdateFailed(String, String),
    #[error("{} {} is already registered for {} {}", "System".gobj_style(), .0.name_style(), "UpdatePhase".sobj_style(), .1.name_style())]
    SystemAlreadyExists(String, String),
    #[error("{} {} is not registered for {} {}", "System".gobj_style(), .0.name_style(), "UpdatePhase".sobj_style(), .1.name_style())]
    SystemNotFound(String, String),
    #[error("{} {} not found", "UpdatePhase".sobj_style(), .0.name_style())]
    SystemUpdatePhaseNotFound(String),
    
    // Resource
    #[error("Path to {} is invalid: {}", "Asset".gobj_style(), .0.name_style())]
    InvalidAssetPath(String),
    #[error("{} format is not supported. Expected one of: {:?} but is .{}", "Asset".gobj_style(), .0, .1.name_style())]
    InvalidAssetFormat(&'a [&'a str], String),
    #[error("{} {} is not registered", "Resource".gobj_style(), .0.sobj_style())]
    ResourceNotRegistered(String),
    #[error("{} {} {} already exists", "Resource".gobj_style(), .0.sobj_style(), .1.name_style())]
    ResourceAlreadyExists(String, String),
    #[error("{} {} for that {} not found", "Resource".gobj_style(), .0.sobj_style(), "Handle".sobj_style())]
    InvalidResourceHandle(String),
    #[error("{} {} of type {} not found", "Resource".gobj_style(), .0.name_style(), .1.sobj_style(),)]
    InvalidResourceName(String, String),
    #[error("Invalid .obj file {}", .0.name_style())]
    InvalidModelFile(String),
    #[error("Invalid .obj file {}\nFiles with multiple meshes are not supported", .0.name_style())]
    InvalidModelFileMultipleMeshes(String),
    #[error("Cannot remove default {} {}", "Resource".gobj_style(), .0.name_style())]
    RemoveDefaultResource(String),
    #[error("Cannot add {} with name {}. This name is reserved only for default engine resources", "Resource".gobj_style(), .0.name_style())]
    WrongResourceName(String),
    #[error("New {} cannot be registered. Maximum number of resources reached. \n\nSource: ", "Resource".gobj_style())]
    ResourceMaximumCountReached,

    // Material textures and parameters
    #[error("Cannot set {} to {}. Accepted range is {}", "RenderingOrder".sobj_style(), .0.name_style(), .1.name_style())]
    WrongRenderingOrder(String, String),
    #[error("Cannot set {} of type {} to slot {} of type {}", "Texture".sobj_style(), .0.name_style(), .1.name_style(), .2.name_style())]
    WrongTextureType(String, String, String),
    #[error("{} slot {} of type {} does not exist", "MaterialParameter".sobj_style(), .0.name_style(), .1.sobj_style())]
    MaterialParameterSlotNotFound(String, String),
    #[error("{} slot {} does not exist", "MaterialTexture".sobj_style(), .0.name_style())]
    MaterialTextureSlotNotFound(String),

    // Other
    #[error("{} error: {}", "Engine".mobj_style(), .0)]
    Other(String)

}

pub fn err_prefix() -> ColoredString {
    "\nERROR".err_style()
}
