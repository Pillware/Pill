use crate::ecs::components::{GlobalComponent, GlobalComponentStorage};

use pill_core::PillTypeMapKey;

#[derive(Copy, Clone)]
pub enum BuildStatus {
    Pass,
    Fail,
    Warning,
}

// display the build type + target
// also display the last hot-reload status (compiled the code and reloaded or failed) with red/green
// light indicator
// updated every time we try hot-reloading and build_project
pub struct BuildStatusIndicatorComponent {
    pub(crate) last_build_status: BuildStatus,
}

impl Default for BuildStatusIndicatorComponent {
    fn default() -> Self {
        Self {
            last_build_status: BuildStatus::Pass,
        }
    }
}

impl PillTypeMapKey for BuildStatusIndicatorComponent {
    type Storage = GlobalComponentStorage<BuildStatusIndicatorComponent>;
}

impl GlobalComponent for BuildStatusIndicatorComponent {}
