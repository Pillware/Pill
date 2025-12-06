use std::collections::HashMap;

use crate::{
    ecs::{
        components::{Component, GlobalComponent, GlobalComponentStorage},
        systems, UpdatePhase,
    },
    engine::Engine,
};

use egui::Ui;
use indexmap::IndexMap;
use pill_core::{PillTypeMapKey, Timer, TimerRecord};

use anyhow::{Context, Error, Result};

pub struct EguiManagerComponent {
    collapsing_state: HashMap<String, bool>,
}

impl EguiManagerComponent {
    pub fn new() -> Self {
        Self {
            collapsing_state: HashMap::new(),
        }
    }

    pub fn get_ui(engine: &mut Engine) -> Box<dyn Fn(&egui::Context) + Send> {
        let entity_count = engine
            .scene_manager
            .get_active_scene()
            .unwrap()
            .entities
            .len();
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
                        (
                            system.name.clone(),
                            system.timer.clone().context(system.name.clone()).unwrap(),
                        )
                    })
                    .collect();
                (update_phase.clone(), system_timers)
            })
            .collect::<Vec<_>>();

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
        let window_w = engine.window_size.width;
        let window_h = engine.window_size.height;

        // Snapshot draw call counter from last frame
        let total_draw_calls: Option<u64> = engine
            .system_manager
            .peek_system_timer(
                crate::config::RENDERING_SYSTEM.name,
                crate::config::RENDERING_SYSTEM.update_phase,
            )
            .ok()
            .and_then(|t| t)
            .and_then(|t| t.get_counter("draw_calls"));

        // Get egui_client for vignette parameter control
        let egui_client = engine
            .get_global_component::<crate::ecs::RenderStateComponent>()
            .ok()
            .and_then(|rs| rs.egui_client.clone());

        let ui = Box::new(move |ui: &egui::Context| {
            egui::Window::new("PillEngine")
                .default_open(true)
                .resizable(true)
                .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2]) // optional: prevent auto shrink
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui.add(egui::Button::new("Click me")).clicked() {
                                    println!("PRESSED");
                                }
                                ui.label(format!("FPS {:.1}", 1000.0 / frame_delta_time));
                                ui.label(format!("{}x{}", window_w, window_h));
                            });
                            ui.add(egui::Label::new(format!(
                                "Frame Delta Time: {:.4} ms",
                                frame_delta_time
                            )));
                            ui.add(egui::Label::new(format!("Entities: {}", entity_count)));
                            if let Some(dc) = total_draw_calls {
                                ui.add(egui::Label::new(format!("Draw calls: {}", dc)));
                            }
                            ui.separator();

                            // Vignette controls
                            if let Some(ref client) = egui_client {
                                ui.label("Vignette Post-Processing:");
                                let mut intensity = *client.vignette_intensity.lock().unwrap();
                                let mut smoothness = *client.vignette_smoothness.lock().unwrap();
                                let mut radius = *client.vignette_radius.lock().unwrap();

                                if ui
                                    .add(
                                        egui::Slider::new(&mut intensity, 0.0..=1.0)
                                            .text("Intensity"),
                                    )
                                    .changed()
                                {
                                    *client.vignette_intensity.lock().unwrap() = intensity;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(&mut smoothness, 0.0..=1.0)
                                            .text("Smoothness"),
                                    )
                                    .changed()
                                {
                                    *client.vignette_smoothness.lock().unwrap() = smoothness;
                                }
                                if ui
                                    .add(egui::Slider::new(&mut radius, 0.1..=2.0).text("Radius"))
                                    .changed()
                                {
                                    *client.vignette_radius.lock().unwrap() = radius;
                                }
                                ui.separator();

                                // Depth of Field controls
                                ui.label("Depth of Field:");
                                let mut dof_enabled = *client.dof_enabled.lock().unwrap();
                                let mut focus_distance = *client.dof_focus_distance.lock().unwrap();
                                let mut focus_range = *client.dof_focus_range.lock().unwrap();
                                let mut blur_strength = *client.dof_blur_strength.lock().unwrap();
                                let mut samples = *client.dof_samples.lock().unwrap();

                                if ui.checkbox(&mut dof_enabled, "Enable DOF").changed() {
                                    *client.dof_enabled.lock().unwrap() = dof_enabled;
                                }

                                if ui
                                    .add(
                                        egui::Slider::new(&mut focus_distance, 0.0..=2.0)
                                            .text("Focus Distance"),
                                    )
                                    .changed()
                                {
                                    *client.dof_focus_distance.lock().unwrap() = focus_distance;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(&mut focus_range, 0.01..=2.5)
                                            .text("Focus Range"),
                                    )
                                    .changed()
                                {
                                    *client.dof_focus_range.lock().unwrap() = focus_range;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(&mut blur_strength, 0.0..=3.0)
                                            .text("Blur Strength"),
                                    )
                                    .changed()
                                {
                                    *client.dof_blur_strength.lock().unwrap() = blur_strength;
                                }
                                if ui
                                    .add(egui::Slider::new(&mut samples, 8..=128).text("Samples"))
                                    .changed()
                                {
                                    *client.dof_samples.lock().unwrap() = samples;
                                }
                                ui.separator();

                                // Chromatic Aberration controls
                                ui.label("Chromatic Aberration:");
                                let mut chroma_intensity = *client.chromatic_aberration_intensity.lock().unwrap();
                                let mut chroma_falloff = *client.chromatic_aberration_radial_falloff.lock().unwrap();

                                if ui
                                    .add(
                                        egui::Slider::new(&mut chroma_intensity, 0.0..=0.02)
                                            .text("Intensity"),
                                    )
                                    .changed()
                                {
                                    *client.chromatic_aberration_intensity.lock().unwrap() = chroma_intensity;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(&mut chroma_falloff, 0.5..=4.0)
                                            .text("Radial Falloff"),
                                    )
                                    .changed()
                                {
                                    *client.chromatic_aberration_radial_falloff.lock().unwrap() = chroma_falloff;
                                }
                                ui.separator();

                                // Color Grading controls
                                ui.label("Color Grading:");
                                let mut contrast = *client.color_grade_contrast.lock().unwrap();
                                let mut brightness = *client.color_grade_brightness.lock().unwrap();
                                let mut saturation = *client.color_grade_saturation.lock().unwrap();
                                let mut curve = *client.color_grade_curve.lock().unwrap();

                                if ui
                                    .add(
                                        egui::Slider::new(&mut contrast, 0.0..=2.0)
                                            .text("Contrast"),
                                    )
                                    .changed()
                                {
                                    *client.color_grade_contrast.lock().unwrap() = contrast;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(&mut brightness, -0.5..=0.5)
                                            .text("Brightness"),
                                    )
                                    .changed()
                                {
                                    *client.color_grade_brightness.lock().unwrap() = brightness;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(&mut saturation, 0.0..=2.0)
                                            .text("Saturation"),
                                    )
                                    .changed()
                                {
                                    *client.color_grade_saturation.lock().unwrap() = saturation;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(&mut curve, 0.5..=2.0)
                                            .text("Tone Curve"),
                                    )
                                    .changed()
                                {
                                    *client.color_grade_curve.lock().unwrap() = curve;
                                }
                                ui.separator();

                                // Bloom controls
                                ui.label("Bloom:");
                                let mut bloom_threshold = *client.bloom_threshold.lock().unwrap();
                                let mut bloom_intensity = *client.bloom_intensity.lock().unwrap();
                                let mut bloom_radius = *client.bloom_radius.lock().unwrap();

                                if ui
                                    .add(
                                        egui::Slider::new(&mut bloom_threshold, 0.0..=1.5)
                                            .text("Threshold"),
                                    )
                                    .changed()
                                {
                                    *client.bloom_threshold.lock().unwrap() = bloom_threshold;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(&mut bloom_intensity, 0.0..=1.0)
                                            .text("Intensity"),
                                    )
                                    .changed()
                                {
                                    *client.bloom_intensity.lock().unwrap() = bloom_intensity;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(&mut bloom_radius, 0.5..=4.0)
                                            .text("Radius"),
                                    )
                                    .changed()
                                {
                                    *client.bloom_radius.lock().unwrap() = bloom_radius;
                                }
                                ui.separator();
                            }

                            ui.add(egui::Label::new(format!(
                                "Systems: {}, Total delta time: {:.3} ms",
                                system_count, total_systems_delta_time
                            )));
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
                                .id_source(&phase_id)
                                .default_open(is_phase_open)
                                .show(ui, |ui| {
                                    for (system_name, timer) in system_timers {
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
        }) as Box<dyn Fn(&egui::Context) + Send>;

        ui
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
            .id_source(&id)
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

    pub(crate) fn update(&mut self, delta_time: f32) -> Result<()> {
        Ok(())
    }
}

impl PillTypeMapKey for EguiManagerComponent {
    type Storage = GlobalComponentStorage<EguiManagerComponent>;
}

impl GlobalComponent for EguiManagerComponent {}
