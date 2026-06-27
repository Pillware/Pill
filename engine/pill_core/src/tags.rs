//! Tags module — ECS component tags for renderer dispatch (work in progress).
//!
//! This module is being refactored as part of the ECS rework (rework_ecs branch).
//! Currently contains minimal stubs so the crate compiles — full implementation pending.

// ---------------------------------------------------------------------------
// Renderer tag types (placeholder — re-export from old locations or define stubs)
// ---------------------------------------------------------------------------

/// Placeholder tag for GPU buffer resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RendererBufferTag;

/// Placeholder tag for camera components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RendererCameraTag;

/// Placeholder tag for material components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RendererMaterialTag;

/// Placeholder tag for mesh components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RendererMeshTag;

/// Placeholder tag for pipeline objects (legacy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RendererPipelineTag;

/// Placeholder tag for pipeline objects (v2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RendererPipelineV2Tag;

/// Placeholder tag for texture resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RendererTextureTag;
