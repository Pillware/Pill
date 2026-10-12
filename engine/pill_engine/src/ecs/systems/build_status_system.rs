#![cfg(feature = "ui")]
use crate::{
    ecs::{BuildStatus, BuildStatusIndicatorComponent},
    engine::Engine,
};
use pill_core::Result;

pub fn build_status_system(engine: &mut Engine) -> Result<()> {
    // Set by pill_native only in hot-reload mode; without it there is no build status to show
    let Ok(status_env) = std::env::var("PILL_HOT_RELOAD_STATUS") else {
        return Ok(());
    };
    let build_status_component =
        engine.get_global_component_mut::<BuildStatusIndicatorComponent>()?;

    let status = match status_env.as_str() {
        "fail" => BuildStatus::Fail,
        "warn" => BuildStatus::Warning,
        "pass" => BuildStatus::Pass,
        _ => BuildStatus::Fail,
    };
    build_status_component.last_build_status = status;
    Ok(())
}
