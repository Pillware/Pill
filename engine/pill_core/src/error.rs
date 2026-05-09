use crate::style::PillStyle;

use thiserror::Error;

// --- Core error ---
#[derive(Error, Debug, Clone)]
pub enum CoreError {
    #[error("{} version_limit has to be a power of 2\n\nSource: ", "Core".general_object_style())]
    NotMultipleOf2,
    #[error("{} Undefined error", "Core".general_object_style())]
    Other,
}

// --- Renderer error ---

#[derive(Error, Debug, Clone)]
pub enum RendererError {
    #[error("Undefined {} error \n\nSource: ", "Renderer".general_object_style())]
    Other,
    #[error("{} {} not found \n\nSource: ", "Renderer".general_object_style(), "Resource".specific_object_style())]
    RendererResourceNotFound,
    #[error("{} {} lost \n\nSource: ", "Renderer".general_object_style(), "Surface".specific_object_style())]
    SurfaceLost,
    #[error("{} {} out of memory \n\nSource: ", "Renderer".general_object_style(), "Surface".specific_object_style())]
    SurfaceOutOfMemory,
    #[error("Undefined {} {} error \n\nSource: ", "Renderer".general_object_style(), "Surface".specific_object_style())]
    SurfaceOther,
}

#[derive(Error, Debug, Clone)]
pub enum EngineError<'a> {
    // Config
    #[error("Invalid {} config file", "Game".module_object_style())]
    InvalidGameConfig(),

    // Scene
    #[error("There is no active {} set", "Scene".general_object_style())]
    NoActiveScene,
    #[error("{} for that {} not found", "Scene".general_object_style(), "SceneHandle".specific_object_style())]
    InvalidSceneHandle,
    #[error("{} {} already exists", "Scene".general_object_style(), .0.name_style())]
    SceneAlreadyExists(String),
    #[error("{} {} does not exist", "Scene".general_object_style(), .0.name_style())]
    InvalidSceneName(String),

    // Entity
    #[error("{} for that {} not found", "Entity".general_object_style(), "EntityHandle".specific_object_style())]
    InvalidEntityHandle,
    #[error("Cannot create {}. Maximum number of entities in {} reached. \n\nSource: ", "Entity".general_object_style(), "Scene".general_object_style())]
    EntityLimitReached,

    // Camera
    #[error("There is no active {} set in active {}",  "Camera".general_object_style(), "Scene".general_object_style())]
    NoActiveCamera,

    // Component
    #[error("{} {} is already registered for {} {}", "Component".general_object_style(), .0.specific_object_style(), "Scene".general_object_style(), .1.name_style())]
    ComponentAlreadyRegistered(String, String),
    #[error("{} {} is not registered for {} {}", "Component".general_object_style(), .0.specific_object_style(), "Scene".general_object_style(), .1.name_style())]
    ComponentNotRegistered(String, String),
    #[error("{} {} is already added to {}", "Component".general_object_style(), .0.specific_object_style(), "Entity".module_object_style())]
    ComponentAlreadyExists(String),
    #[error("{} {} is already added to {}", "GlobalComponent".general_object_style(), .0.specific_object_style(), "Engine".module_object_style())]
    GlobalComponentAlreadyExists(String),
    #[error("{} {} not found in {}", "GlobalComponent".general_object_style(), .0.specific_object_style(), "Engine".module_object_style())]
    GlobalComponentNotFound(String),
    #[error("{} is {} that cannot be removed", .0.specific_object_style(), "GlobalComponent".general_object_style())]
    GlobalComponentCannotBeRemoved(String),

    // System
    #[error("Failed to update {} {} in {} {}", "System".general_object_style(), .0.specific_object_style(), "UpdatePhase".specific_object_style(), .1.name_style())]
    SystemUpdateFailed(String, String),
    #[error("{} {} is already registered for {} {}", "System".general_object_style(), .0.name_style(), "UpdatePhase".specific_object_style(), .1.name_style())]
    SystemAlreadyExists(String, String),
    #[error("{} {} is not registered for {} {}", "System".general_object_style(), .0.name_style(), "UpdatePhase".specific_object_style(), .1.name_style())]
    SystemNotFound(String, String),
    #[error("{} {} not found", "UpdatePhase".specific_object_style(), .0.name_style())]
    SystemUpdatePhaseNotFound(String),

    // Resource
    #[error("Path to {} is invalid: {}", "Asset".general_object_style(), .0.name_style())]
    InvalidAssetPath(String),
    #[error("{} format is not supported. Expected one of: {:?} but is .{}", "Asset".general_object_style(), .0, .1.name_style())]
    InvalidAssetFormat(&'a [&'a str], String),
    #[error("{} {} is not registered", "Resource".general_object_style(), .0.specific_object_style())]
    ResourceNotRegistered(String),
    #[error("{} {} {} already exists", "Resource".general_object_style(), .0.specific_object_style(), .1.name_style())]
    ResourceAlreadyExists(String, String),
    #[error("{} {} for that {} not found", "Resource".general_object_style(), .0.specific_object_style(), "Handle".specific_object_style())]
    InvalidResourceHandle(String),
    #[error("{} {} of type {} not found", "Resource".general_object_style(), .0.name_style(), .1.specific_object_style(),)]
    InvalidResourceName(String, String),
    #[error("Invalid .obj file {}", .0.name_style())]
    InvalidModelFile(String),
    #[error("Invalid .obj file {}\nFiles with multiple meshes are not supported", .0.name_style())]
    InvalidModelFileMultipleMeshes(String),
    #[error("Cannot remove default {} {}", "Resource".general_object_style(), .0.name_style())]
    RemoveDefaultResource(String),
    #[error("Cannot add {} with name {}. This name is reserved only for default engine resources", "Resource".general_object_style(), .0.name_style())]
    WrongResourceName(String),
    #[error("Cannot add {} {}. Maximum number of resources reached. \n\nSource: ", "Resource".general_object_style(), .0.specific_object_style())]
    ResourceLimitReached(String),

    // Material textures and parameters
    #[error("Cannot set {} to {}. Accepted range is {}", "RenderingOrder".specific_object_style(), .0.name_style(), .1.name_style())]
    WrongRenderingOrder(String, String),
    #[error("Cannot set {} of type {} to slot {} of type {}", "Texture".specific_object_style(), .0.name_style(), .1.name_style(), .2.name_style())]
    WrongTextureType(String, String, String),
    #[error("{} slot {} of type {} does not exist in {} {}", "MaterialParameter".specific_object_style(), .0.name_style(), .1.specific_object_style(), "Material".specific_object_style(), .2.name_style())]
    MaterialParameterSlotNotFound(String, String, String),
    #[error("{} slot {} does not exist", "MaterialTexture".specific_object_style(), .0.name_style())]
    MaterialTextureSlotNotFound(String),
    #[error("Invalid {} for {} in slot {}", "Handle".specific_object_style(), "Texture".specific_object_style(), .0.name_style())]
    InvalidTextureHandleForSlot(String),

    // Timer
    #[error("Timer context {} is invalid", .0.name_style())]
    InvalidTimerContext(String),
    #[error("System timer was used in the system {} but it wasn't returned using update_system_timer function", .0.name_style())]
    NonReturnedSystemTimer(String),
    #[error("There is no active timer context to end")]
    NoTimerContextToEnd(),

    // Network
    #[error("Invalid NetworkAction byte: {}", .0)]
    InvalidNetworkAction(u8),
    #[error("Connection not stable yet")]
    ConnectionNotStable,

    // Other
    #[error("{} error: {}", "Engine".module_object_style(), .0)]
    Other(String),
}

pub fn err_prefix() -> String {
    "\nERROR".error_style()
}
