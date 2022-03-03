use crate::{
    engine::Engine,
};

use anyhow::{ Result };

pub fn egui_system(engine: &mut Engine) -> Result<()> {
   
    // Update egui state
    engine.egui_state.update();

    Ok(())
}
