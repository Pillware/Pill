//! The renderer contract: what the host hands any renderer and what every
//! renderer hands back, with no graphics library behind it.
//!
//! # Responsibilities
//!
//! - Define the backend contract a frontend drives ([`PillRenderer`]) and the
//!   values it reports ([`FrameOutcome`], [`RenderCapabilities`],
//!   [`RenderMetrics`]), plus the failures a renderer raises
//!   ([`RendererError`]).
//! - Define the frame a renderer draws ([`RenderFrame`]) and the vocabulary
//!   it is written in: pass kinds and targets, cull modes, material
//!   parameters.
//! - Define the camera component the frame embeds and the viewport rectangle
//!   a frontend points a renderer at, and register the components through
//!   [`register_contract_components`].
//! - Carry a window ([`RawWindowData`]), a surface set ([`SurfaceId`],
//!   [`SurfaceCameras`]) and the scene grid ([`editor::SceneGrid`], the frame's
//!   cell size) as plain data.
//! - Carry debug drawing between the world and any renderer ([`DebugDraw`]).
//! - Carry the editor's own inputs - its viewports and grid, the selection it
//!   outlines, picking and previews - between an editor and any renderer, inside
//!   [`editor`].
//!
//! # Design
//!
//! This crate never depends on wgpu or winit and holds nothing specific to one
//! renderer. A renderer comes as a data crate (assets, its own components,
//! pipelines, shaders) plus a GPU module, and both build on this contract. The
//! host links this crate in every posture, so a change here needs a host
//! restart. The types stay small and hold plain data.

/// The renderer contract a frontend drives, plus a headless stub.
pub mod api;

/// The contract's camera component and viewport rectangle, and
/// [`register_contract_components`].
pub mod components;

/// Debug drawing: lines, shapes, labels and gizmo handles a system asks a
/// renderer to draw.
pub mod debug_draw;

/// What exists because an editor asks for it: its own views and grid, the
/// selection it outlines, picking and asset previews.
pub mod editor;

/// Renderer failures, the [`error::Result`] alias and the
/// [`error::ErrorContext`] helper.
pub mod error;

/// The frame a renderer draws: instances, resolved passes, the surfaces'
/// cameras and the clock.
pub mod frame;

/// Component gizmos: what a component declares about its own drawing, the
/// registry an editor reads, and the pass that runs them.
pub mod gizmos;

/// A window's platform handles as plain data, for attaching a renderer to it.
pub mod raw_window;

/// Several surfaces per renderer: identities, presentation, camera bindings.
pub mod surface;

/// Whether the frame's shadows are traced with hardware ray tracing.
pub mod shadows;

// Current crate
pub use api::{
    AttachFuture, FrameOutcome, HeadlessRenderer, PanelFrame, PillRenderer, RenderCapabilities,
    RenderMetrics,
};
pub use components::*;
pub use debug_draw::{
    DebugColor, DebugDepth, DebugDraw, DebugFrame, DebugLabel, DebugLifetime, DebugLine,
    DebugStyle, GizmoLine, GizmoPoint, GizmoPointShape, GizmoTriangle, HiddenLook, LabelAlign,
    LabelAnchor,
};
pub use error::RendererError;
pub use frame::{
    AppendPass, AssetKey, ChainSource, CullMode, MaterialParameter, PassKind, PassSettings,
    PassSurfaces, PassTarget, RenderFrame, RenderInstance, ResolvedPass, DEPTH_INPUT,
};
pub use gizmos::{
    GizmoContext, GizmoDeclaration, GizmoDraw, GizmoDrawEntry, GizmoDrawing, GizmoEntry,
    GizmoFrame, GizmoOverrideDeclaration, GizmoOverrideEntry, GizmoOverrideMode, GizmoRegistry,
    GizmoSelection, GizmoVisibility, GizmoWhen,
};
pub use raw_window::{RawWindowData, RawWindowKind};
pub use shadows::RayTracedShadows;
pub use surface::{
    resolve_camera_binding, CameraBinding, PresentPreference, SurfaceCameras, SurfaceId,
    SurfaceOptions, SurfaceView,
};

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::Resource;

    /// Every resource the contract defines pins its shared name exactly.
    ///
    /// The names are identities, not paths: the host and a renderer module
    /// resolve the same resource by them across the artifact boundary, so a
    /// "corrected" path would silently register a second resource and leave
    /// the first stale. A resource that was published under another name
    /// declares that name as a former one, which is how a stored document
    /// written then still reads.
    #[test]
    fn every_contract_resource_pins_its_shared_name() {
        assert_eq!(
            RenderFrame::shared_name(),
            Some("pill_rendering::RenderFrame")
        );
        assert_eq!(
            RenderFrame::former_shared_names(),
            ["pill_master_renderer::RenderFrame"]
        );
        assert_eq!(
            SurfaceCameras::shared_name(),
            Some("pill_renderer_api::SurfaceCameras")
        );
        assert_eq!(
            editor::SelectionOutlines::shared_name(),
            Some("pill_renderer_api::SelectionOutlines")
        );
        assert_eq!(
            editor::PickRequests::shared_name(),
            Some("pill_renderer_api::PickRequests")
        );
        assert_eq!(
            editor::PickResults::shared_name(),
            Some("pill_renderer_api::PickResults")
        );
        assert_eq!(
            editor::PreviewRequests::shared_name(),
            Some("pill_renderer_api::PreviewRequests")
        );
        assert_eq!(
            editor::PreviewResults::shared_name(),
            Some("pill_renderer_api::PreviewResults")
        );
        assert_eq!(
            editor::PreviewGeneration::shared_name(),
            Some("pill_renderer_api::PreviewGeneration")
        );
        assert_eq!(
            editor::DefaultShader::shared_name(),
            Some("pill_renderer_api::DefaultShader")
        );
        assert_eq!(
            DebugDraw::shared_name(),
            Some("pill_renderer_api::DebugDraw")
        );
        assert_eq!(
            GizmoRegistry::shared_name(),
            Some("pill_renderer_api::GizmoRegistry")
        );
        assert_eq!(
            GizmoSelection::shared_name(),
            Some("pill_renderer_api::GizmoSelection")
        );
    }
}
