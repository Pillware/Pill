//! Two hundred thousand pills drift through a curl noise field. A free flying
//! camera watches them, and an in-game overlay tunes the field live.
//!
//! # Responsibilities
//!
//! - Own the crate root: `init` (registration, asset import, theme, systems),
//!   the modules, and the items the other files name through `pills_rust::`.
//!
//! # Design
//!
//! The pills are the whole scene: there is no level geometry and no sky. The
//! project ships one model, one map, one material, one font and a theme
//! override, so the loading code is a few calls in [`load_assets`] and in
//! `init`, not a module of its own.
//!
//! Every other file holds at least one `#[pill_hot]` system. The generated
//! fast patch builds a small crate from the edited file. It copies the file's
//! top-level imports and links this library. Those files therefore must not
//! name `crate::`: the components and the resource they share are re-exported
//! below and reached as `pills_rust::...`.
//!
//! A patch carries only the function's body. A system that calls a helper in
//! its own file, like the field's maths, cannot be patched that way and falls
//! back to a reload; the host reports it and reloads the project. The overlay
//! calls no helper of its own, so its edits patch in place.
//!
//! `init` runs again on every hot reload, on the same world. Every step it
//! takes can run twice without changing anything: imports hand back the assets
//! that are already loaded, the scene is only built into a world that does not
//! hold it yet, and the tuning resource is only inserted when an earlier
//! generation did not leave one behind.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use pill_engine::{
    pill_project, AssetImport, AssetLoader, AssetManager, Engine, Handle, MetadataPolicy, World,
};
use pill_master_renderer_data::{
    config::pbr_pipeline, Material, Mesh, Texture, TextureImportSettings, TextureType,
};

// The crate's own name, so a hot file can say `pills_rust::...` in its patch
// crate too, where `crate::` means nothing.
extern crate self as pills_rust;

mod camera;
mod curl_noise_system;
mod game;
mod overlay;

// The items the hot files name, so their patches can reach them through
// `pills_rust::...`.
pub use game::{CameraMovementComponent, CurlNoiseComponent, DemoState, PILL_COUNT};

/// Registers what the demo uses, loads its assets, installs the UI theme and
/// builds the scene.
#[pill_project]
pub fn init(engine: &mut Engine) -> u32 {
    // Registers the renderer's data and installs its PBR chain, which is where
    // the pill material's shader comes from.
    pill_master_renderer_data::register(engine);
    game::register(engine.world_mut());

    let (mesh, material) = match load_assets(engine.world_mut()) {
        Ok(assets) => assets,
        Err(error) => {
            pill_engine::tracing::error!("asset loading failed: {error}");
            return 1;
        }
    };
    if let Err(error) = game::create_scene(engine.world_mut(), mesh, material) {
        pill_engine::tracing::error!("scene creation failed: {error}");
        return 1;
    }

    // The overlay draws its egui UI through this context. It is installed once
    // and kept across a project reload, so the reloaded overlay finds the same
    // fonts and widget state. On every generation `install_with_theme`
    // registers both resource types, so a reload releases only what the new
    // generation does not register again, and it inserts a value only when an
    // earlier generation left none.
    //
    // The frame slot is what tells the renderer that this world's UI is the
    // game's. The base theme is `Theme::brutalist`, with the body font the
    // project ships; `ui/accent.theme` changes only the accent and the heading
    // size.
    let mut theme = pill_project_ui::Theme::brutalist();
    theme.fonts.body = Some("fonts/Silkscreen-Regular.ttf".to_string());
    pill_project_ui::install_with_theme(
        engine,
        pill_project_ui::ThemeInstall::new(theme).with_override("ui/accent.theme"),
    );

    0
}

/// Loads the pill mesh, the map the material binds and the material itself.
///
/// The order matters. The material names its map by guid, so the map loads
/// first, and it names a shader that comes from the PBR chain, so the chain
/// installs first too. Every import can run again without harm, so a reload
/// gets the assets that are already loaded.
///
/// # Errors
///
/// Returns an error when the world has no asset manager, when a file or its
/// metadata fails to load, or when the PBR chain does not install.
fn load_assets(
    world: &mut World,
) -> Result<(Handle<Mesh>, Handle<Material>), Box<dyn std::error::Error>> {
    AssetLoader::set_root(res_directory());
    let assets = world
        .get_resource_mut::<AssetManager>()
        .ok_or("the engine AssetManager resource is missing")?;

    let mesh = assets
        .import(AssetImport::<Mesh>::new(
            "models/pill.obj",
            MetadataPolicy::CreateIfMissing,
        ))?
        .handle;
    assets.import(
        AssetImport::<Texture>::new("textures/grid.png", MetadataPolicy::CreateIfMissing)
            .with_initial_settings(TextureImportSettings {
                texture_type: TextureType::Color,
            }),
    )?;
    pbr_pipeline::install(assets)?;
    let material = assets
        .import_standalone::<Material>(Path::new("materials/pill.material"))?
        .handle;
    Ok((mesh, material))
}

/// This project's `res` directory.
///
/// In an ordinary build, `file!()` gives this source's path relative to the
/// project's manifest. The dev host compiles the project through a generated
/// workspace member that has its own manifest elsewhere, and there `file!()`
/// gives an absolute path instead. Joining the path onto `CARGO_MANIFEST_DIR`
/// completes a relative path and leaves an absolute one alone, so both builds
/// find this file.
fn res_directory() -> PathBuf {
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file!());
    source_path
        .parent()
        .and_then(Path::parent)
        .expect("this file lives in the project's `src`")
        .join("res")
}
