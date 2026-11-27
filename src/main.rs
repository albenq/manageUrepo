#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // release 版隐藏控制台黑窗

mod app;
mod browser;
mod crypto;
mod db;
mod git;
mod i18n;
mod loc;
mod password;
mod scan;
mod svn;

use eframe::egui;

/// Windows 常见 CJK 字体候选：(路径, .ttc 集合内 face 索引)。
const WINDOWS_FONTS: [(&str, u32); 3] = [
    ("C:\\Windows\\Fonts\\msyh.ttc", 0),   // 微软雅黑
    ("C:\\Windows\\Fonts\\simhei.ttf", 0), // 黑体
    ("C:\\Windows\\Fonts\\simsun.ttc", 0), // 宋体
];

/// Linux 常见 CJK 字体候选（Debian/Ubuntu/Arch/Fedora/openSUSE 常见打包路径）。
const LINUX_FONTS: [(&str, u32); 6] = [
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0), // Debian/Ubuntu fonts-noto-cjk
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),      // Arch noto-fonts-cjk
    ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),         // Debian fonts-wqy-microhei
    ("/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc", 0), // Fedora/openSUSE wqy-microhei
    ("/usr/share/fonts/truetype/arphic/uming.ttc", 0),             // AR PL UMing（文鼎明体）
    ("/usr/share/fonts/droid/DroidSansFallbackFull.ttf", 0),       // 旧发行版 Droid Sans Fallback
];

/// 合并字体候选列表：当前平台的常见路径排在前面；
/// 另一平台的路径在该系统上不存在，探测时自然跳过，无害。
fn font_candidates() -> Vec<(&'static str, u32)> {
    let mut list: Vec<(&'static str, u32)> = Vec::new();
    if cfg!(windows) {
        list.extend(WINDOWS_FONTS);
        list.extend(LINUX_FONTS);
    } else {
        list.extend(LINUX_FONTS);
        list.extend(WINDOWS_FONTS);
    }
    list
}

/// 加载系统中文字体作为回退，避免 CJK 字符显示为方块。
/// 按当前平台顺序依次尝试候选字体（Windows: 雅黑/黑体/宋体；
/// Linux: Noto CJK/WQY/UMing/Droid）；都不存在时保持默认字体（英文界面可用）。
fn load_cjk_fonts(ctx: &egui::Context) {
    for (path, index) in font_candidates() {
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

/// 诊断日志：追加一行到 exe 同目录的 `git-pm.log`（GUI 子系统下 panic
/// 与启动错误无处可看，Win7 排障靠它）。写失败时静默放弃——诊断设施
/// 决不能反过来让程序崩溃。
pub fn log_line(msg: &str) {
    use std::io::Write;
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let path = dir.join("git-pm.log");
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path)
    else {
        return;
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = writeln!(f, "[{ts}] {msg}");
}

/// 把 panic 信息写入日志（GUI 子系统看不到 stderr）。
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        log_line(&format!("PANIC: {info}"));
    }));
}

/// 用 Win32 消息框显示致命启动错误（日志之外多一条用户可见的通道）。
#[cfg(windows)]
fn show_error_box(title: &str, msg: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_SETFOREGROUND,
    };
    let to_wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            to_wide(msg).as_ptr(),
            to_wide(title).as_ptr(),
            MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(not(windows))]
fn show_error_box(_title: &str, _msg: &str) {}

fn main() -> eframe::Result<()> {
    install_panic_hook();
    log_line("startup begin");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "LocalRepoManager",
        options,
        Box::new(|cc| {
            log_line("creator callback: loading fonts + app state");
            load_cjk_fonts(&cc.egui_ctx);
            let app = app::GitPmApp::new();
            log_line("app state ready");
            Ok(Box::new(app))
        }),
    );
    match &result {
        Ok(()) => log_line("run_native returned normally"),
        Err(e) => {
            log_line(&format!("run_native FAILED: {e}"));
            show_error_box(
                "LocalRepoManager",
                &format!("启动失败 / Failed to start:\n{e}\n\n详见同目录 git-pm.log"),
            );
        }
    }
    result
}
