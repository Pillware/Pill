//! The game UI: an egui context a project draws through, painted by the
//! renderer.
//!
//! # Responsibilities
//!
//! - [`GameUi`]: the egui context, the published pointer and keyboard claims,
//!   and the texture mirror.
//! - [`GameUi::run`]: one pass that takes the engine's input, builds the
//!   frame's draw list and textures, and publishes them into the world's
//!   [`UiFrameSlot`].
//! - [`install`] and [`install_with_theme`]: put the resource into a project's
//!   world with its theme, plus the system that re-applies the theme when the
//!   override file is reimported.
//! - [`mod@convert`], [`input`], [`textures`]: the three pieces it composes.
//!
//! # Design
//!
//! This crate is a shared `dylib`, so the egui context and what it holds -
//! fonts, widget state, textures - outlive a project reload. [`GameUi`] lives
//! here too, so every reload sees the same type. The rule that
//! `pill_editor_core` states applies here as well: nothing a module defines
//! may be stored in the context.
//!
//! This pass is the only producer for the world's [`UiFrameSlot`], and it
//! publishes a frame built from `pill_ui_frame` types. The renderer therefore
//! never sees egui, and no renderer links this crate. A game gates its own
//! input on [`GameUi::wants_pointer`] / [`GameUi::wants_keyboard`].

/// The tessellated output to `pill_ui_frame`.
pub mod convert;
/// The engine's input snapshot and the bridge to egui's input.
pub mod input;
/// The texture mirror a rebuilt painter can upload from.
pub mod textures;

/// The egui this crate pins, re-exported so a project names it through this
/// crate rather than depending on a second copy of its own.
pub use egui;
/// The slot a pass publishes into, re-exported so a project's system names it
/// without depending on the draw-data crate itself.
pub use pill_ui_frame::UiFrameSlot;
/// The UI kit: the theme-to-egui translator, the font installer and the
/// scoped style variants. Re-exported so a project never names the
/// rlib directly.
pub use pill_ui_kit as kit;
/// The theme data a project's UI installs.
pub use pill_ui_theme::{Theme, ThemeOverrides, ThemePathScope};

// External crates
use egui::Context;
use pill_core::math::Vector2f;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_engine_core::{
    AssetManager, Engine, Handle, Res, ResMut, Resource, SystemError, SystemOwner,
};
use pill_ui_frame::{DrawList, UiDraw, UiFrame};

// Current crate
/// The pass's tessellation: the draw conversion, the texture id mapping and
/// the colour conversion.
pub use convert::{convert, external_id, straight_color};
/// The input snapshot and the bridge that turns it into egui's raw input.
pub use input::{raw_input, InputFrame, InputMemory};
use textures::TextureMirror;

// =============================================================================
// Constants
// =============================================================================

/// The surface id of the window a game draws into.
///
/// A pass names the surface whose frame it belongs in. A game passes this
/// constant to [`GameUi::run`], so the UI is drawn with the rest of the
/// primary window's frame.
pub const PRIMARY_SURFACE: u32 = 0;

/// How many families a theme may name, in [`kit::style::ThemeFonts`] order.
const FONT_FAMILIES: usize = 3;

/// The name the theme re-apply system is registered under.
const THEME_SYSTEM_NAME: &str = "ui_theme";

// =============================================================================
// GameUi
// =============================================================================

/// The game's UI: one egui context, the theme it paints with, and the result
/// of the last pass.
pub struct GameUi {
    /// The context a project's UI closures run in.
    context: Context,
    /// What the previous frame told the input bridge.
    input_memory: InputMemory,
    /// Every texture egui handed out, kept whole.
    textures: TextureMirror,
    /// When this UI was created, for egui's animation clock.
    started_at: Instant,
    /// How many draws the last pass produced.
    draw_count: usize,
    /// Whether the last pass wanted the pointer.
    wants_pointer: bool,
    /// Whether the last pass wanted the keyboard.
    wants_keyboard: bool,
    /// The painter's resend request this UI last served.
    ///
    /// A rebuilt renderer starts with an empty texture map and asks for every
    /// image again. The request is a counter, so this UI can tell a new
    /// request from the one it already served.
    served_resend_generation: u64,
    /// The project's code default, before any override.
    base: Theme,
    /// The theme the context currently paints with.
    applied: Theme,
    /// How many themes this UI has applied.
    generation: u64,
    /// The override path the project named, when one loaded.
    override_path: Option<String>,
    /// The loaded override, when one loaded.
    override_handle: Option<Handle<ThemeOverrides>>,
    /// The override's content version this UI last merged from.
    override_version: Option<u64>,
    /// The three font families' handles, in [`kit::style::ThemeFonts`] order
    /// (body, display, monospace).
    font_handles: [Option<Handle<pill_ui_theme::FontAsset>>; 3],
    /// Those handles' content versions when the bytes were last read.
    font_versions: [Option<u64>; 3],
    /// The bytes the kit installs.
    fonts: kit::style::ThemeFonts,
    /// The font set installed in the context.
    font_signature: kit::style::FontSignature,
}

impl Resource for GameUi {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine_core::resource::PlayState {
        pill_engine_core::resource::PlayState::Keep
    }
}

impl Default for GameUi {
    fn default() -> Self {
        GameUi::new()
    }
}

impl GameUi {
    /// A UI with a fresh context, the fallback theme installed and no
    /// textures.
    pub fn new() -> GameUi {
        let theme = Theme::fallback();
        let mut game_ui = GameUi {
            context: Context::default(),
            input_memory: InputMemory::new(),
            textures: TextureMirror::new(),
            started_at: Instant::now(),
            draw_count: 0,
            wants_pointer: false,
            wants_keyboard: false,
            served_resend_generation: 0,
            base: theme.clone(),
            applied: theme.clone(),
            generation: 1,
            override_path: None,
            override_handle: None,
            override_version: None,
            font_handles: [None; 3],
            font_versions: [None; 3],
            fonts: kit::style::ThemeFonts::default(),
            font_signature: kit::style::FontSignature::default(),
        };
        // The fallback install is silent. The theme line worth logging is the
        // project's own, when it is applied.
        kit::style::install(
            &game_ui.context,
            &theme,
            &game_ui.fonts,
            |_| {},
            &mut game_ui.font_signature,
        );
        game_ui
    }

    /// The egui context a project's UI closure runs in.
    ///
    /// Exposed for the callers a facade cannot wrap - a project that installs
    /// its own style, or a test that inspects egui's own state.
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// The theme the context currently paints with.
    pub fn theme(&self) -> &Theme {
        &self.applied
    }

    /// The code default the active theme was merged from.
    pub fn base_theme(&self) -> &Theme {
        &self.base
    }

    /// How many themes this UI has applied.
    pub fn theme_generation(&self) -> u64 {
        self.generation
    }

    /// Install `theme` on this UI's context.
    ///
    /// The theme is plain data, so what reaches the context's style is egui's
    /// own types and owned font bytes. Nothing a reload could invalidate is
    /// stored there.
    pub fn apply_theme(&mut self, theme: &Theme) {
        self.apply_and_record_theme(theme);
    }

    /// Apply `theme`, record it as the active theme and log the application.
    fn apply_and_record_theme(&mut self, theme: &Theme) {
        let fonts_installed = kit::style::install(
            &self.context,
            theme,
            &self.fonts,
            |_| {},
            &mut self.font_signature,
        );
        if fonts_installed {
            pill_core::info!(
                target: telemetry_target::ENGINE,
                signature = %self.font_signature,
                "[ui] fonts installed"
            );
        }
        self.applied = theme.clone();
        self.generation = self.generation.wrapping_add(1);
        let layers = if self.override_handle.is_some() {
            "code,file"
        } else {
            "code"
        };
        let path = self.override_path.as_deref().unwrap_or("none");
        pill_core::info!(
            target: telemetry_target::ENGINE,
            layers = %layers,
            path = %path,
            generation = self.generation,
            accent = %theme.palette.accent.hex(),
            corner = theme.shape.corner_radius,
            "[ui] theme applied"
        );
    }

    /// Re-merge the base with the override when the override's content moved.
    ///
    /// A refused merge warns and keeps the theme that is already applied. An
    /// edited font re-installs the fonts on its own, because the store's
    /// content version of the font moved even when the theme did not.
    fn refresh_from_assets(&mut self, assets: &AssetManager) {
        let fonts_moved = self.refresh_font_bytes(assets);
        let Some(override_handle) = self.override_handle else {
            if fonts_moved {
                let theme = self.applied.clone();
                self.apply_and_record_theme(&theme);
            }
            return;
        };
        let override_version = assets.content_version(override_handle);
        if override_version == self.override_version && !fonts_moved {
            return;
        }
        self.override_version = override_version;
        let Some(overrides) = assets.get(override_handle) else {
            return;
        };
        match self.base.merged_with(overrides) {
            Ok(theme) => {
                if theme != self.applied || fonts_moved {
                    self.apply_and_record_theme(&theme);
                }
            }
            Err(error) => {
                pill_core::warn!(
                    target: telemetry_target::ENGINE,
                    "[ui] theme refused: {error}"
                );
            }
        }
    }

    /// Re-read the loaded fonts whose content moved, and report whether one
    /// did.
    ///
    /// The bytes are copied out of the store again. The handle, the file name
    /// and the guid stay as they were, so the context sees an edited file as a
    /// new font set and nothing else.
    fn refresh_font_bytes(&mut self, assets: &AssetManager) -> bool {
        let mut moved = false;
        for index in 0..FONT_FAMILIES {
            let Some(font_handle) = self.font_handles[index] else {
                continue;
            };
            let content_version = assets.content_version(font_handle);
            if content_version == self.font_versions[index] {
                continue;
            }
            self.font_versions[index] = content_version;
            let bytes = assets.get(font_handle).map(|font| font.bytes.clone());
            set_font_family(&mut self.fonts, index, bytes);
            moved = true;
        }
        moved
    }

    /// Run one pass. `build` draws the UI, and the frame it produces is
    /// published into `slot`, where the renderer takes it.
    ///
    /// `input` is this frame's engine snapshot (see [`InputFrame::capture`]).
    /// `viewport` is the UI's logical size, and `scale_factor` is the monitor
    /// scale. `surface` is the id of the surface the frame is drawn into.
    ///
    /// A frame that a previous pass published and nobody drained is thrown
    /// away, so the renderer always draws the newest one.
    pub fn run(
        &mut self,
        slot: &mut UiFrameSlot,
        input: &InputFrame,
        viewport: Vector2f,
        scale_factor: f32,
        surface: u32,
        build: impl FnMut(&mut egui::Ui),
    ) {
        // Step 1: run the pass on this frame's engine input.
        let elapsed_seconds = self.started_at.elapsed().as_secs_f64();
        let egui_raw_input = raw_input(
            input,
            viewport,
            scale_factor,
            elapsed_seconds,
            &mut self.input_memory,
        );
        let pass_output = self.context.run_ui(egui_raw_input, build);

        // Step 2: copy the textures egui produced into the mirror. epaint
        // panics when a texture delta is dropped while it still holds changes,
        // so the mirror applies all of it and the delta is cleared right
        // after.
        let mut texture_delta = pass_output.textures_delta;
        self.textures.apply(&texture_delta);
        texture_delta.clear();

        // Step 3: turn the tessellated output into the plain frame the
        // renderer drains.
        let primitives = self
            .context
            .tessellate(pass_output.shapes, pass_output.pixels_per_point);
        let mut draw_list = DrawList::new();
        self.draw_count = convert(&primitives, &mut draw_list);
        let mut frame = UiFrame::default();
        // A painter that was rebuilt lost every texture it had and asks for
        // them again. Without this request, nothing would re-send an image
        // that changed only because the painter was rebuilt.
        let resend_generation = slot.texture_resend_generation();
        if resend_generation != self.served_resend_generation {
            self.textures.resend_all();
            self.served_resend_generation = resend_generation;
        }
        self.textures.carry(&mut frame.textures);
        if !draw_list.is_empty() {
            frame.draws.push(UiDraw {
                surface,
                scale_factor,
                list: draw_list,
            });
        }

        // Step 4: record what the UI claimed this pass, so a game can gate its
        // own input on it.
        self.wants_pointer = self.context.egui_wants_pointer_input();
        self.wants_keyboard = self.context.egui_wants_keyboard_input();
        slot.publish(frame);
    }

    /// How many draws the last pass produced.
    pub fn draws(&self) -> usize {
        self.draw_count
    }

    /// Whether the last pass wants the pointer. A game uses this to decide
    /// whether to act on its own pointer input.
    pub fn wants_pointer(&self) -> bool {
        self.wants_pointer
    }

    /// Whether the last pass wants the keyboard. A game uses this to decide
    /// whether to act on its own key input.
    pub fn wants_keyboard(&self) -> bool {
        self.wants_keyboard
    }

    /// How many textures the UI currently mirrors.
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }
}

// =============================================================================
// ThemeInstall
// =============================================================================

/// A project's UI theme: its code default and an optional override file.
///
/// The project builds the base in its own code (a [`Theme`] value, or
/// [`Theme::brutalist`]) and names a `.theme` file below `res`. The file
/// states only the items it changes.
#[derive(Clone, Debug)]
pub struct ThemeInstall {
    /// The project's code default.
    pub base: Theme,
    /// A `.theme` override below `res`, when the project names one.
    pub override_path: Option<String>,
}

impl ThemeInstall {
    /// A theme install with no override file.
    pub fn new(base: Theme) -> Self {
        Self {
            base,
            override_path: None,
        }
    }

    /// Name the `.theme` file merged over the base.
    pub fn with_override(mut self, path: impl Into<String>) -> Self {
        self.override_path = Some(path.into());
        self
    }
}

// =============================================================================
// ResolvedFonts
// =============================================================================

/// The fonts a theme's paths resolved to.
#[derive(Default)]
struct ResolvedFonts {
    /// Each family's handle in the store.
    font_handles: [Option<Handle<pill_ui_theme::FontAsset>>; FONT_FAMILIES],
    /// Those handles' content versions when the bytes were read.
    font_versions: [Option<u64>; FONT_FAMILIES],
    /// The bytes the kit installs.
    fonts: kit::style::ThemeFonts,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Put a [`GameUi`] into `engine`'s world with the fallback theme, the frame
/// slot a renderer drains, and the theme re-apply system.
///
/// A project that wants its own look calls [`install_with_theme`] instead.
/// An existing UI is kept, so a project reload does not reset the context, its
/// fonts, or the state the player typed into it. The slot is what tells a
/// renderer that this world's UI is the game's.
pub fn install(engine: &mut Engine) {
    install_with_theme(engine, ThemeInstall::new(Theme::fallback()));
}

/// Install a project's theme: the code default merged with an optional
/// `.theme` override, plus the system that re-applies it on a reimport.
///
/// A refused override warns, and the code default is used instead. An
/// `editor/` path is refused, because editor content never reaches a game.
///
/// An existing [`GameUi`] keeps its context and its textures. The base theme,
/// the override and the fonts are replaced, and the theme is applied again
/// only when the merge or the fonts differ from what is already applied.
///
/// # Panics
///
/// Panics if an override is loaded and `engine` has no asset manager. Every
/// engine owns one, so this only fires on a broken engine.
pub fn install_with_theme(engine: &mut Engine, install: ThemeInstall) {
    let ThemeInstall {
        base,
        override_path,
    } = install;
    {
        let world = engine.world_mut();
        // Step 1: register the two resources, and keep the UI a previous
        // install created.
        pill_ui_theme::register(world);
        world.register_resource::<GameUi>();
        world.register_resource::<UiFrameSlot>();
        if world.get_resource::<GameUi>().is_none() {
            world.insert_resource(GameUi::new());
        }
        if world.get_resource::<UiFrameSlot>().is_none() {
            world.insert_resource(UiFrameSlot::new());
        }

        // Step 2: load the override and merge it into the base theme. Both are
        // resolved before the UI is borrowed mutably.
        let mut override_path = override_path;
        let mut override_handle = None;
        if let Some(named_path) = override_path.clone() {
            match pill_ui_theme::load_overrides(world, &named_path, ThemePathScope::Game) {
                Ok(loaded) => override_handle = Some(loaded),
                Err(error) => {
                    pill_core::warn!(
                        target: telemetry_target::ENGINE,
                        "[ui] theme refused: {error}"
                    );
                    override_path = None;
                }
            }
        }

        let (merged, override_version) = match override_handle {
            Some(override_handle) => {
                let assets = world
                    .get_resource::<AssetManager>()
                    .expect("the engine owns an asset manager");
                let override_version = assets.content_version(override_handle);
                let merged = match assets.get(override_handle) {
                    Some(overrides) => match base.merged_with(overrides) {
                        Ok(theme) => Some(theme),
                        Err(error) => {
                            pill_core::warn!(
                                target: telemetry_target::ENGINE,
                                "[ui] theme refused: {error}"
                            );
                            None
                        }
                    },
                    None => None,
                };
                (merged, override_version)
            }
            None => (None, None),
        };
        let merged = merged.unwrap_or_else(|| base.clone());
        let resolved = resolve_fonts(world, &merged);

        // Step 3: store the theme on the UI, and apply it when anything
        // changed.
        let game_ui = world.get_resource_mut::<GameUi>().expect("inserted above");
        game_ui.base = base.clone();
        game_ui.override_path = override_path;
        game_ui.override_handle = override_handle;
        game_ui.override_version = override_version;
        let fonts_changed = game_ui.fonts != resolved.fonts;
        game_ui.font_handles = resolved.font_handles;
        game_ui.font_versions = resolved.font_versions;
        game_ui.fonts = resolved.fonts;
        if merged != game_ui.applied || fonts_changed {
            game_ui.apply_and_record_theme(&merged);
        }
    }
    register_reload_system(engine);
}

/// Resolve a theme's font paths through the project's asset store.
///
/// A refused font warns, and its family falls back to the built-in one. The
/// other families and the theme's own values still apply.
fn resolve_fonts(world: &mut pill_engine_core::World, theme: &Theme) -> ResolvedFonts {
    // Step 1: look each family's path up in the store.
    let paths = [
        theme.fonts.body.clone(),
        theme.fonts.display.clone(),
        theme.fonts.monospace.clone(),
    ];
    let mut resolved = ResolvedFonts::default();
    for (index, path) in paths.into_iter().enumerate() {
        let Some(path) = path else {
            continue;
        };
        match pill_ui_theme::load_font(world, &path, ThemePathScope::Game) {
            Ok(loaded) => {
                resolved.font_handles[index] = Some(loaded.handle);
                resolved.font_versions[index] = loaded.version;
            }
            Err(message) => pill_core::warn!(
                target: telemetry_target::ENGINE,
                "[ui] font refused: {message}"
            ),
        }
    }
    // Step 2: copy each loaded font's bytes into the set the kit installs.
    if let Some(assets) = world.get_resource::<AssetManager>() {
        for (index, font_handle) in resolved.font_handles.iter().enumerate() {
            let bytes = font_handle
                .and_then(|font_handle| assets.get(font_handle))
                .map(|font| font.bytes.clone());
            set_font_family(&mut resolved.fonts, index, bytes);
        }
    }
    resolved
}

/// Put one family's bytes where [`kit::style::ThemeFonts`] holds them.
fn set_font_family(fonts: &mut kit::style::ThemeFonts, index: usize, bytes: Option<Vec<u8>>) {
    match index {
        0 => fonts.body = bytes,
        1 => fonts.display = bytes,
        _ => fonts.monospace = bytes,
    }
}

/// Register the re-apply system once per project generation.
///
/// A reload removes the project's systems, so the install that follows a
/// reload registers a fresh one. The check only stops a second
/// `install_with_theme` inside one `init` from adding a duplicate.
fn register_reload_system(engine: &mut Engine) {
    let registered = engine
        .system_snapshots()
        .iter()
        .any(|system| system.name == THEME_SYSTEM_NAME && system.owner == SystemOwner::PROJECT);
    if !registered {
        engine.register_system(THEME_SYSTEM_NAME, theme_reload_system);
    }
}

/// Re-merge and re-apply the theme when the override or a font changed.
fn theme_reload_system(
    mut game_ui: ResMut<GameUi>,
    assets: Res<AssetManager>,
) -> Result<(), SystemError> {
    let (Some(mut game_ui), Some(assets)) = (game_ui.get_mut(), assets.get()) else {
        return Ok(());
    };
    game_ui.refresh_from_assets(assets);
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // Standard library
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};

    // External crates
    use pill_engine_core::asset_store::mount_directory;
    use pill_engine_core::input::{ButtonState, Input, InputEvent, KeyCode};
    use pill_ui_theme::Rgba;

    // Current crate
    use super::*;

    /// Serializes this crate's tests that mount the process-wide asset
    /// directory.
    static MOUNT_LOCK: Mutex<()> = Mutex::new(());

    /// A scratch `res` directory mounted under the mount lock.
    struct ScratchResourceDirectory {
        root_directory: PathBuf,
        _mounted: MutexGuard<'static, ()>,
    }

    impl ScratchResourceDirectory {
        fn new(label: &str) -> Self {
            let mounted_guard = MOUNT_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let root_directory = std::env::temp_dir().join(format!(
                "pill-project-ui-theme-{label}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root_directory);
            std::fs::create_dir_all(root_directory.join("ui")).unwrap();
            mount_directory(&root_directory);
            Self {
                root_directory,
                _mounted: mounted_guard,
            }
        }

        /// Write a theme file at `relative`, inside a standalone document
        /// header, with `body` as the asset.
        fn write_theme(&self, relative: &str, body: &str) {
            let path = self.root_directory.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                path,
                format!(
                    r#"{{"format_version": 1, "asset_type": "pill_ui_theme::ThemeOverrides", "guid": "000000000000000000000000000000a1", "asset": {body}}}"#
                ),
            )
            .unwrap();
        }

        /// Write a font file's bytes at `relative`.
        fn write_font(&self, relative: &str, bytes: &[u8]) {
            let path = self.root_directory.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    }

    impl Drop for ScratchResourceDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root_directory);
        }
    }

    /// Install a brutalist base with `ui/accent.theme` holding `body`.
    fn install_with_override(body: &str) -> (Engine, ScratchResourceDirectory) {
        let scratch_directory = ScratchResourceDirectory::new("install");
        scratch_directory.write_theme("ui/accent.theme", body);
        let mut engine = Engine::new();
        install_with_theme(
            &mut engine,
            ThemeInstall::new(Theme::brutalist()).with_override("ui/accent.theme"),
        );
        (engine, scratch_directory)
    }

    fn current_theme_generation(engine: &Engine) -> u64 {
        engine
            .world()
            .get_resource::<GameUi>()
            .unwrap()
            .theme_generation()
    }

    fn reimport(engine: &mut Engine) {
        let assets = engine
            .world_mut()
            .get_resource_mut::<AssetManager>()
            .unwrap();
        let _ = assets
            .reimport_standalone::<ThemeOverrides>(Path::new("ui/accent.theme"))
            .unwrap();
    }

    /// A theme whose code default names `body` as its body font.
    fn theme_with_body_font(body: &str) -> Theme {
        let mut theme = Theme::brutalist();
        theme.fonts.body = Some(body.to_string());
        theme
    }

    /// A font a theme names is loaded, copied into the UI and recorded, and an
    /// edit to it re-installs the fonts without a theme change.
    #[test]
    fn a_theme_font_loads_and_an_edit_reinstalls_it() {
        let scratch_directory = ScratchResourceDirectory::new("fonts");
        scratch_directory.write_font("fonts/body.ttf", b"\x00\x01\x00\x00first");
        let mut engine = Engine::new();
        install_with_theme(
            &mut engine,
            ThemeInstall::new(theme_with_body_font("fonts/body.ttf")),
        );
        let before = current_theme_generation(&engine);
        {
            let ui = engine.world().get_resource::<GameUi>().unwrap();
            assert_eq!(
                ui.fonts.body.as_deref(),
                Some(&b"\x00\x01\x00\x00first"[..])
            );
            assert!(ui.font_handles[0].is_some(), "the font has a handle");
        }

        scratch_directory.write_font("fonts/body.ttf", b"\x00\x01\x00\x00second");
        {
            let assets = engine
                .world_mut()
                .get_resource_mut::<AssetManager>()
                .unwrap();
            let outcome = assets
                .reimport::<pill_ui_theme::FontAsset>(Path::new("fonts/body.ttf"))
                .expect("the edited font reimports");
            assert!(outcome.replaced, "the store replaced the font");
        }
        // The reload system is what catches the store's new version in a real
        // run; one frame runs it.
        engine.process_frame().expect("the frame runs");

        let ui = engine.world().get_resource::<GameUi>().unwrap();
        assert_eq!(
            ui.fonts.body.as_deref(),
            Some(&b"\x00\x01\x00\x00second"[..]),
            "the edited font's bytes replaced the old ones"
        );
        assert!(
            current_theme_generation(&engine) > before,
            "the fonts re-installed"
        );
    }

    /// A font a theme names but the project does not ship is refused. The
    /// install still happens, the theme's other values apply, and the
    /// built-in families still answer under their names.
    #[test]
    fn a_missing_font_is_refused_and_the_family_falls_back() {
        let _scratch_directory = ScratchResourceDirectory::new("missing-font");
        let mut engine = Engine::new();
        install_with_theme(
            &mut engine,
            ThemeInstall::new(theme_with_body_font("fonts/missing.ttf")),
        );
        let ui = engine.world().get_resource::<GameUi>().unwrap();
        assert_eq!(ui.fonts.body, None, "the refused font installed nothing");
        assert!(ui.font_handles[0].is_none());
        assert_eq!(ui.theme().palette.accent, Theme::brutalist().palette.accent);
        let font_definitions = kit::style::font_definitions(&ui.fonts, |_| {});
        assert!(font_definitions
            .families
            .get(&egui::FontFamily::Proportional)
            .is_some_and(|family| !family.is_empty()));
    }

    /// An engine with a `GameUi` installed that ran one frame with `events`
    /// queued.
    fn engine_with_ui(events: &[InputEvent]) -> Engine {
        let mut engine = Engine::new();
        install(&mut engine);
        for event in events {
            engine.push_input_event(event.clone());
        }
        engine.process_frame().expect("the frame runs");
        engine
    }

    /// The input frame a system would capture this frame.
    fn capture(engine: &Engine) -> InputFrame {
        InputFrame::capture(
            engine
                .world()
                .get_resource::<Input>()
                .expect("the engine installs its input"),
        )
    }

    /// Run one pass into a fresh slot and return what it published.
    fn run_once(engine: &mut Engine) -> UiFrame {
        let input_frame = capture(engine);
        let mut slot = UiFrameSlot::new();
        let world = engine.world_mut();
        let ui = world.get_resource_mut::<GameUi>().expect("installed above");
        ui.run(
            &mut slot,
            &input_frame,
            Vector2f::new(320.0, 200.0),
            1.0,
            PRIMARY_SURFACE,
            |ui| {
                ui.heading("Pill");
                ui.label("game ui");
            },
        );
        slot.take()
    }

    #[test]
    fn a_headless_pass_produces_a_draw_list_and_carries_the_atlas() {
        let mut engine = engine_with_ui(&[]);
        let frame = run_once(&mut engine);
        assert!(!frame.draws.is_empty(), "the pass drew no draw list");
        assert!(
            !frame.draws[0].list.vertices.is_empty(),
            "the draw list carries no geometry"
        );
        assert_eq!(frame.draws[0].surface, 0);
        // egui's first managed texture is its font atlas; it arrives whole.
        assert!(
            frame
                .textures
                .textures
                .iter()
                .any(|texture| texture.id == pill_ui_frame::TextureId(1 << 63)),
            "the font atlas was not carried"
        );
        let ui = engine
            .world()
            .get_resource::<GameUi>()
            .expect("still installed");
        assert!(ui.draws() > 0);
    }

    #[test]
    fn a_second_pass_carries_no_textures_that_did_not_change() {
        let mut engine = engine_with_ui(&[]);
        let first = run_once(&mut engine);
        assert!(!first.textures.textures.is_empty());
        let second = run_once(&mut engine);
        assert!(
            second.textures.textures.is_empty(),
            "unchanged textures were carried again"
        );
        assert!(!second.is_empty(), "the geometry is still drawn");
    }

    #[test]
    fn a_pass_publishes_its_frame_once() {
        let mut engine = engine_with_ui(&[]);
        let input_frame = capture(&engine);
        let mut slot = UiFrameSlot::new();
        assert!(slot.is_empty());
        let world = engine.world_mut();
        let ui = world.get_resource_mut::<GameUi>().expect("installed above");
        ui.run(
            &mut slot,
            &input_frame,
            Vector2f::new(320.0, 200.0),
            1.0,
            PRIMARY_SURFACE,
            |ui| {
                ui.label("published");
            },
        );
        assert!(!slot.is_empty(), "the pass published nothing");
        assert!(!slot.take().is_empty());
        assert!(slot.is_empty(), "the take drained the slot");
    }

    #[test]
    fn installing_again_keeps_the_context_and_its_textures() {
        let mut engine = engine_with_ui(&[]);
        run_once(&mut engine);
        let texture_count = engine
            .world()
            .get_resource::<GameUi>()
            .expect("installed")
            .texture_count();
        assert!(texture_count > 0);
        install(&mut engine);
        let after = engine
            .world()
            .get_resource::<GameUi>()
            .expect("installed")
            .texture_count();
        assert_eq!(after, texture_count, "a reload reset the UI");
    }

    #[test]
    fn a_key_press_reaches_the_ui() {
        let mut engine = engine_with_ui(&[InputEvent::Key {
            key: KeyCode::Tab,
            state: ButtonState::Pressed,
        }]);
        let input_frame = capture(&engine);
        let mut seen_tab = false;
        let mut slot = UiFrameSlot::new();
        let world = engine.world_mut();
        let ui = world.get_resource_mut::<GameUi>().expect("installed above");
        ui.run(
            &mut slot,
            &input_frame,
            Vector2f::new(320.0, 200.0),
            1.0,
            PRIMARY_SURFACE,
            |ui| {
                seen_tab = ui.input(|state| state.key_pressed(egui::Key::Tab));
            },
        );
        assert!(seen_tab, "the bridge did not deliver the key press");
        // A label claims neither the pointer nor the keyboard.
        assert!(!ui.wants_pointer());
        assert!(!ui.wants_keyboard());
    }

    #[test]
    fn wants_pointer_follows_a_window_under_the_cursor() {
        let mut ui = GameUi::new();
        let mut slot = UiFrameSlot::new();
        let viewport = Vector2f::new(320.0, 200.0);
        // The window sits at (10, 10) and fills 120 x 80 points, so (40, 40)
        // is inside it and (300, 190) is not.
        let mut pass = |cursor: Option<Vector2f>| {
            let input_frame = InputFrame {
                cursor,
                ..Default::default()
            };
            ui.run(
                &mut slot,
                &input_frame,
                viewport,
                1.0,
                PRIMARY_SURFACE,
                |ui| {
                    egui::Window::new("tuning")
                        .fixed_pos(egui::pos2(10.0, 10.0))
                        .fixed_size(egui::vec2(120.0, 80.0))
                        .show(ui.ctx(), |ui| {
                            ui.label("volume");
                        });
                },
            );
            ui.wants_pointer()
        };
        // The first pass lays the window out; egui hit-tests from the areas it
        // recorded, so the claim is read on the passes after it.
        pass(None);
        assert!(
            pass(Some(Vector2f::new(40.0, 40.0))),
            "the window is under the cursor"
        );
        assert!(
            !pass(Some(Vector2f::new(300.0, 190.0))),
            "the cursor left the window"
        );
    }

    #[test]
    fn plain_install_applies_the_fallback() {
        let mut engine = Engine::new();
        install(&mut engine);
        let ui = engine.world().get_resource::<GameUi>().unwrap();
        assert_eq!(ui.theme(), &Theme::fallback());
        assert!(ui.override_handle.is_none());
    }

    #[test]
    fn an_override_changes_only_the_items_it_names() {
        let (engine, _scratch_directory) = install_with_override(
            r##"{"palette": {"accent": "#33d17a"}, "text_styles": {"heading": {"size": 24.0}}}"##,
        );
        let ui = engine.world().get_resource::<GameUi>().unwrap();
        assert_eq!(ui.theme().palette.accent, Rgba::rgb(0x33, 0xd1, 0x7a));
        assert_eq!(ui.theme().text_styles["heading"].size, 24.0);
        assert_eq!(ui.theme().palette.panel, Theme::brutalist().palette.panel);
        assert!(ui.override_handle.is_some());
    }

    #[test]
    fn a_reimport_raises_the_generation() {
        let (mut engine, scratch_directory) =
            install_with_override(r##"{"palette": {"accent": "#111111"}}"##);
        let before = current_theme_generation(&engine);
        scratch_directory.write_theme("ui/accent.theme", r##"{"palette": {"accent": "#222222"}}"##);
        reimport(&mut engine);
        engine.process_frame().expect("the frame runs");
        let ui = engine.world().get_resource::<GameUi>().unwrap();
        assert!(ui.theme_generation() > before);
        assert_eq!(ui.theme().palette.accent, Rgba::rgb(0x22, 0x22, 0x22));
    }

    #[test]
    fn an_unchanged_version_does_not_re_apply() {
        let (mut engine, _scratch_directory) =
            install_with_override(r##"{"palette": {"accent": "#111111"}}"##);
        let before = current_theme_generation(&engine);
        engine.process_frame().expect("the frame runs");
        assert_eq!(current_theme_generation(&engine), before);
    }

    #[test]
    fn a_refused_override_keeps_the_previous_theme() {
        let (mut engine, scratch_directory) =
            install_with_override(r##"{"palette": {"accent": "#111111"}}"##);
        let before = current_theme_generation(&engine);
        scratch_directory.write_theme("ui/accent.theme", r##"{"palette": {"accent": "nope"}}"##);
        reimport(&mut engine);
        engine.process_frame().expect("the frame runs");
        let ui = engine.world().get_resource::<GameUi>().unwrap();
        assert_eq!(ui.theme_generation(), before);
        assert_eq!(ui.theme().palette.accent, Rgba::rgb(0x11, 0x11, 0x11));
    }

    #[test]
    fn an_editor_path_is_refused() {
        let mut engine = Engine::new();
        install_with_theme(
            &mut engine,
            ThemeInstall::new(Theme::brutalist()).with_override("editor/a.theme"),
        );
        let ui = engine.world().get_resource::<GameUi>().unwrap();
        assert_eq!(ui.theme(), &Theme::brutalist());
        assert!(ui.override_handle.is_none());
        assert!(ui.override_path.is_none());
    }

    #[test]
    fn installing_twice_with_the_same_input_does_not_re_apply() {
        let (mut engine, _scratch_directory) =
            install_with_override(r##"{"palette": {"accent": "#111111"}}"##);
        let before = current_theme_generation(&engine);
        install_with_theme(
            &mut engine,
            ThemeInstall::new(Theme::brutalist()).with_override("ui/accent.theme"),
        );
        assert_eq!(current_theme_generation(&engine), before);
        let system_count = engine
            .system_snapshots()
            .into_iter()
            .filter(|system| system.name == "ui_theme")
            .count();
        assert_eq!(system_count, 1);
    }
}
