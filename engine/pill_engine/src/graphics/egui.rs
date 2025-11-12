pub struct EguiUI<'a> {
    frame_delta_time: &'a f32, // Use a reference to frame_delta_time
}

impl<'a> EguiUI<'a> {
    fn render(&self, ui: &egui::Context) {
        egui::Window::new("PillEngine")
            .default_open(true)
            .resizable(true)
            .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
            .show(ui, |ui| {
                if ui.add(egui::Button::new("Click me")).clicked() {
                    println!("PRESSED");
                    println!("{}", self.frame_delta_time);
                }
            });
    }
}