use std::collections::HashMap;

use crate::{
    ecs::{components::{ Component, GlobalComponent, GlobalComponentStorage }, systems, UpdatePhase}, engine::Engine
};

use egui::Ui;
use indexmap::IndexMap;
use pill_core::{PillTypeMapKey, Timer, TimerRecord};

use anyhow::{Result, Error, Context};

pub struct EguiManagerComponent {
    collapsing_state: HashMap<String, bool>,
}

impl EguiManagerComponent {
    pub fn new() -> Self {
        Self { 
          collapsing_state: HashMap::new(),
        }
    }

    pub fn get_ui(engine: &mut Engine) -> Box<dyn Fn(&egui::Context)> {

        let entity_count =  engine.scene_manager.get_active_scene().unwrap().entities.len();
        let system_count = engine.system_manager.update_phases.iter().map(|(_, systems)| systems.len()).sum::<usize>();
        
        let system_timers: Vec<(UpdatePhase, Vec<(String, Timer)>)> = engine.system_manager.update_phases
            .iter()
            .map(|(update_phase, systems)| {
                let system_timers = systems.iter().map(|(_, system)| (system.name.clone(), system.timer.clone().context(system.name.clone()).unwrap())).collect();
                (update_phase.clone(), system_timers)
            })
            .collect::<Vec<_>>();

        let total_systems_delta_time = system_timers.iter().map(|(_, timers)| timers.iter().map(|(_, timer)| timer.get_total_duration()).sum::<f32>()).sum::<f32>();
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
                    ui.add(egui::Label::new(format!("Systems: {}, Total delta time: {:.3} ms", system_count, total_systems_delta_time)));
                    let mut phase_state = HashMap::new();
                    for (update_phase, system_timers) in system_timers.iter() {
                        let phase_duration = system_timers.iter().map(|(_, timer)| timer.get_total_duration()).sum::<f32>();
                        let phase_id = format!("phase_{}", update_phase);
                        let is_phase_open = phase_state.get(&phase_id).copied().unwrap_or(false);
                        let phase_response = egui::CollapsingHeader::new(format!("Update Phase: {} {:.3} ms", update_phase, phase_duration))
                            .id_source(&phase_id)
                            .default_open(is_phase_open)
                            .show(ui, |ui| {
                                for (i, (system_name, system_timer)) in system_timers.iter().enumerate() {
                                    let mut timer_state = HashMap::new();
                                    Self::render_timer_tree_with_state(ui, &system_timer.records, &mut timer_state);
                                }
                            });
                        let header_response = phase_response.header_response;
                        if header_response.clicked() {
                            phase_state.insert(phase_id, !is_phase_open);
                        }
                    }
                });
        });

        ui
    }

    fn render_timer_tree(ui: &mut Ui, records: &IndexMap<String, TimerRecord>, indent: usize) {
        let mut state = HashMap::new();
        Self::render_timer_tree_with_state(ui, records, &mut state);
    }

    fn render_timer_tree_with_state(ui: &mut Ui, records: &IndexMap<String, TimerRecord>, state: &mut HashMap<String, bool>) {
        for (label, record) in records {
            let summary = format!("{:.3} ms - {}", record.duration, label);
            if record.subrecords.is_empty() {
                ui.label(summary);
            } else {
                let id = format!("_{}", label);
                let is_open = state.get(&id).copied().unwrap_or(false);
                let response = egui::CollapsingHeader::new(summary)
                    .id_source(&id)
                    .default_open(is_open)
                    .show(ui, |ui| {
                        Self::render_timer_tree_with_state(ui, &record.subrecords, state);
                    });
                let header_response = response.header_response;
                if header_response.clicked() {
                    state.insert(id, !is_open);
                }
            }
        }
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