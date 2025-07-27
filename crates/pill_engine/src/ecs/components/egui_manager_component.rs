use crate::{
    ecs::{components::{ Component, GlobalComponent, GlobalComponentStorage }, systems, UpdatePhase}, engine::Engine
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

        let entity_count =  engine.scene_manager.get_active_scene().unwrap().entities.len();
        let system_count = engine.system_manager.update_phases.iter().map(|(_, systems)| systems.len()).sum::<usize>();
        let systems_delta_times: Vec<(UpdatePhase, Vec<(String, f32)>)> = engine.system_manager.update_phases
            .iter()
            .map(|(update_phase, systems)| {
                let system_time = systems.iter().map(|(_name, system)| (system.name.clone(), system.delta_time)).collect();
                (update_phase.clone(), system_time)
            })
            .collect();
        let total_systems_delta_time = systems_delta_times.iter().map(|(_, times)| times.iter().map(|(_, dt)| *dt).sum::<f32>()).sum::<f32>();
        let frame_delta_time = engine.frame_delta_time;

        let ui = Box::new(move |ui: &egui::Context| {
            egui::Window::new("PillEngine")
                .default_open(true)
                .resizable(true)
                .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
                .show(ui, |ui| {
                    if ui.add(egui::Button::new("Click me")).clicked() {
                        println!("PRESSED");
                    }
                    ui.add(egui::Label::new(format!("FPS {}", 1000.0 / frame_delta_time) ));
                    ui.add(egui::Label::new(format!("Frame Delta Time: {:.5} ms", frame_delta_time)));
                    ui.add(egui::Label::new(format!("Entities: {}", entity_count)));
                    ui.separator();
                    ui.add(egui::Label::new(format!("Systems: {}, Total delta time: {:.5} ms", system_count, total_systems_delta_time)));
                    for (update_phase, systems_delta_times) in systems_delta_times.iter() {
                        ui.add(egui::Label::new(format!("  Update Phase: {}", update_phase)));
                        for (i, (system_name, delta_time)) in systems_delta_times.iter().enumerate() {
                            ui.add(egui::Label::new(format!("    {}. System {}: {:.5} ms", i, system_name, delta_time)));
                        }
                    }
                });
        });

        ui
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