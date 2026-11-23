//! The wgpu renderer: the GPU work that turns the world's renderer data into
//! frames.
//!
//! # Responsibilities
//!
//! - Own the GPU half: the device and surface lifecycle ([`Renderer`]), the
//!   [`resources`] the renderer keeps, the [`drawers`], the [`render_queue`]
//!   keys that order a frame's draws, and the [`ui`] painter that draws the
//!   frame's `pill_ui_frame` lists.
//! - Fill each frame from the world ([`frame`], the `rendering` system).
//! - Install the renderer on an engine through [`register`], which adds that
//!   system. The components and assets are not registered here:
//!   `pill_rendering` registers them itself.
//!
//! # Design
//!
//! The data a renderer draws with - assets, components, the default pipelines
//! and their WGSL - lives in `pill_rendering`; the contract between
//! host and renderer - the frame, the backend trait, the camera - lives in
//! `pill_renderer_api`. Neither links wgpu, and both are re-exported here.
//! This crate is the only renderer crate that links wgpu, which is what lets a
//! development host load and reload it as a module while everything around it
//! compiles against data only.

/// The mesh drawer: batches the instances a frame queues and records the
/// instanced draws they become.
///
/// Crate-private: `renderer.rs` is its only consumer, and other crates reach
/// the renderer through its extension ABI, not through these helpers.
pub(crate) mod drawers;

/// Renderer failures, re-exported from the contract crate, and the capture
/// that turns wgpu's errors into them.
pub mod error;

/// The system that turns the world into the frame the renderer draws.
pub mod frame;

/// The per-instance transform the drawer uploads for the vertex shader.
pub mod instance;

/// Holds the frame's chain as GPU objects and the plan each frame is recorded
/// from.
mod chain;

/// The asset preview path: one small off-screen render, read back as pixels.
mod preview;

/// Debug drawing, gizmos, selection outlines, picking and the scene grid,
/// drawn over each surface.
mod overlay;

/// The query sets and readbacks behind GPU profiling measurements.
pub mod profiler;

/// Hardware ray tracing: whether this build, this machine and this run can
/// trace rays, and the device features the path asks for.
mod ray_tracing;

/// Mapping a GPU buffer back to the CPU without stalling a frame.
mod readback;

/// One queued draw per frame instance, and the sort key that puts draws
/// sharing a shader, material and mesh next to each other.
pub mod render_queue;

/// The wgpu renderer: the surfaces, the GPU caches and the frame submission.
pub mod renderer;

/// The GPU objects that mirror the asset store, refreshed one asset type at a
/// time.
mod rendering_resources_manager;

/// One module per resource the renderer keeps, re-exported flat so callers
/// name them without the path.
pub mod resources;

/// A GPU buffer rewritten every surface, shared by the overlay and the UI
/// painter.
mod stream_buffer;

/// The window surface, its swapchain, and the lifecycle that keeps them live.
mod surface;

/// The UI painter: draws the frame's `pill_ui_frame` lists over each surface.
mod ui;

/// The C ABI a host drives this renderer through when it loads it as a module.
///
/// Public so the wrapper crate's expansion of `__pill_renderer_entry_points!`
/// can call into it; the exported symbols come from that macro.
#[cfg(not(target_arch = "wasm32"))]
pub mod module_entry;

/// The pipelines this renderer ships, their shaders, and the layout facts they
/// share with a project's own pipelines.
pub mod config;

/// Development shader reload: puts the WGSL the host re-cooked into the shader
/// assets built from that source.
#[cfg(any(test, all(debug_assertions, not(target_arch = "wasm32"))))]
pub mod shader_hot_reload;

// Standard library
use std::path::Path;
// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{pill_module, AssetManager, Engine, World};

// The contract and the data crates are re-exported here - their items and
// their module names - so this crate's own paths (`crate::api`,
// `crate::assets`, `crate::components`) resolve.
pub use pill_renderer_api::api;
pub use pill_renderer_api::{
    FrameOutcome, HeadlessRenderer, PillRenderer, RawWindowData, RenderCapabilities, RenderMetrics,
};
pub use pill_rendering::components::*;
pub use pill_rendering::{assets, components};
pub use pill_rendering::{
    AssetLoader, Material, MaterialBuilder, MaterialParameter, Mesh, MeshVertex, PassKind,
    PassTarget, PipelinePass, RenderPass, RenderingPipeline, Shader, ShaderBuilder,
    ShaderParameterSlot, ShaderParameterType, ShaderTextureSlot, Texture, TextureType,
};

// Current crate
pub use error::RendererError;
pub use frame::{rendering_system, RenderFrame, RenderInstance, ResolvedPass};
pub use instance::Instance;
pub use renderer::Renderer;
pub use resources::RenderingManager;

// =============================================================================
// Constants
// =============================================================================

/// The name the host knows this module by, which is its crate name.
const MODULE_NAME: &str = env!("CARGO_PKG_NAME");

// =============================================================================
// Free Functions
// =============================================================================

/// The registration half: declares this renderer's own assets and installs its
/// default pipeline, then returns zero.
///
/// This is the half a reload repeats. The post-update `rendering` system needs
/// no call of its own: [`frame::rendering_system`] declares itself, so the
/// engine registers it under the module's own owner before this function runs,
/// and the next generation registers its own after the reload removed the
/// previous one.
///
/// It registers no components or asset types: `pill_rendering`'s own `register`
/// declares them before this module loads, and doing it here would point the
/// asset types' tables at this image, which reloads most often. What it declares
/// is what belongs to drawing - the shaders its chain is built from - and the
/// default chain that names them, so a project that never declares a frame still
/// draws one.
pub fn register(engine: &mut Engine) -> u32 {
    // Step 1: Declare this crate's own `res`, the shader sources the store
    // answers by path. A file that went missing stops the engine here rather
    // than leaving the chain unbuilt.
    declare_crate_roots();

    // Step 2: Install the pipeline the renderer runs when a project declares
    // none. A project that sets its own pipeline does so after this call, so
    // its choice wins.
    if engine
        .world()
        .get_resource::<RenderingManager>()
        .is_some_and(|rendering_manager| rendering_manager.pipeline().is_none())
    {
        install_default_pbr_pipeline(engine.world_mut());
    }
    0
}

/// Declares the read-only roots this renderer's content lives under.
///
/// This crate's own `res` holds the shader sources the chain's shaders are
/// cooked from. The `.wgsl` beside each source is this crate's build product:
/// the store answers it because a shader's stages are read from it by path, and
/// the editor's tree leaves it out so a person sees only what they author. The
/// materials those shaders draw with are `pill_rendering`'s, so its root is
/// declared here too.
///
/// Declaring the same root again is what a reload does, and it changes
/// nothing. It is also what lets this crate's own tests read the same files a
/// running module reads.
///
/// # Panics
///
/// Panics when `res` exists but one of the files this function names below it
/// is missing; the message names the module and the path.
fn declare_crate_roots() {
    // The chain draws its default material from `pill_rendering`'s `res`, so
    // that root is part of this renderer's content as well. A running process
    // usually has it already, from the data module's own registration; a test
    // and a statically linked build do not.
    pill_rendering::declare_crate_roots();

    let resources_directory = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/res"));
    if resources_directory.is_dir() {
        pill_engine::asset_store::declare_module_root_hiding(
            MODULE_NAME,
            resources_directory,
            &[
                "shaders/common_shaders/default_vertex.hlsl",
                "shaders/common_shaders/include/common.hlsl",
                "shaders/common_shaders/include/lit_common.hlsl",
                "shaders/pbr_pipeline/include/pbr_surface.hlsl",
                "shaders/pbr_pipeline/pbr_fragment.hlsl",
                "shaders/pbr_pipeline/pbr_fragment.hlsl.meta",
                "shaders/post_processing/bloom_composite_fragment.hlsl",
                "shaders/post_processing/bloom_composite_fragment.hlsl.meta",
                "shaders/post_processing/bloom_prefilter_fragment.hlsl",
                "shaders/post_processing/bloom_prefilter_fragment.hlsl.meta",
                "shaders/post_processing/fullscreen_vertex.hlsl",
                "shaders/post_processing/lens_fragment.hlsl",
                "shaders/post_processing/lens_fragment.hlsl.meta",
                "shaders/post_processing/tonemap_fragment.hlsl",
                "shaders/post_processing/tonemap_fragment.hlsl.meta",
                "shaders/simple_pipeline/default_lit_fragment.hlsl",
                "shaders/simple_pipeline/default_lit_fragment.hlsl.meta",
                "shaders/skybox/include/skybox_common.hlsl",
                "shaders/skybox/skybox_cubemap_fragment.hlsl",
                "shaders/skybox/skybox_cubemap_fragment.hlsl.meta",
                "shaders/skybox/skybox_equirect_fragment.hlsl",
                "shaders/skybox/skybox_equirect_fragment.hlsl.meta",
                "shaders/skybox/skybox_vertex.hlsl",
            ],
            &["wgsl"],
        );
    }
}

/// Installs [`config::pbr_pipeline`] and points the manager at it.
///
/// A failure is logged and not returned: a renderer that could not install the
/// default pipeline still runs through its own built-in chain, and the log says
/// what went wrong. Three failures reach here: a name clash with an asset the
/// project already owns, the default material's file failing to parse or load,
/// and one of our shaders failing to build.
fn install_default_pbr_pipeline(world: &mut World) {
    let pipeline_install = match world.get_resource_mut::<AssetManager>() {
        Some(assets) => config::pbr_pipeline::install(assets),
        // No store to install into: nothing can be drawn anyway.
        None => return,
    };

    match pipeline_install {
        Ok(pipeline) => {
            if let Some(rendering_manager) = world.get_resource_mut::<RenderingManager>() {
                rendering_manager.set_pipeline(pipeline);
            }
        }
        Err(error) => pill_core::warn!(
            target: telemetry_target::RENDERING,
            "the default PBR pipeline was not installed: {error}"
        ),
    }
}

/// The artifact's one-time entry point: nothing is one-time here.
///
/// Everything the module owns is registration that [`register`] repeats on
/// every generation. This function exists because a host that links the crate
/// statically loads it once and calls this entry point alone, and because the
/// generated `host_module_*` wrapper carries it as `pill_module_init` by
/// expanding `__pill_module_entry_points!`.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

/// Starts building a renderer backend on a window, for a host that links this
/// crate statically - the shipping posture, where there is no module to load.
///
/// Returns a future because creating the device is asynchronous: a native
/// frontend blocks on it once, and a web frontend awaits it. A host that loads
/// this crate as a module reaches the same renderer through the blocking
/// `pill_renderer_attach` export instead.
///
/// # Errors
///
/// The future fails with a [`RendererError`] when building the renderer fails:
/// acquiring the surface or the adapter, creating the device, or creating an
/// object built after them - the surface configuration, the depth buffer, the
/// resource storage, the default material or the camera.
///
/// # Safety
///
/// `window` must name a live window that outlives the returned renderer.
pub unsafe fn attach(
    window: pill_renderer_api::RawWindowData,
    width: u32,
    height: u32,
) -> pill_renderer_api::AttachFuture {
    Box::pin(async move {
        // SAFETY: forwarded from this function's own contract, which covers
        // the future as much as the renderer it resolves to.
        let renderer = unsafe { Renderer::new_async(window, width, height) }.await?;
        Ok(Box::new(renderer) as Box<dyn PillRenderer>)
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::{AssetManager, Engine, Handle};

    /// An engine with the data crate registered, the way a host loads it: the
    /// data module comes first, and the renderer fills what it leaves open.
    fn engine_with_the_data_registered() -> Engine {
        let mut engine = Engine::new();
        assert_eq!(pill_rendering::register(&mut engine), 0);
        engine
    }

    /// The pipeline the manager points at.
    fn installed_pipeline(engine: &Engine) -> Handle<RenderingPipeline> {
        engine
            .world()
            .get_resource::<RenderingManager>()
            .expect("the data registers the manager")
            .pipeline()
            .expect("register points the manager at a pipeline")
    }

    /// The artifact entry point registers the declared `rendering` system,
    /// and the reload half declares nothing, so a second registration cannot
    /// leave a second copy.
    #[test]
    fn registering_the_renderer_adds_the_rendering_system_once() {
        let mut engine = Engine::new();

        assert_eq!(
            pill_engine::component_registry::run_artifact_registration(&mut engine, register),
            0
        );
        assert_eq!(register(&mut engine), 0);

        let registered = engine
            .system_snapshots()
            .iter()
            .filter(|snapshot| snapshot.name == "rendering")
            .count();
        assert_eq!(registered, 1, "the rendering system is registered once");
    }

    /// Registering installs the PBR chain and points the manager at it, so a
    /// project that declares no pipeline still draws.
    #[test]
    fn registering_installs_the_pbr_pipeline_as_the_default_frame() {
        let mut engine = engine_with_the_data_registered();

        assert_eq!(register(&mut engine), 0);

        let pipeline = installed_pipeline(&engine);
        let assets = engine
            .world()
            .get_resource::<AssetManager>()
            .expect("the engine inserts the store");
        assert_eq!(
            assets.handle_by_name::<RenderingPipeline>(config::pbr_pipeline::PIPELINE_NAME),
            Some(pipeline)
        );
    }

    /// A reload calls `register` again on a store that already holds the chain.
    #[test]
    fn installing_the_chain_twice_returns_the_same_assets() {
        let mut engine = engine_with_the_data_registered();
        register(&mut engine);
        let pipeline_before_reload = installed_pipeline(&engine);

        assert_eq!(register(&mut engine), 0);

        assert_eq!(installed_pipeline(&engine), pipeline_before_reload);
    }

    /// A pipeline a project sets survives a later `register`: the default only
    /// fills a manager that has none.
    #[test]
    fn a_pipeline_a_project_set_survives_a_later_register() {
        let mut engine = engine_with_the_data_registered();
        register(&mut engine);
        let project_pipeline = {
            let assets = engine
                .world_mut()
                .get_resource_mut::<AssetManager>()
                .expect("the engine inserts the store");
            config::simple_pipeline::install(assets).expect("a free name")
        };
        engine
            .world_mut()
            .get_resource_mut::<RenderingManager>()
            .expect("the data registers the manager")
            .set_pipeline(project_pipeline);

        register(&mut engine);

        assert_eq!(installed_pipeline(&engine), project_pipeline);
    }
}
