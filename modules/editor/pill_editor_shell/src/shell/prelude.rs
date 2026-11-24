//! The names every shell module shares, re-exported in one place.
//!
//! # Responsibilities
//!
//! - Re-export the engine, editor-core, UI and JSON types the shell's modules
//!   name, so each module opens with two globs instead of a long import list.
//!
//! # Design
//!
//! A shell module writes
//! `use pill_editor_shell::shell::prelude::*;` and
//! `use pill_editor_shell::shell::*;`. Both spell the whole path
//! from the crate root. A generated patch copies the edited file's top-level
//! `use` lines into a small crate of its own, where a `super::`-relative
//! glob would have nothing to point at.

// External crates
pub use pill_core::telemetry::telemetry_target;
pub use pill_editor_core::assets::{self, AssetDialog};
pub use pill_editor_core::boundary::{
    frame_from_output, EguiShellFrame, EguiShellInput, ShellBadge, ShellMarquee, ShellRect,
    BADGES_PER_VIEW_MAX, DEFAULT_SCENE_VIEWS, SCENE_VIEWS_MAX,
};
pub use pill_editor_core::browser::{self, ProjectBrowser};
pub use pill_editor_core::commands::{
    CommandInvocation, CommandTarget, CommandView, PendingEditorCommands,
};
pub use pill_editor_core::console::{
    self as console, ConsoleChannel, ConsoleFilterSettings, ConsoleLine, ConsoleView,
};
pub use pill_editor_core::editor_state::{
    AssetFieldDecorator, AssetKind, AssetListing, AssetReferenceField, AssetReferenceLabel,
    ComponentView, EditorCommand, EditorSelection, EditorSnapshot, EngineAsset, EntityDetail,
    EntityListEntry, EntityReferenceLabel, ResourceFieldView, ResourceView,
};
pub use pill_editor_core::egui;
pub use pill_editor_core::egui_dock::{
    DockArea, DockState, NodeIndex, NodePath, SurfaceIndex, TabViewer,
};
pub use pill_editor_core::entities;
pub use pill_editor_core::gizmo::{GizmoMode, GizmoSpace};
pub use pill_editor_core::icons;
pub use pill_editor_core::inspector;
pub use pill_editor_core::kit;
pub use pill_editor_core::layout::EditorTab;
pub use pill_editor_core::menubar;
pub use pill_editor_core::notices::{EditorNotices, Notice, NoticeLevel, ProblemRow};
pub use pill_editor_core::panels::{
    PanelCommand, PanelComponentValues, PanelContext, PanelDrawFn, PanelEntityDetail,
    PanelEntityRow, PanelFieldValue, PanelRegistry, PanelWindowRequest,
};
pub use pill_editor_core::placements::{
    CommandDrawContext, CommandDraws, CommandState, CommandStates, UiSlot,
};
pub use pill_editor_core::preferences;
pub use pill_editor_core::project_settings;
pub use pill_editor_core::requests::{HistoryRequest, ShellRequests};
pub use pill_editor_core::session::{
    EguiShellState, InspectorSession, LoggedRects, LoggedTexts, MenubarSession, MenubarSlot,
    PaletteState, PanelBudget, PanelTiming, PanelWindowAnswer, PanelWindowSession,
    PanelWindowState, StatisticsSession, Toast, ToastSession, ToolboxSession, PANEL_BUDGET_FRAMES,
    PANEL_BUDGET_MICROS, PANEL_WINDOWS_MAX, TOASTS_MAX, TOAST_FRAMES,
};
pub use pill_editor_core::style;
pub use pill_editor_core::systems;
pub use pill_editor_core::toolbox;
pub use pill_editor_core::unsaved_work::{QuitAnswer, QuitPromptState, UnsavedItem, UnsavedWork};
pub use pill_engine::FieldDecorator;
pub use pill_engine::{pill_hot_fn, Entity, FieldValue, Input};
pub use serde_json::Value as JsonValue;
