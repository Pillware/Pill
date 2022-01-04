#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

pub(crate) mod mesh_rendering_component;
pub(crate) mod transform_component;
pub(crate) mod camera_component;
pub(crate) mod deferred_update_component;
pub(crate) mod input_component;
pub(crate) mod time_component;

// --- Use ---

// pub use mesh_rendering_component::{
//     MeshRenderingComponent,
// };

// pub use transform_component::{
//     TransformComponent,
// };

// pub use camera_component::{
//     CameraComponent,
//     CameraAspectRatio,
//     get_renderer_resource_handle_from_camera_component,
// };

// pub use deferred_update_component::{
//     DeferredUpdateComponent,
//     DeferredUpdateManager,
//     DeferredUpdateManagerPointer,
//     DeferredUpdateRequest,
//     DeferredUpdateComponentRequest,
//     DeferredUpdateResourceRequest
// };

// pub use input_component::{
//     InputComponent,
// };