fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "git-pm",
        options,
        Box::new(|_cc| Ok(Box::new(GitPmApp::default()))),
    )
}

#[derive(Default)]
struct GitPmApp;

impl eframe::App for GitPmApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                ui.label("Hello git-pm");
            });
        });
    }
}
