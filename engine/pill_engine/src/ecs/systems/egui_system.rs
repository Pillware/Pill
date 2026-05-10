#![cfg(feature = "ui")]

use crate::{ecs::EguiComponent, engine::Engine};
use pill_core::Result;

pub fn egui_system(engine: &mut Engine) -> Result<()> {
    let ui = EguiComponent::get_ui(engine);
    let client = engine
        .get_global_component::<EguiComponent>()?
        .egui_client
        .clone();
    client.set_ui(move |ctx| ui(ctx));
    Ok(())
}
