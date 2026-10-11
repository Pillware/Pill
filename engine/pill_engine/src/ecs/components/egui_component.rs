#![cfg(feature = "ui")]

use std::{collections::HashMap, sync::Arc};

use crate::{
    ecs::{
        components::{GlobalComponent, GlobalComponentStorage},
        BuildStatus, BuildStatusIndicatorComponent, EguiClient, UpdatePhase,
    },
    engine::Engine,
    internal::CompileMode,
};

use egui::{Color32, Ui};
use pill_core::{PillTypeMapKey, Timer, TimerRecord};

use pill_core::{ErrorContext, Result};

type DebugUiCallback = Arc<dyn Fn(&egui::Context) + Send + Sync + 'static>;

struct RegisteredUi {
    id: String,
    callback: DebugUiCallback,
}

fn format_record(buf: &mut String, record: &TimerRecord, depth: usize) {
    buf.push_str(&format!(
        "{}{:.3} ms  {}\n",
        "  ".repeat(depth),
        record.duration,
        record.label
    ));
    for sub in &record.subrecords {
        format_record(buf, sub, depth + 1);
    }
}

fn format_timers(system_timers: &[(UpdatePhase, Vec<(String, Timer)>)]) -> String {
    let mut buf = String::new();
    for (phase, timers) in system_timers {
        let total: f32 = timers.iter().map(|(_, t)| t.total_duration()).sum();
        buf.push_str(&format!("Phase: {}  {:.3} ms\n", phase, total));
        for (name, timer) in timers {
            buf.push_str(&format!("  {}\n", name));
            for record in &timer.records {
                format_record(&mut buf, record, 2);
            }
        }
    }
    buf
}

pub struct EguiComponent {
    pub egui_client: Arc<EguiClient>,
    collapsing_state: HashMap<String, bool>,
    game_overlay: Option<Arc<dyn Fn(&egui::Context) + Send + Sync>>,
    registered_ui: Vec<RegisteredUi>,
}

impl Default for EguiComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl EguiComponent {
    pub fn new() -> Self {
        Self {
            egui_client: EguiClient::new(),
            collapsing_state: HashMap::new(),
            game_overlay: None,
            registered_ui: Vec::new(),
        }
    }

    pub fn set_overlay(&mut self, f: impl Fn(&egui::Context) + Send + Sync + 'static) {
        self.game_overlay = Some(Arc::new(f));
    }

    pub fn register_ui<F>(&mut self, id: impl Into<String>, callback: F)
    where
        F: Fn(&egui::Context) + Send + Sync + 'static,
    {
        let id = id.into();
        let callback: DebugUiCallback = Arc::new(callback);

        // update existing or add a new
        if let Some(existing) = self.registered_ui.iter_mut().find(|e| e.id == id) {
            existing.callback = callback;
        } else {
            self.registered_ui.push(RegisteredUi { id, callback });
        }
    }

    pub fn clear_registered_ui(&mut self) {
        self.registered_ui.clear();
    }

    pub fn snapshot_registered_ui(&self) -> Vec<DebugUiCallback> {
        self.registered_ui
            .iter()
            .map(|entry| Arc::clone(&entry.callback))
            .collect()
    }

    pub fn get_ui(engine: &mut Engine) -> Result<Box<dyn Fn(&egui::Context) + Send>> {
        let game_overlay = engine
            .get_global_component::<EguiComponent>()
            .ok()
            .and_then(|c| c.game_overlay.clone());

        // No active scene yet (e.g. while loading): show 0 instead of failing the UI
        let entity_count = engine
            .scene_manager
            .get_active_scene()
            .map(|scene| scene.entities.len())
            .unwrap_or(0);
        let system_count = engine
            .system_manager
            .update_phases
            .iter()
            .map(|(_, systems)| systems.len())
            .sum::<usize>();

        let system_timers: Vec<(UpdatePhase, Vec<(String, Timer)>)> = engine
            .system_manager
            .update_phases
            .iter()
            .map(|(update_phase, systems)| {
                let system_timers = systems
                    .iter()
                    .map(|(_, system)| {
                        Ok((
                            system.name.clone(),
                            system.timer.clone().context(system.name.clone())?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok((update_phase.clone(), system_timers))
            })
            .collect::<Result<Vec<_>>>()?;

        let total_systems_delta_time = system_timers
            .iter()
            .map(|(_, timers)| {
                timers
                    .iter()
                    .map(|(_, timer)| timer.total_duration())
                    .sum::<f32>()
            })
            .sum::<f32>();
        let frame_delta_time = engine.frame_delta_time;

        let build_mode = engine.process.mode.as_str();
        let build_target = engine.process.target.as_str();
        let build_status = engine
            .get_global_component::<BuildStatusIndicatorComponent>()?
            .last_build_status;

        let registered_ui = engine
            .get_global_component::<EguiComponent>()?
            .snapshot_registered_ui();

        let ui: Box<dyn Fn(&egui::Context) + Send> = Box::new(move |ui: &egui::Context| {
            if let Some(ref overlay) = game_overlay {
                overlay(ui);
            }
            egui::Window::new("Pill Engine")
                .default_open(true)
                .resizable(true)
                .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(egui::Label::new(format!("Mode: {}", build_mode)));
                        ui.add(egui::Label::new(format!("Target: {}", build_target)));
                        if build_mode == CompileMode::HotReload.as_str() {
                            ui.add(egui::Label::new("Build Status:"));
                            let color = match build_status {
                                BuildStatus::Pass => Color32::GREEN,
                                BuildStatus::Fail => Color32::RED,
                                BuildStatus::Warning => Color32::YELLOW,
                            };
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 12.0, color);
                        }
                    });
                });

            egui::Window::new("Details")
                .default_open(false)
                .resizable(true)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2]) // optional: prevent auto shrink
                        .show(ui, |ui| {
                            if ui.add(egui::Button::new("Click me")).clicked() {
                                println!("PRESSED");
                            }
                            ui.add(egui::Label::new(format!("Mode: {}", build_mode)));
                            ui.add(egui::Label::new(format!("Target: {}", build_target)));
                            ui.add(egui::Label::new(format!(
                                "FPS {}",
                                1000.0 / frame_delta_time
                            )));
                            ui.add(egui::Label::new(format!(
                                "Frame Delta Time: {:.4} ms",
                                frame_delta_time
                            )));
                            ui.add(egui::Label::new(format!("Entities: {}", entity_count)));
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.add(egui::Label::new(format!(
                                    "Systems: {}, Total delta time: {:.3} ms",
                                    system_count, total_systems_delta_time
                                )));
                                if ui.small_button("[C]").clicked() {
                                    ui.ctx().copy_text(format_timers(&system_timers));
                                }
                            });
                            let mut phase_state = HashMap::new();
                            for (update_phase, system_timers) in system_timers.iter() {
                                let phase_duration = system_timers
                                    .iter()
                                    .map(|(_, timer)| timer.total_duration())
                                    .sum::<f32>();

                                let phase_id = format!("phase_{}", update_phase);
                                let is_phase_open = *phase_state.get(&phase_id).unwrap_or(&true);

                                let header = egui::CollapsingHeader::new(format!(
                                    "Update Phase: {} {:.4} ms",
                                    update_phase, phase_duration
                                ))
                                .id_salt(&phase_id)
                                .default_open(is_phase_open)
                                .show(ui, |ui| {
                                    for (_system_name, timer) in system_timers {
                                        let mut state = HashMap::new();
                                        for record in &timer.records {
                                            Self::render_timer_tree_with_state(
                                                ui, record, &mut state,
                                            );
                                        }
                                    }
                                });

                                if header.header_response.clicked() {
                                    phase_state.insert(phase_id, !is_phase_open);
                                }
                            }
                        });
                });

            for callback in &registered_ui {
                callback(ui);
            }
        });

        Ok(ui)
    }

    pub fn render_timer_tree_with_state(
        ui: &mut Ui,
        record: &TimerRecord,
        state: &mut HashMap<String, bool>,
    ) {
        let summary = format!("{:.3} ms - {}", record.duration, record.label);
        if record.subrecords.is_empty() {
            ui.label(summary);
        } else {
            let id = format!("_{}", record.label);
            let is_open = state.get(&id).copied().unwrap_or(false);
            let response = egui::CollapsingHeader::new(
                egui::RichText::new(summary)
                    .text_style(egui::TextStyle::Body)
                    .color(ui.visuals().text_color()),
            )
            .id_salt(&id)
            .default_open(is_open)
            .show(ui, |ui| {
                for sub in &record.subrecords {
                    Self::render_timer_tree_with_state(ui, sub, state);
                }
            });
            let header_response = response.header_response;
            if header_response.clicked() {
                state.insert(id, !is_open);
            }
        }
    }

    pub(crate) fn update(&mut self, _delta_time: f32) -> Result<()> {
        Ok(())
    }
}

impl PillTypeMapKey for EguiComponent {
    type Storage = GlobalComponentStorage<EguiComponent>;
}

impl GlobalComponent for EguiComponent {}
