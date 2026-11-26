#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // release 版隐藏控制台黑窗

mod app;
mod browser;
mod crypto;
mod db;
mod git;
mod password;

use eframe::egui;

/// 加载系统中文字体作为回退，避免 CJK 字符显示为方块。
/// 依次尝试微软雅黑 / 黑体 / 宋体；都不存在时保持默认字体（英文界面可用）。
fn load_cjk_fonts(ctx: &egui::Context) {
    let candidates: [(&str, u32); 3] = [
        ("C:\\Windows\\Fonts\\msyh.ttc", 0),   // 微软雅黑
        ("C:\\Windows\\Fonts\\simhei.ttf", 0), // 黑体
        ("C:\\Windows\\Fonts\\simsun.ttc", 0), // 宋体
    ];
    for (path, index) in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            let mut data = egui::FontData::from_owned(bytes);
            data.index = index;
            fonts
                .font_data
                .insert("cjk".to_owned(), data);
            // 追加为回退字体：拉丁字符仍用默认字体，中文落到 CJK 字体
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .push("cjk".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".to_owned());
            ctx.set_fonts(fonts);
            return;
        }
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "git-pm",
        options,
        Box::new(|cc| {
            load_cjk_fonts(&cc.egui_ctx);
            Ok(Box::new(app::GitPmApp::new()))
        }),
    )
}
