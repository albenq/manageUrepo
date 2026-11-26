mod app;
mod crypto;
mod db;
mod git;
mod password;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "git-pm",
        options,
        Box::new(|_cc| Ok(Box::new(app::GitPmApp::new()))),
    )
}
