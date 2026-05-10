// WHY: zero-size marker types so Handle<RendererMeshTag> and Handle<RendererTextureTag> are different types — passing the wrong one is a compile error, not a runtime bug.

pub struct RendererBufferTag;
pub struct RendererCameraTag;
pub struct RendererMaterialTag;
pub struct RendererMeshTag;
pub struct RendererPipelineTag;
pub struct RendererPipelineV2Tag;
pub struct RendererTextureTag;
