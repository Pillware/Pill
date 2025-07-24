#![cfg(feature = "rendering")]

use crate::{
    ecs::components::{ Component, GlobalComponentStorage, GlobalComponent }, engine::Engine
};

use pill_core::PillTypeMapKey;

use anyhow::{Result, Error, Context};

pub struct EguiManagerComponent {
}

impl EguiManagerComponent {
    pub fn new() -> Self {
        Self {

        }
    }

    pub fn get_ui(engine: &mut Engine) -> Box<dyn Fn(&egui::Context)> {

        let ff = engine.frame_delta_time;
        let x = Box::new(move |ui: &egui::Context| {
            egui::Window::new("PillEngine")
                .default_open(true)
                .resizable(true)
                .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
                .show(ui, |ui| {
                    if ui.add(egui::Button::new("Click me")).clicked() {
                        println!("PRESSED");
                    }
                    ui.add(egui::Label::new(format!("FPS {}", 1000.0 / ff) ));
                    //ui.add(egui::Label::new(format!("Entities {}", entity_count)));
                    //ui.add(egui::Label::new(format!("Entities {}", engine.scene_manager.get_active_scene().unwrap().entities.len()) ));
                });
        });

        x
    }

    pub(crate) fn update(&mut self, delta_time: f32) -> Result<()> {


        Ok(())
    }
}

impl PillTypeMapKey for EguiManagerComponent {
    type Storage = GlobalComponentStorage<EguiManagerComponent>;
}

impl GlobalComponent for EguiManagerComponent {

}
