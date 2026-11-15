//! Statically linked project and module registration, for shipping builds.
//!
//! # Responsibilities
//!
//! - Describes a project and its extensions as ordinary Rust functions that
//!   are compiled into the binary, rather than as DLLs found at run time.
//! - Initializes them in the same order, and under the same owners, that the
//!   hot-reloading host uses.
//! - Defines [`ProjectBackend`], the extension point through which a project
//!   that is not Rust (a C# assembly, through `pill_csharp_bridge`) starts.
//!
//! # Design
//!
//! The dependency direction forces this shape. This crate cannot name the
//! project crate, or every game would need its own player, so the crate that
//! links both - the generated shipping bundle - passes the functions in. That
//! also makes the set of linked modules visible at the call site. A shipping
//! build wants exactly that: the bundle generator reads
//! `project_settings.yaml` while the binary is built, and the released binary
//! holds what the generator baked in instead of reading a settings file.
//!
//! For the same reason this crate names no C# type: a managed project arrives
//! as a [`StaticProjectBackend::External`] backend the bridge implements, so a
//! build that runs no C# links none of it.
//!
//! A linked entry point runs the same transaction as the macro-generated
//! `pill_module_init` wrapper. `register_all_components` drains this artifact's
//! `inventory` registry into the world first; skipping it fails as "my
//! components do not exist" rather than as a link error. The wrapper also
//! catches panics. Here there is no C ABI to unwind across, so a panic stays an
//! ordinary Rust panic instead of becoming a status that hides the backtrace.

// Standard library
use std::any::Any;
use std::sync::Arc;

// External crates
use pill_core::error::{HostError, LibraryError, ModuleError};
use pill_core::telemetry::telemetry_target;
use pill_core::{info, warn};
use pill_engine::{Engine, SystemOwner};

// Current crate
use crate::registration::{extension_owner, register_module, PROJECT_SCOPE};
use crate::LoggingSettings;

// =============================================================================
// Types
// =============================================================================

/// One extension compiled into the binary.
///
/// It is the counterpart of an extension the development host loads: the same
/// module, named the same way, but reached by a direct call instead of the
/// `pill_module_init` export of a loaded DLL.
#[derive(Clone, Copy)]
pub struct StaticModule {
    /// Crate name, used for logging exactly as the reloading host uses the
    /// extension's name.
    pub name: &'static str,
    /// The function `#[pill_module]` was written on.
    ///
    /// A loaded module reaches the same function through its `pill_module_init`
    /// export. Here this crate calls it directly.
    pub init: fn(&mut Engine) -> u32,
}

/// A project that registers itself by other means than a linked Rust
/// function: a C# assembly, through `pill_csharp_bridge`.
///
/// This crate calls it once, after every extension has registered, and keeps
/// what it returns until the engine is gone.
pub trait ProjectBackend {
    /// Register the project's systems and data with `engine`.
    ///
    /// `exposed_component_names` are the components the extensions registered,
    /// in registration order, for a backend that binds them (the C# bridge
    /// gives managed code byte-level access to them). The returned guard holds
    /// whatever must stay alive as long as the systems it registered - for C#,
    /// the .NET runtime itself. Dropping it earlier would unload their code.
    ///
    /// # Errors
    ///
    /// Returns a [`HostError`] when the project cannot start. There is no
    /// previous generation to fall back to, so the player stops there.
    fn start(
        &self,
        engine: &mut Engine,
        exposed_component_names: &[String],
    ) -> Result<Box<dyn Any>, HostError>;
}

/// How a shipping build reaches its project.
#[derive(Clone)]
pub enum StaticProjectBackend {
    /// A Rust project linked into this binary.
    Native {
        /// The `register` half of `#[pill_project(register = ...)]`.
        ///
        /// A linked project runs it once, before [`Self::Native::init`] and
        /// after its declarations, which is the same order a reloading host
        /// uses on its first load. A project that keeps state only a
        /// registration half can refresh therefore behaves in a shipped build
        /// as it does in the development host.
        register: fn(&mut Engine) -> u32,
        /// The function `#[pill_project]` was written on.
        init: fn(&mut Engine) -> u32,
    },
    /// A project started by a backend outside this crate: the C# bridge's
    /// CoreCLR or NativeAOT backend.
    ///
    /// Shared rather than boxed, so a [`StaticProject`] stays cloneable. The
    /// windowed frontend needs that: it clones the project to run the setup and
    /// keeps the copy it reads its name from.
    External(Arc<dyn ProjectBackend>),
}

/// The renderer a windowed shipping build links, as the two functions this
/// crate needs from it.
///
/// Function pointers rather than a dependency, because this crate names no
/// renderer type. The shipping bundle, the one crate that depends on the
/// renderer, fills this in.
#[derive(Clone, Copy)]
pub struct StaticRenderer {
    /// The renderer's registration (`pill_master_renderer::register`), which
    /// adds the `rendering` system.
    pub init: fn(&mut Engine) -> u32,
    /// Builds a backend on a window (`pill_master_renderer::attach`), as a
    /// future: a native frontend blocks on it once, a web frontend awaits it.
    ///
    /// # Safety
    ///
    /// The window must outlive the backend built on it. That is the renderer
    /// module's own contract. This crate meets it by keeping the backend in a
    /// field declared before the window it was built on.
    pub attach: StaticRendererAttachFn,
}

/// `attach(window, width, height)`: a renderer backend being built on `window`.
pub type StaticRendererAttachFn =
    unsafe fn(pill_renderer_api::RawWindowData, u32, u32) -> pill_renderer_api::AttachFuture;

/// A project and its extensions, compiled into the binary.
///
/// It takes the place of the development host's project settings. There is no
/// project path, no build command and no watch directory, because nothing is
/// built or watched.
#[derive(Clone)]
pub struct StaticProject {
    /// Project name, used in logs and as the window title.
    pub name: &'static str,
    /// How to reach the project itself.
    pub backend: StaticProjectBackend,
    /// Extensions, initialized in order **before** the project.
    ///
    /// The order matters here for the same reason it does with reloading: the
    /// project may name types a module defines, so the module has to register
    /// them first. A managed project also needs every module's components
    /// registered before its assembly loads, so the bindings it is handed can
    /// resolve them.
    pub modules: &'static [StaticModule],
    /// The renderer this binary links, when it was built to draw.
    ///
    /// `None` for a headless shipping build, which links no renderer at all.
    pub renderer: Option<StaticRenderer>,
    /// The project's `res` directory as an asset pack, embedded in the binary
    /// by the shipping bundle and mounted before anything initializes.
    ///
    /// `None` for a project without a `res` directory, and for the browser
    /// posture, whose pack is fetched instead (see [`Self::asset_pack_url`]).
    pub asset_pack: Option<&'static [u8]>,
    /// Where the browser posture fetches its asset pack from, relative to the
    /// page. The shipping bundle names the file that `build_web.py` puts beside
    /// it.
    ///
    /// `pill_web` is the only code that reads it. A native build embeds its
    /// pack instead and leaves this `None`, and this crate ignores it: fetching
    /// is a browser's job.
    pub asset_pack_url: Option<&'static str>,
    /// The `logging:` section of the project's settings, as the bundle
    /// generator validated it; [`StaticLogging::NONE`] for none.
    pub logging: StaticLogging,
    /// Whether debug drawing starts on: the settings' `debug_draw:` key,
    /// `false` when absent. A system can still switch it at runtime.
    pub debug_draw: bool,
    /// The scene the project's settings named as its startup scene, loaded once
    /// every component is registered (`startup_scene:` in
    /// `project_settings.yaml`).
    ///
    /// A natively linked project gets it before its `init` seeds anything; an
    /// external backend gets it once its `start` has returned.
    pub startup_scene: Option<&'static str>,
}

impl StaticProject {
    /// Initialize every extension, then the project.
    ///
    /// Owners come from [`crate::registration`], as they do in the reloading
    /// host, so a statically linked module's systems are attributed to the same
    /// owner they would have had there. Nothing here can be cleared or
    /// re-registered, but the attribution still decides which systems a pause
    /// suspends and how a system list labels them.
    ///
    /// On success it returns an external backend's guard, which the caller must
    /// keep alive for as long as the engine lives (see [`ProjectBackend::start`]).
    /// A natively linked project returns `None`.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleError::InitializationFailed`] naming the first module
    /// whose registration or init reported a non-zero status,
    /// [`LibraryError::InitializationFailed`] when a native project's entry
    /// point does, or what an external backend returns.
    ///
    /// Initialization stops at the first failure: there is no previous
    /// generation to roll back to on this path, so continuing would run a
    /// partially registered world.
    pub(crate) fn initialize(
        &self,
        engine: &mut Engine,
    ) -> Result<Option<Box<dyn Any>>, HostError> {
        // Step 1: Register every extension under the owner its load index
        // gives it, and collect the components it adds as we go, so an
        // external backend is handed those names instead of rebuilding them
        // afterwards. The renderer's data crate is the first module, as in the
        // reloading host (the bundle generator puts it there), so modules and
        // the project find its components registered.
        let mut exposed_component_names: Vec<String> = Vec::new();

        for (index, module) in self.modules.iter().enumerate() {
            let system_owner = extension_owner(index);
            let registration_sequence = engine.world().component_registration_sequence();
            run_linked_entry_point(
                engine,
                pill_engine::component_registry::no_registration,
                module.init,
                Some(system_owner),
                None,
            )
            .map_err(|status| ModuleError::InitializationFailed {
                module: module.name.to_string(),
                status,
            })?;
            exposed_component_names.extend(
                engine
                    .world()
                    .registered_component_names_since(registration_sequence),
            );
            info!(
                target: telemetry_target::LINK,
                module = module.name,
                owner = system_owner.0,
                "extension linked"
            );
        }

        // Step 2: The project itself, through its linked functions or through
        // an external backend.
        let project_guard = match &self.backend {
            StaticProjectBackend::Native { register, init } => {
                run_linked_entry_point(engine, *register, *init, PROJECT_SCOPE, self.startup_scene)
                    .map_err(|status| LibraryError::InitializationFailed { status })?;
                None
            }
            // No build, no codegen, no watcher: whatever the backend loads was
            // produced when this binary was built.
            //
            // A managed project registers its components inside `start`, so its
            // startup scene goes in once that returns - the earliest point at
            // which its types exist. A native project's scene goes in between
            // its registration and its `init`; see `run_linked_entry_point`.
            StaticProjectBackend::External(project_backend) => {
                let project_guard = project_backend.start(engine, &exposed_component_names)?;
                if let Some(path) = self.startup_scene {
                    apply_project_startup_scene(engine, path);
                }
                Some(project_guard)
            }
        };

        info!(
            target: telemetry_target::LINK,
            project = self.name,
            modules = self.modules.len(),
            external = project_guard.is_some(),
            "project linked"
        );
        // Step 3: Every manager's settings asset, out of the pack the frontend
        // mounted. After the project, because a manager may be one the project
        // registers itself. A bundle has no writable project directory, so a
        // manager without a file keeps its document defaults.
        apply_manager_settings(engine);
        Ok(project_guard)
    }
}

/// A project's logging settings, compiled into the binary as text.
///
/// Text rather than levels, so the generated bundle names no engine type.
/// [`Self::settings`] reads it into [`LoggingSettings`].
#[derive(Clone, Copy, Debug, Default)]
pub struct StaticLogging {
    /// The `level:` key, when the settings set one.
    pub level: Option<&'static str>,
    /// The `timestamp:` key (`date_time` or `time`), when the settings set one.
    pub timestamp: Option<&'static str>,
    /// The `source_location:` key, when the settings set one.
    pub source_location: Option<bool>,
    /// The `targets:` map, as `(target, level)` pairs sorted by target. The
    /// bundle generator writes them in that order.
    pub targets: &'static [(&'static str, &'static str)],
}

impl StaticLogging {
    /// No logging settings: the engine's defaults apply.
    pub const NONE: Self = Self {
        level: None,
        timestamp: None,
        source_location: None,
        targets: &[],
    };

    /// The settings this text describes.
    ///
    /// # Errors
    ///
    /// Returns the message of the first level, timestamp format or target that
    /// does not read. Only a hand-edited bundle can hold one, because the
    /// generator refuses all three.
    pub fn settings(&self) -> Result<LoggingSettings, String> {
        LoggingSettings::from_text(
            self.level,
            self.timestamp,
            self.source_location,
            self.targets,
        )
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Applies every manager's settings asset, reading each one from the mounted
/// asset roots - in a bundle, the pack.
///
/// A settings asset is an ordinary file under `res/settings`, so a shipped
/// build needs nothing baked: the pack answers the conventional path the same
/// way a project directory does. A manager with no file keeps its document
/// defaults, which is what a bundle that ships no settings for it means.
///
/// A refusal is the manager's own call, and a file that does not fit is the
/// project's - both are logged and neither stops the build.
fn apply_manager_settings(engine: &mut Engine) {
    let world = engine.world_mut();
    for manager in world.settings_asset_manager_ids() {
        let Some(surface) = world.manager_settings_surface(manager) else {
            continue;
        };
        let Some(path) = world.manager_settings_path(manager) else {
            continue;
        };
        match world.apply_manager_settings_asset(manager) {
            Some(Ok(pill_engine::SettingsOutcome::Applied)) => info!(
                target: telemetry_target::LINK,
                manager = surface.settings_name.as_str(),
                path = path.as_str(),
                "[settings] applied manager settings"
            ),
            Some(Ok(pill_engine::SettingsOutcome::Refused)) => warn!(
                target: telemetry_target::LINK,
                manager = surface.settings_name.as_str(),
                path = path.as_str(),
                "[settings] refused manager settings; the manager keeps its values"
            ),
            Some(Err(reason)) => warn!(
                target: telemetry_target::LINK,
                manager = surface.settings_name.as_str(),
                path = path.as_str(),
                reason = reason.as_str(),
                "[settings] no settings asset was applied; the manager keeps its document defaults"
            ),
            None => {}
        }
    }
}

/// Queue and load a project's startup scene, warning when it cannot be read.
///
/// A failure is a warning rather than an error, because a missing or unreadable
/// scene must not stop a project from starting.
fn apply_project_startup_scene(engine: &mut Engine, scene_path: &str) {
    pill_engine::queue_startup_scene(engine.world_mut(), scene_path);
    if let Some(Err(reason)) = pill_engine::apply_startup_scene(engine) {
        pill_core::warn!(
            target: telemetry_target::ENGINE,
            path = scene_path,
            reason,
            "startup scene not loaded; the project starts without it"
        );
    }
}

/// A project registration half that does nothing.
///
/// A generated shipping bundle points a project without a `register` half at
/// this, so the static backend's shape stays the same for every project. A
/// project that seeds everything from its `init` needs nothing else.
pub fn no_project_registration(_engine: &mut Engine) -> u32 {
    0
}

/// Run one linked entry point the way its generated ABI wrapper would.
///
/// `scope` sets the owner the registrations are recorded under when present.
/// An extension needs that; the project must not have it. `startup_scene` is
/// the project's own startup scene, applied after the component types exist and
/// before the project seeds its entities over them. An extension passes `None`,
/// because a module has no scene of its own.
///
/// # Errors
///
/// Returns the non-zero status the entry point reported, from the artifact's
/// registration or from `init`. The caller names the subject, because only it
/// knows whether the failure is a module's or the project's.
pub(crate) fn run_linked_entry_point(
    engine: &mut Engine,
    register: fn(&mut Engine) -> u32,
    init: fn(&mut Engine) -> u32,
    scope: Option<SystemOwner>,
    startup_scene: Option<&'static str>,
) -> Result<(), u32> {
    // The same transaction the generated wrappers run, in the same order: both
    // paths end in `run_artifact_entry`, so the order lives in one place rather
    // than in two copies that have to stay identical.
    let status = pill_engine::component_registry::run_artifact_entry(
        engine,
        |engine| {
            // A project's startup scene goes in here: the component types
            // exist, and the project has not yet seeded its own entities over
            // them.
            if let Some(path) = startup_scene {
                apply_project_startup_scene(engine, path);
            }
        },
        register,
        |engine| register_module(engine, scope, init),
    );

    if status == 0 {
        Ok(())
    } else {
        Err(status)
    }
}

// =============================================================================
// Test Projects
// =============================================================================

/// A project with nothing set but `backend`: what a test starts from.
///
/// Every other field holds one plain value, so a test names only the field it
/// is about: `StaticProject { modules: MODULES, ..test_project(backend) }`.
#[cfg(test)]
pub(crate) fn test_project(backend: StaticProjectBackend) -> StaticProject {
    StaticProject {
        name: "project",
        backend,
        modules: &[],
        renderer: None,
        asset_pack: None,
        asset_pack_url: None,
        logging: StaticLogging::NONE,
        debug_draw: false,
        startup_scene: None,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::{render_standalone, save_scene, AssetGuid, EntityId};

    /// A module that registers nothing, for shape assertions.
    fn no_op(_engine: &mut Engine) -> u32 {
        0
    }

    /// A module that reports failure, to exercise the error path.
    fn failing(_engine: &mut Engine) -> u32 {
        7
    }

    /// A backend that records the names it was handed and returns a guard.
    struct RecordingBackend {
        /// The component names `StaticProject::initialize` handed to `start`.
        recorded_component_names: std::sync::Mutex<Vec<String>>,
    }

    impl ProjectBackend for RecordingBackend {
        fn start(
            &self,
            _engine: &mut Engine,
            exposed_component_names: &[String],
        ) -> Result<Box<dyn Any>, HostError> {
            *self.recorded_component_names.lock().unwrap() = exposed_component_names.to_vec();
            Ok(Box::new(42u32))
        }
    }

    /// Modules initialize before the project, because the project may name
    /// types a module defines.
    #[test]
    fn modules_initialize_before_the_project() {
        static ORDER: std::sync::Mutex<Vec<&str>> = std::sync::Mutex::new(Vec::new());

        fn module_init(_engine: &mut Engine) -> u32 {
            ORDER.lock().unwrap().push("module");
            0
        }
        fn project_module_init(_engine: &mut Engine) -> u32 {
            ORDER.lock().unwrap().push("project");
            0
        }
        static MODULES: &[StaticModule] = &[StaticModule {
            name: "first",
            init: module_init,
        }];

        let mut engine = Engine::new();
        let project = StaticProject {
            modules: MODULES,
            ..test_project(StaticProjectBackend::Native {
                register: no_op,
                init: project_module_init,
            })
        };
        project.initialize(&mut engine).expect("both succeed");

        assert_eq!(*ORDER.lock().unwrap(), vec!["module", "project"]);
    }

    /// A linked project runs its own registration half first, then its init,
    /// so a project that keeps state in `register` behaves in a shipped build
    /// as it does in the development host.
    #[test]
    fn a_linked_project_registers_before_it_initializes() {
        static ORDER: std::sync::Mutex<Vec<&str>> = std::sync::Mutex::new(Vec::new());

        fn project_register(_engine: &mut Engine) -> u32 {
            ORDER.lock().unwrap().push("register");
            0
        }
        fn project_init(_engine: &mut Engine) -> u32 {
            ORDER.lock().unwrap().push("init");
            0
        }

        let mut engine = Engine::new();
        let project = test_project(StaticProjectBackend::Native {
            register: project_register,
            init: project_init,
        });
        project.initialize(&mut engine).expect("both succeed");

        assert_eq!(*ORDER.lock().unwrap(), vec!["register", "init"]);
    }

    /// A failing module stops initialization and names itself, rather than
    /// leaving a partially registered world running.
    #[test]
    fn a_failing_module_is_named_and_stops_initialization() {
        static MODULES: &[StaticModule] = &[
            StaticModule {
                name: "healthy",
                init: no_op,
            },
            StaticModule {
                name: "broken",
                init: failing,
            },
        ];

        let mut engine = Engine::new();
        let project = StaticProject {
            modules: MODULES,
            ..test_project(StaticProjectBackend::Native {
                register: no_op,
                init: no_op,
            })
        };
        let Err(error) = project.initialize(&mut engine) else {
            panic!("a non-zero module status must be reported");
        };
        let message = error.to_string();
        assert!(
            message.contains("broken"),
            "the failure should name the module that reported it, got {message:?}"
        );
    }

    /// A native project returns no guard, so nothing is kept alive that does
    /// not need to be.
    #[test]
    fn a_native_project_returns_no_guard() {
        let mut engine = Engine::new();
        let project = test_project(StaticProjectBackend::Native {
            register: no_op,
            init: no_op,
        });
        let guard = project.initialize(&mut engine).expect("it succeeds");
        assert!(guard.is_none());
    }

    /// An external backend starts after the modules, is handed the components
    /// they registered, and its guard is returned for the caller to keep.
    #[test]
    fn an_external_backend_gets_the_module_components_and_returns_its_guard() {
        fn registers_common_components(engine: &mut Engine) -> u32 {
            pill_engine::register_transform_component(engine.world_mut());
            0
        }
        static MODULES: &[StaticModule] = &[StaticModule {
            name: "common",
            init: registers_common_components,
        }];

        let backend = Arc::new(RecordingBackend {
            recorded_component_names: std::sync::Mutex::new(Vec::new()),
        });
        let mut engine = Engine::new();
        let project = StaticProject {
            modules: MODULES,
            ..test_project(StaticProjectBackend::External(backend.clone()))
        };
        let guard = project.initialize(&mut engine).expect("it succeeds");

        let guard = guard.expect("an external backend's guard is returned");
        assert_eq!(guard.downcast_ref::<u32>(), Some(&42));
        assert!(
            !backend.recorded_component_names.lock().unwrap().is_empty(),
            "the backend must see what the module registered"
        );
    }

    /// Write a one-entity scene below a fresh temporary asset directory and
    /// mount it, returning that directory and the id the entity carries.
    fn one_entity_scene() -> (std::path::PathBuf, EntityId) {
        // Step 1: A fresh directory below the system temp path, cleared first
        // so a repeated run in this process never reads a previous scene.
        let root =
            std::env::temp_dir().join(format!("pill-player-startup-scene-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scenes")).unwrap();

        // Step 2: A world holding one entity, saved as a scene document.
        let id = EntityId::from_name("seeded");
        let mut writer = Engine::new();
        writer
            .world_mut()
            .register_standalone_asset::<pill_engine::Scene>();
        writer
            .world_mut()
            .create_entity()
            .with_id(id)
            .build()
            .unwrap();
        let registry = writer
            .world()
            .get_resource::<pill_engine::ImportRegistry>()
            .cloned()
            .expect("the engine registers its import registry");
        let (document, report) = {
            let world = writer.world();
            let assets = world
                .get_resource::<pill_engine::asset::AssetManager>()
                .expect("the engine owns an asset manager");
            save_scene(world, assets, &registry, None)
        };
        assert!(
            report.skipped_fields.is_empty(),
            "{:?}",
            report.skipped_fields
        );
        // Step 3: Render the document as a standalone scene asset and write it
        // into that directory, then mount the directory. The project's startup
        // scene then resolves through the store like any other asset.
        let bytes = render_standalone::<pill_engine::Scene>(
            AssetGuid::random().expect("a fresh guid"),
            &document,
        )
        .expect("the scene renders");
        std::fs::write(root.join("scenes/one.scene"), bytes).unwrap();
        pill_engine::asset_store::mount_directory(root.clone());
        (root, id)
    }

    /// A project's startup scene is loaded for a managed project too, not only
    /// for a natively linked one.
    #[test]
    fn an_external_backend_starts_with_the_projects_startup_scene() {
        let (_root, seeded) = one_entity_scene();

        // A module registers the scene import, as a renderer's data crate does
        // in a real project.
        fn registers_the_scene_import(engine: &mut Engine) -> u32 {
            engine
                .world_mut()
                .register_standalone_asset::<pill_engine::Scene>();
            0
        }
        static MODULES: &[StaticModule] = &[StaticModule {
            name: "scene",
            init: registers_the_scene_import,
        }];

        let backend = Arc::new(RecordingBackend {
            recorded_component_names: std::sync::Mutex::new(Vec::new()),
        });
        let mut engine = Engine::new();
        let project = StaticProject {
            modules: MODULES,
            startup_scene: Some("scenes/one.scene"),
            ..test_project(StaticProjectBackend::External(backend))
        };
        project.initialize(&mut engine).expect("it succeeds");

        assert!(
            engine.world().entity_by_id(seeded).is_some(),
            "a managed project must start with the scene its settings name"
        );
    }
}
