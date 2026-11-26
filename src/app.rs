//! eframe main window: master-password setup/unlock flow, the project tree,
//! and the per-project git operations panel.
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

/// Maximum number of entries kept in the bottom output log.
const LOG_MAX: usize = 200;

/// Remote name used by the Push button (credential injection lands in Task 10).
const DEFAULT_PUSH_REMOTE: &str = "origin";

/// One rendered git invocation in the bottom read-only output area.
struct LogEntry {
    /// False when the call failed (non-zero exit or git could not run).
    ok: bool,
    /// Header line: what ran and how it ended.
    title: String,
    /// Combined stdout/stderr, or the io error message.
    text: String,
}

/// Result of one read-only git query, shown in the right panel's status area.
struct StatusSection {
    ok: bool,
    text: String,
}

/// Latest snapshot of the three read-only queries for the selected project.
struct StatusSnapshot {
    status: Option<StatusSection>,
    branches: Option<StatusSection>,
    tags: Option<StatusSection>,
}

/// Combine the stdout/stderr of a git result into displayable text.
fn git_output_text(out: &git::GitOutput) -> String {
    let stdout = out.stdout.trim_end();
    let stderr = out.stderr.trim_end();
    let mut text = String::new();
    if !stdout.is_empty() {
        text.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(stderr);
    }
    if text.is_empty() {
        text.push_str("（无输出）");
    }
    text
}

/// Convert a finished git call into a status-area section.
fn status_section(out: &git::GitOutput) -> StatusSection {
    StatusSection {
        ok: out.ok(),
        text: git_output_text(out),
    }
}

/// Render one status-area section: plain text on success, red on failure.
fn show_section(ui: &mut egui::Ui, title: &str, section: Option<&StatusSection>) {
    ui.add_space(4.0);
    ui.strong(title);
    match section {
        Some(s) if s.ok => {
            ui.monospace(&s.text);
        }
        Some(s) => {
            ui.colored_label(egui::Color32::RED, &s.text);
        }
        None => {
            ui.label("（尚未加载）");
        }
    }
}

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

    // Right panel: per-project git operations.
    /// Latest status/branch/tag snapshot for the selected project.
    status: Option<StatusSnapshot>,
    /// Project id the snapshot belongs to; refreshed on selection change.
    status_loaded_for: Option<i64>,
    commit_msg: String,
    new_branch: String,
    tag_name: String,
    tag_msg: String,
    /// Bottom read-only log, one entry per git invocation.
    log: Vec<LogEntry>,
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
            status: None,
            status_loaded_for: None,
            commit_msg: String::new(),
            new_branch: String::new(),
            tag_name: String::new(),
            tag_msg: String::new(),
            log: Vec::new(),
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
        self.sync_status_for_selection();
        self.show_operations_panel(ctx);
        self.show_output_area(ctx);
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

    // ---------------------------------------------------- project operations

    /// Refresh the status area when the selection changes.
    fn sync_status_for_selection(&mut self) {
        if self.selected == self.status_loaded_for {
            return;
        }
        self.status_loaded_for = self.selected;
        match self.selected {
            Some(_) => self.refresh_status(),
            None => self.status = None,
        }
    }

    fn show_operations_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("operations_panel")
            .default_width(380.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("操作");
                ui.separator();
                if self.selected_project().is_none() {
                    ui.label("从左侧选择一个项目，即可查看其 git 状态并执行操作。");
                    return;
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.show_status_area(ui);
                        ui.add_space(8.0);
                        ui.separator();
                        self.show_commit_area(ui);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("拉取（pull）").clicked() {
                                self.do_pull();
                            }
                            if ui.button("推送（push）").clicked() {
                                self.do_push();
                            }
                        });
                        ui.add_space(8.0);
                        ui.separator();
                        self.show_branch_area(ui);
                        ui.add_space(8.0);
                        self.show_tag_area(ui);
                    });
            });
    }

    /// 状态区：status --porcelain 输出 + 分支列表 + 标签列表。
    fn show_status_area(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("状态");
            if ui.button("刷新").clicked() {
                self.refresh_status();
            }
        });
        let Some(snapshot) = self.status.as_ref() else {
            ui.label("（尚未加载）");
            return;
        };
        show_section(ui, "工作区（status）", snapshot.status.as_ref());
        show_section(ui, "分支（branch）", snapshot.branches.as_ref());
        show_section(ui, "标签（tag）", snapshot.tags.as_ref());
    }

    /// Commit 消息输入框 + Commit 按钮（空消息禁用）。
    fn show_commit_area(&mut self, ui: &mut egui::Ui) {
        ui.strong("提交（commit）");
        ui.add_sized(
            [ui.available_width(), 22.0],
            egui::TextEdit::singleline(&mut self.commit_msg).hint_text("提交信息（必填）"),
        );
        let can_commit = !self.commit_msg.trim().is_empty();
        if ui
            .add_enabled(can_commit, egui::Button::new("提交"))
            .clicked()
        {
            self.do_commit();
        }
    }

    /// “新建分支”：名称输入 → create_branch + checkout。
    fn show_branch_area(&mut self, ui: &mut egui::Ui) {
        ui.strong("新建分支");
        ui.add_sized(
            [ui.available_width(), 22.0],
            egui::TextEdit::singleline(&mut self.new_branch).hint_text("分支名称（必填）"),
        );
        let can_create = !self.new_branch.trim().is_empty();
        if ui
            .add_enabled(can_create, egui::Button::new("创建并切换"))
            .clicked()
        {
            self.do_new_branch();
        }
    }

    /// “打 tag”：名称 + 可选 message → create_tag。
    fn show_tag_area(&mut self, ui: &mut egui::Ui) {
        ui.strong("打标签（tag）");
        ui.add_sized(
            [ui.available_width(), 22.0],
            egui::TextEdit::singleline(&mut self.tag_name).hint_text("标签名称（必填）"),
        );
        ui.add_sized(
            [ui.available_width(), 22.0],
            egui::TextEdit::singleline(&mut self.tag_msg)
                .hint_text("标签说明（可选；填写则创建附注标签）"),
        );
        let can_tag = !self.tag_name.trim().is_empty();
        if ui.add_enabled(can_tag, egui::Button::new("打标签")).clicked() {
            self.do_tag();
        }
    }

    /// 底部只读输出区：每次 git 调用一条记录，失败为红色。
    fn show_output_area(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("output_area")
            .default_height(180.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("输出");
                    if ui.button("清空").clicked() {
                        self.log.clear();
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.log.is_empty() {
                            ui.label("（暂无输出：对项目执行 git 操作后，结果会显示在这里）");
                        }
                        for entry in &self.log {
                            let color = if entry.ok {
                                egui::Color32::LIGHT_GREEN
                            } else {
                                egui::Color32::RED
                            };
                            ui.colored_label(color, &entry.title);
                            ui.monospace(&entry.text);
                            ui.add_space(6.0);
                        }
                    });
            });
    }

    // -------------------------------------------------- git operation handlers

    /// Append a finished git invocation to the bottom output log.
    fn log_git_output(&mut self, label: &str, out: &git::GitOutput) {
        self.log.push(LogEntry {
            ok: out.ok(),
            title: format!("{label}（退出码 {}）", out.code),
            text: git_output_text(out),
        });
        self.trim_log();
    }

    /// Append a failed-to-run entry (git could not be spawned / cwd missing).
    fn log_git_error(&mut self, label: &str, err: &std::io::Error) {
        self.log.push(LogEntry {
            ok: false,
            title: format!("{label}（执行失败）"),
            text: format!("无法运行 git：{err}"),
        });
        self.trim_log();
    }

    fn trim_log(&mut self) {
        if self.log.len() > LOG_MAX {
            let excess = self.log.len() - LOG_MAX;
            self.log.drain(0..excess);
        }
    }

    /// Run one read-only query, log its result, and return the section to show.
    fn run_readonly(
        &mut self,
        label: &str,
        result: std::io::Result<git::GitOutput>,
    ) -> StatusSection {
        match result {
            Ok(out) => {
                self.log_git_output(label, &out);
                status_section(&out)
            }
            Err(e) => {
                self.log_git_error(label, &e);
                StatusSection {
                    ok: false,
                    text: format!("无法运行 git：{e}"),
                }
            }
        }
    }

    /// Re-run the three read-only queries for the selected project.
    fn refresh_status(&mut self) {
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let path = Path::new(&project.path);
        let snapshot = StatusSnapshot {
            status: Some(self.run_readonly(
                "git status --porcelain=v1 -b",
                git::status_porcelain(path),
            )),
            branches: Some(self.run_readonly(
                "git branch --format=%(refname:short)",
                git::branch_list(path),
            )),
            tags: Some(self.run_readonly("git tag --list", git::tag_list(path))),
        };
        self.status = Some(snapshot);
    }

    /// Commit 按钮：`commit_all`，成功后清空输入并刷新状态区。
    fn do_commit(&mut self) {
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let message = self.commit_msg.trim().to_string();
        if message.is_empty() {
            return; // the button is disabled; defensive only
        }
        let label = format!("git add -A + git commit -m {message}");
        match git::commit_all(Path::new(&project.path), &message) {
            Ok(out) => {
                self.log_git_output(&label, &out);
                if out.ok() {
                    self.commit_msg.clear();
                }
            }
            Err(e) => self.log_git_error(&label, &e),
        }
        self.refresh_status();
    }

    /// Pull 按钮：`git pull`，随后刷新状态区。
    fn do_pull(&mut self) {
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        match git::pull(Path::new(&project.path)) {
            Ok(out) => self.log_git_output("git pull", &out),
            Err(e) => self.log_git_error("git pull", &e),
        }
        self.refresh_status();
    }

    /// Push 按钮：先取当前分支，再推送到 origin（凭据注入见 Task 10）。
    fn do_push(&mut self) {
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let path = Path::new(&project.path);
        const HEAD_LABEL: &str = "git rev-parse --abbrev-ref HEAD";
        let branch = match git::run(path, &["rev-parse", "--abbrev-ref", "HEAD"]) {
            Ok(out) => {
                self.log_git_output(HEAD_LABEL, &out);
                if !out.ok() {
                    return; // e.g. unborn branch; the stderr is already logged
                }
                out.stdout.trim().to_string()
            }
            Err(e) => {
                self.log_git_error(HEAD_LABEL, &e);
                return;
            }
        };
        if branch.is_empty() {
            self.log.push(LogEntry {
                ok: false,
                title: "git push".to_string(),
                text: "无法确定当前分支：rev-parse 输出为空".to_string(),
            });
            self.trim_log();
            return;
        }
        let label = format!("git push {DEFAULT_PUSH_REMOTE} {branch}");
        match git::push(path, DEFAULT_PUSH_REMOTE, &branch) {
            Ok(out) => self.log_git_output(&label, &out),
            Err(e) => self.log_git_error(&label, &e),
        }
    }

    /// “新建分支”：`create_branch` 成功后 `checkout`，随后刷新状态区。
    fn do_new_branch(&mut self) {
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let name = self.new_branch.trim().to_string();
        if name.is_empty() {
            return; // the button is disabled; defensive only
        }
        let path = Path::new(&project.path);
        let create_label = format!("git branch {name}");
        let created = match git::create_branch(path, &name) {
            Ok(out) => {
                self.log_git_output(&create_label, &out);
                out.ok()
            }
            Err(e) => {
                self.log_git_error(&create_label, &e);
                false
            }
        };
        if !created {
            return;
        }
        let checkout_label = format!("git checkout {name}");
        match git::checkout(path, &name) {
            Ok(out) => {
                self.log_git_output(&checkout_label, &out);
                if out.ok() {
                    self.new_branch.clear();
                }
            }
            Err(e) => self.log_git_error(&checkout_label, &e),
        }
        self.refresh_status();
    }

    /// “打 tag”：有 message 则附注标签，否则轻量标签，随后刷新状态区。
    fn do_tag(&mut self) {
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let name = self.tag_name.trim().to_string();
        if name.is_empty() {
            return; // the button is disabled; defensive only
        }
        let trimmed_msg = self.tag_msg.trim();
        let message = if trimmed_msg.is_empty() {
            None
        } else {
            Some(trimmed_msg.to_string())
        };
        let label = if message.is_some() {
            format!("git tag -a {name} -m <说明>")
        } else {
            format!("git tag {name}")
        };
        match git::create_tag(Path::new(&project.path), &name, message.as_deref()) {
            Ok(out) => {
                self.log_git_output(&label, &out);
                if out.ok() {
                    self.tag_name.clear();
                    self.tag_msg.clear();
                }
            }
            Err(e) => self.log_git_error(&label, &e),
        }
        self.refresh_status();
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
