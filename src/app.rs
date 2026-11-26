//! eframe main window: master-password setup/unlock flow and the project tree.
//!
//! This module is UI glue only: it binds the domain modules (`crypto`, `db`,
//! `git`) to egui widgets. Every fallible call is matched and its error is
//! rendered as a message; nothing here unwraps or panics.

use std::path::Path;

use eframe::egui;

use crate::crypto::{self, CryptoError, VaultKeys};
use crate::db::{self, Project};
use crate::git;

/// `meta` key that marks an initialized vault (written by `crypto::setup_master`).
const VERIFIER_KEY: &str = "verifier";

/// Local database, created next to the program's working directory.
const DB_PATH: &str = "gitpm.db";

/// Width of the centered password inputs on the Setup/Unlock screens.
const FIELD_WIDTH: f32 = 320.0;

/// Which screen the main window currently shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No verifier stored yet: create the master password.
    Setup,
    /// Verifier stored: ask for the master password.
    Unlock,
    /// Vault unlocked: show the project tree.
    Main,
}

pub struct GitPmApp {
    conn: Option<rusqlite::Connection>,
    /// Root cause captured when `gitpm.db` could not be opened.
    open_error: Option<String>,
    phase: Phase,
    error: Option<String>,

    setup_pw1: String,
    setup_pw2: String,
    unlock_pw: String,

    // Session keys from the master password. Unused until the credential
    // panel lands (Task 9/10); kept here so unlocking hands them to Main.
    #[allow(dead_code)]
    keys: Option<VaultKeys>,
    projects: Vec<Project>,
    selected: Option<i64>,
    confirm_remove: bool,
}

impl GitPmApp {
    pub fn new() -> Self {
        let mut app = Self {
            conn: None,
            open_error: None,
            phase: Phase::Setup,
            error: None,
            setup_pw1: String::new(),
            setup_pw2: String::new(),
            unlock_pw: String::new(),
            keys: None,
            projects: Vec::new(),
            selected: None,
            confirm_remove: false,
        };

        match db::init_db(Path::new(DB_PATH)) {
            Ok(conn) => {
                match db::get_meta(&conn, VERIFIER_KEY) {
                    Ok(Some(_)) => app.phase = Phase::Unlock,
                    Ok(None) => app.phase = Phase::Setup,
                    Err(e) => {
                        // We cannot tell whether the vault is initialized, so
                        // stay on Setup: `setup_master` refuses to re-initialize
                        // an existing vault and the error is shown there.
                        app.phase = Phase::Setup;
                        app.error = Some(format!("读取保险库状态失败：{e}"));
                    }
                }
                app.conn = Some(conn);
            }
            Err(e) => {
                let msg = format!("无法打开数据库 {DB_PATH}：{e}");
                app.open_error = Some(msg.clone());
                app.error = Some(msg);
            }
        }
        app
    }

    // ---------------------------------------------------------------- setup

    fn show_setup(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(64.0);
                ui.heading("欢迎使用 git-pm");
                ui.add_space(8.0);
                ui.label("首次使用：请设置一个主密码，用于加密本机保存的凭据。");
                ui.label("主密码无法找回，请务必牢记。");
                ui.add_space(24.0);
                ui.add_sized(
                    [FIELD_WIDTH, 24.0],
                    egui::TextEdit::singleline(&mut self.setup_pw1)
                        .password(true)
                        .hint_text("主密码"),
                );
                ui.add_space(8.0);
                let confirm = ui.add_sized(
                    [FIELD_WIDTH, 24.0],
                    egui::TextEdit::singleline(&mut self.setup_pw2)
                        .password(true)
                        .hint_text("再次输入主密码"),
                );
                let pressed_enter =
                    confirm.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(12.0);
                let clicked = ui
                    .add_sized([FIELD_WIDTH, 32.0], egui::Button::new("设置主密码"))
                    .clicked();
                if clicked || pressed_enter {
                    self.submit_setup();
                }
                self.show_error(ui);
            });
        });
    }

    fn submit_setup(&mut self) {
        self.error = None;
        if self.setup_pw1.is_empty() || self.setup_pw2.is_empty() {
            self.error = Some("请输入主密码并在第二栏重复输入".into());
            return;
        }
        if self.setup_pw1 != self.setup_pw2 {
            self.error = Some("两次输入的主密码不一致，请重新输入".into());
            self.setup_pw2.clear();
            return;
        }
        let Some(conn) = self.conn.as_ref() else {
            self.error = Some(self.open_error.clone().unwrap_or_else(|| "数据库未打开".into()));
            return;
        };

        // The vault may have been initialized by another instance since
        // startup; switch to Unlock instead of failing on the setup guard.
        match db::get_meta(conn, VERIFIER_KEY) {
            Ok(Some(_)) => {
                self.phase = Phase::Unlock;
                self.setup_pw1.clear();
                self.setup_pw2.clear();
                return;
            }
            Ok(None) => {}
            Err(e) => {
                self.error = Some(format!("读取保险库状态失败：{e}"));
                return;
            }
        }

        if let Err(e) = crypto::setup_master(conn, &self.setup_pw1) {
            self.error = Some(format!("设置主密码失败：{e}"));
            return;
        }
        // setup_master does not hand back keys; unlock to start the session.
        match crypto::unlock(conn, &self.setup_pw1) {
            Ok(keys) => {
                self.keys = Some(keys);
                self.setup_pw1.clear();
                self.setup_pw2.clear();
                self.enter_main();
            }
            Err(e) => self.error = Some(format!("解锁刚创建的保险库失败：{e}")),
        }
    }

    // --------------------------------------------------------------- unlock

    fn show_unlock(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading("解锁 git-pm");
                ui.add_space(8.0);
                ui.label("请输入主密码以解锁凭据保险库。");
                ui.add_space(24.0);
                let field = ui.add_sized(
                    [FIELD_WIDTH, 24.0],
                    egui::TextEdit::singleline(&mut self.unlock_pw)
                        .password(true)
                        .hint_text("主密码"),
                );
                let pressed_enter =
                    field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(12.0);
                let clicked = ui
                    .add_sized([FIELD_WIDTH, 32.0], egui::Button::new("解锁"))
                    .clicked();
                if clicked || pressed_enter {
                    self.submit_unlock();
                }
                self.show_error(ui);
            });
        });
    }

    fn submit_unlock(&mut self) {
        self.error = None;
        let Some(conn) = self.conn.as_ref() else {
            self.error = Some(self.open_error.clone().unwrap_or_else(|| "数据库未打开".into()));
            return;
        };
        match crypto::unlock(conn, &self.unlock_pw) {
            Ok(keys) => {
                self.keys = Some(keys);
                self.unlock_pw.clear();
                self.enter_main();
            }
            Err(e) => match e {
                CryptoError::WrongMaster => {
                    self.error = Some("主密码错误，请重试".into());
                    self.unlock_pw.clear();
                }
                other => self.error = Some(format!("解锁失败：{other}")),
            },
        }
    }

    // ----------------------------------------------------------------- main

    fn enter_main(&mut self) {
        self.phase = Phase::Main;
        self.error = None;
        self.reload_projects();
    }

    fn show_main(&mut self, ctx: &egui::Context) {
        self.show_top_bar(ctx);
        self.show_project_list(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.selected_project().cloned() {
                Some(project) => {
                    ui.heading(&project.name);
                    ui.label(format!("路径：{}", project.path));
                    ui.label(format!("添加时间：{}", project.added_at));
                }
                None => {
                    ui.heading("git-pm");
                    ui.label("从左侧选择一个项目，或点击「添加项目…」将一个 git 仓库加入列表。");
                }
            }
            ui.add_space(12.0);
            ui.separator();
            ui.label("项目的 git 操作面板将在后续版本提供。");
            self.show_error(ui);
        });
        self.show_remove_confirm(ctx);
    }

    fn show_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("git-pm");
                ui.separator();
                if ui.button("添加项目…").clicked() {
                    self.pick_and_add_project();
                }
                let has_selection = self.selected.is_some();
                if ui
                    .add_enabled(has_selection, egui::Button::new("移除项目"))
                    .clicked()
                {
                    self.confirm_remove = true;
                    self.error = None;
                }
            });
        });
    }

    fn show_project_list(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("project_list").show(ctx, |ui| {
            ui.heading("项目");
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Clone the row data so the click handler can mutate state.
                    let rows: Vec<(i64, String, String)> = self
                        .projects
                        .iter()
                        .map(|p| (p.id, p.name.clone(), p.path.clone()))
                        .collect();
                    if rows.is_empty() {
                        ui.label("（暂无项目，点击上方「添加项目…」）");
                    }
                    for (id, name, path) in rows {
                        let selected = self.selected == Some(id);
                        let resp = ui.add_sized(
                            [ui.available_width(), 22.0],
                            egui::SelectableLabel::new(selected, name),
                        );
                        if resp.clicked() {
                            self.selected = Some(id);
                        }
                        if !path.is_empty() {
                            resp.on_hover_text(path);
                        }
                    }
                });
        });
    }

    fn show_remove_confirm(&mut self, ctx: &egui::Context) {
        if !self.confirm_remove {
            return;
        }
        let Some((name, id)) = self.selected_project().map(|p| (p.name.clone(), p.id)) else {
            self.confirm_remove = false;
            return;
        };
        let mut open = self.confirm_remove;
        egui::Window::new("移除项目")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("确定要把「{name}」从列表中移除吗？"));
                ui.label("仅从列表移除，不会删除磁盘上的文件。");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        self.confirm_remove = false;
                    }
                    if ui.button("移除").clicked() {
                        self.remove_selected(id);
                    }
                });
            });
        // Closed via the window's X button.
        if !open {
            self.confirm_remove = false;
        }
    }

    fn selected_project(&self) -> Option<&Project> {
        let id = self.selected?;
        self.projects.iter().find(|p| p.id == id)
    }

    fn reload_projects(&mut self) {
        let Some(conn) = self.conn.as_ref() else {
            self.error = Some("数据库未打开".into());
            return;
        };
        match db::list_projects(conn) {
            Ok(projects) => {
                if let Some(sel) = self.selected {
                    if !projects.iter().any(|p| p.id == sel) {
                        self.selected = None;
                    }
                }
                self.projects = projects;
            }
            Err(e) => self.error = Some(format!("读取项目列表失败：{e}")),
        }
    }

    fn pick_and_add_project(&mut self) {
        self.error = None;
        let Some(conn) = self.conn.as_ref() else {
            self.error = Some(self.open_error.clone().unwrap_or_else(|| "数据库未打开".into()));
            return;
        };
        let Some(dir) = rfd::FileDialog::new()
            .set_title("选择 git 仓库目录")
            .pick_folder()
        else {
            return; // user cancelled the dialog; not an error
        };
        let path = dir.to_string_lossy().into_owned();
        if !git::is_repo(&dir) {
            self.error = Some(format!("所选目录不是 git 仓库：{path}"));
            return;
        }
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        match db::list_projects(conn) {
            Ok(projects) => {
                if projects.iter().any(|p| p.path == path) {
                    self.error = Some(format!("该项目已在列表中：{path}"));
                    return;
                }
            }
            Err(e) => {
                self.error = Some(format!("读取项目列表失败：{e}"));
                return;
            }
        }
        match db::add_project(conn, &name, &path) {
            Ok(_) => self.reload_projects(),
            Err(e) => self.error = Some(format!("添加项目失败：{e}")),
        }
    }

    fn remove_selected(&mut self, id: i64) {
        self.confirm_remove = false;
        let Some(conn) = self.conn.as_ref() else {
            self.error = Some("数据库未打开".into());
            return;
        };
        // Only the database row is deleted; the directory on disk is untouched.
        match db::remove_project(conn, id) {
            Ok(_) => {
                self.error = None;
                self.reload_projects();
            }
            Err(e) => self.error = Some(format!("移除项目失败：{e}")),
        }
    }

    // --------------------------------------------------------------- shared

    fn show_error(&self, ui: &mut egui::Ui) {
        if let Some(err) = &self.error {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::RED, err);
        }
    }
}

impl Default for GitPmApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for GitPmApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        match self.phase {
            Phase::Setup => self.show_setup(ctx),
            Phase::Unlock => self.show_unlock(ctx),
            Phase::Main => self.show_main(ctx),
        }
    }
}
