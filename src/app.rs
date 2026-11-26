//! eframe main window: master-password setup/unlock flow, the project tree,
//! and the per-project git operations panel.
//!
//! This module is UI glue only: it binds the domain modules (`crypto`, `db`,
//! `git`) to egui widgets. Every fallible call is matched and its error is
//! rendered as a message; nothing here unwraps or panics.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::browser;
use crate::crypto::{self, CryptoError, VaultKeys};
use crate::db::{self, Project};
use crate::git;
use crate::password::{self, GenOptions};

/// `meta` key that marks an initialized vault (written by `crypto::setup_master`).
const VERIFIER_KEY: &str = "verifier";

/// `meta` key storing the persisted UI theme.
const THEME_KEY: &str = "theme";

/// Bytes stored under `meta[THEME_KEY]` for the light theme. Anything else
/// (missing key, other value, read failure) falls back to the dark default.
const THEME_LIGHT: &[u8] = b"light";

/// Local database, created next to the program's working directory.
const DB_PATH: &str = "gitpm.db";

/// Width of the centered password inputs on the Setup/Unlock screens.
const FIELD_WIDTH: f32 = 320.0;

/// Maximum number of entries kept in the bottom output log.
const LOG_MAX: usize = 200;

/// Remote name used by Push/Pull and as the default credential label. When a
/// credential is stored under this label and the configured remote URL is
/// https, the UI injects it into the URL for that one git call.
const DEFAULT_PUSH_REMOTE: &str = "origin";

/// Width of the single-line inputs inside the modal dialogs.
const DIALOG_FIELD_WIDTH: f32 = 360.0;

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

/// Credential data prepared for exactly one in-memory authenticated git call.
struct InjectedAuth {
    /// `https://user:pass@host/...`. Only ever passed to `git` as an argument;
    /// never logged, rendered, or stored anywhere.
    url: String,
    /// The remote URL without credentials, used to redact git output.
    plain_url: String,
    /// Raw and percent-encoded passwords, both scrubbed from git output.
    password: String,
    encoded_password: String,
}

impl InjectedAuth {
    /// `(from, to)` replacement pairs that scrub credentials from git output.
    fn redact_pairs(&self) -> Vec<(String, String)> {
        vec![
            (self.url.clone(), self.plain_url.clone()),
            (self.password.clone(), "******".to_string()),
            (self.encoded_password.clone(), "******".to_string()),
        ]
    }
}

/// Percent-encode a username or password for the userinfo part of a URL
/// (anything outside the RFC 3986 unreserved set becomes `%XX`).
fn percent_encode_userinfo(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Split a stored secret (`user:pass`) into its two parts.
fn split_credential_secret(secret: &str) -> (String, String) {
    match secret.split_once(':') {
        Some((user, pass)) => (user.to_string(), pass.to_string()),
        None => (String::new(), secret.to_string()),
    }
}

/// Embed `user:pass` into an https URL (`https://user:pass@host/...`).
///
/// Returns `None` when the URL is not https or already carries userinfo; the
/// git call must then run without injected credentials.
fn inject_https_userinfo(url: &str, user: &str, pass: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return None;
    }
    Some(format!(
        "https://{}:{}@{rest}",
        percent_encode_userinfo(user),
        percent_encode_userinfo(pass)
    ))
}

/// Apply `(from, to)` replacements, skipping empty needles.
fn redact_text(text: String, pairs: &[(String, String)]) -> String {
    let mut text = text;
    for (from, to) in pairs {
        if !from.is_empty() {
            text = text.replace(from.as_str(), to.as_str());
        }
    }
    text
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

/// UI theme, persisted in the meta table under [`THEME_KEY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Theme {
    Dark,
    Light,
}

/// Which window a password-generator UI is rendered in. Both instances share
/// the same options/result state; only the copy inside the credential dialog
/// offers「填入密码框」. Their open/close flags are fully independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GenWindow {
    /// The generator opened from the credential dialog's「生成…」button.
    CredDialog,
    /// The standalone generator opened from the top-bar「工具」menu.
    ToolWindow,
}

pub struct GitPmApp {
    conn: Option<rusqlite::Connection>,
    /// Root cause captured when `gitpm.db` could not be opened.
    open_error: Option<String>,
    phase: Phase,
    /// UI theme, restored from the meta table at startup.
    theme: Theme,
    error: Option<String>,

    setup_pw1: String,
    setup_pw2: String,
    unlock_pw: String,

    // Session keys from the master password, used to read/write credentials.
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

    // Central panel: file browser of the selected project.
    /// The directory currently being browsed; `None` = not yet initialized (or no project selected).
    browser_path: Option<PathBuf>,
    /// Contents of the current directory, cached from `browser::list_dir`.
    browser_entries: Vec<browser::Entry>,
    /// The most recent directory read failure message, rendered inside the panel; never panics.
    browser_err: Option<String>,

    // Clone dialog.
    show_clone_dialog: bool,
    clone_url: String,
    /// Parent directory chosen via rfd; the repo lands in a subdirectory of it.
    clone_parent: Option<PathBuf>,
    clone_subdir: String,
    /// Error shown inside the clone dialog.
    clone_error: Option<String>,

    // Credential dialog.
    show_cred_dialog: bool,
    cred_label: String,
    cred_user: String,
    cred_pass: String,
    /// Error shown inside the credential dialog.
    cred_error: Option<String>,

    // Password generator dialogs (one from the credential dialog, one
    // standalone from the top-bar「工具」menu), sharing options and result.
    show_gen_dialog: bool,
    show_tool_gen_window: bool,
    gen_opts: GenOptions,
    /// Last generated password, masked, until copied or filled into the
    /// credential dialog's password field. Cleared whenever either generator
    /// window closes.
    gen_result: String,
    gen_error: Option<String>,
}

impl GitPmApp {
    pub fn new() -> Self {
        let mut app = Self {
            conn: None,
            open_error: None,
            phase: Phase::Setup,
            theme: Theme::Dark,
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
            browser_path: None,
            browser_entries: Vec::new(),
            browser_err: None,
            show_clone_dialog: false,
            clone_url: String::new(),
            clone_parent: None,
            clone_subdir: String::new(),
            clone_error: None,
            show_cred_dialog: false,
            cred_label: String::new(),
            cred_user: String::new(),
            cred_pass: String::new(),
            cred_error: None,
            show_gen_dialog: false,
            show_tool_gen_window: false,
            gen_opts: GenOptions::default(),
            gen_result: String::new(),
            gen_error: None,
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
                // Persisted theme preference: `light` wins, everything else
                // (missing key or read failure) keeps the dark default.
                match db::get_meta(&conn, THEME_KEY) {
                    Ok(Some(value)) => {
                        if value == THEME_LIGHT {
                            app.theme = Theme::Light;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        // Cosmetic only; keep any more important startup error.
                        if app.error.is_none() {
                            app.error = Some(format!("读取主题偏好失败，已回退到暗黑主题：{e}"));
                        }
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
                    ui.separator();
                    // 防御性兜底：已选中项目但浏览器尚未初始化时，重置到项目根目录。
                    if self.browser_path.is_none() {
                        self.reset_browser_to(Path::new(&project.path));
                    }
                    self.show_browser(ui, Path::new(&project.path));
                }
                None => {
                    ui.heading("git-pm");
                    ui.label("从左侧选择一个项目，或点击「添加项目…」将一个 git 仓库加入列表。");
                }
            }
            self.show_error(ui);
        });
        self.show_remove_confirm(ctx);
        self.show_clone_dialog(ctx);
        self.show_cred_dialog(ctx);
        self.show_gen_dialog(ctx);
        self.show_tool_generator(ctx);
    }

    fn show_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("git-pm");
                ui.separator();
                if ui.button("添加项目…").clicked() {
                    self.pick_and_add_project();
                }
                if ui.button("克隆仓库…").clicked() {
                    self.open_clone_dialog();
                }
                let has_selection = self.selected.is_some();
                if ui
                    .add_enabled(has_selection, egui::Button::new("移除项目"))
                    .clicked()
                {
                    self.confirm_remove = true;
                    self.error = None;
                }
                ui.separator();
                self.show_menu_bar(ui);
            });
        });
    }

    /// 顶栏菜单栏（egui::menu::bar）：主题切换（暗黑/明亮）与工具（独立
    /// 密码生成器）。
    fn show_menu_bar(&mut self, ui: &mut egui::Ui) {
        let mut chosen_theme: Option<Theme> = None;
        let mut open_generator = false;
        egui::menu::bar(ui, |ui| {
            ui.menu_button("主题", |ui| {
                if ui.button("暗黑").clicked() {
                    chosen_theme = Some(Theme::Dark);
                }
                if ui.button("明亮").clicked() {
                    chosen_theme = Some(Theme::Light);
                }
            });
            ui.menu_button("工具", |ui| {
                if ui.button("密码生成器").clicked() {
                    open_generator = true;
                }
            });
        });
        if let Some(theme) = chosen_theme {
            self.switch_theme(ui.ctx(), theme);
        }
        if open_generator {
            self.open_tool_generator();
        }
    }

    /// 切换主题：立即生效，并尽力写入 meta 表持久化；写失败仅记入日志区，
    /// 不影响本次切换。
    fn switch_theme(&mut self, ctx: &egui::Context, theme: Theme) {
        self.theme = theme;
        ctx.set_visuals(match theme {
            Theme::Dark => egui::Visuals::dark(),
            Theme::Light => egui::Visuals::light(),
        });
        let Some(conn) = self.conn.as_ref() else {
            self.log_theme_save_error("数据库未打开");
            return;
        };
        let value: &[u8] = match theme {
            Theme::Dark => b"dark",
            Theme::Light => THEME_LIGHT,
        };
        if let Err(e) = db::set_meta(conn, THEME_KEY, value) {
            self.log_theme_save_error(&e.to_string());
        }
    }

    /// 主题偏好写库失败：仅在底部日志区提示一条失败记录。
    fn log_theme_save_error(&mut self, text: &str) {
        self.log.push(LogEntry {
            ok: false,
            title: "主题偏好保存失败".to_string(),
            text: text.to_string(),
        });
        self.trim_log();
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
                        if resp.clicked() && self.selected != Some(id) {
                            // 选中项目变化：浏览器重置为新项目的根目录。
                            self.selected = Some(id);
                            self.reset_browser_to(Path::new(&path));
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
                            if ui.button("凭据…").clicked() {
                                self.open_cred_dialog();
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

    // -------------------------------------------------------- file browser

    /// 把浏览器重置到给定根目录（选中项目变化时调用）。
    /// 根目录读取失败时仍指向该目录并清空条目，错误显示在面板内。
    fn reset_browser_to(&mut self, root: &Path) {
        self.browser_err = None;
        match browser::list_dir(root) {
            Ok(entries) => {
                self.browser_path = Some(root.to_path_buf());
                self.browser_entries = entries;
            }
            Err(e) => {
                self.browser_path = Some(root.to_path_buf());
                self.browser_entries = Vec::new();
                self.browser_err = Some(format!("无法读取目录：{e}"));
            }
        }
    }

    /// 进入某个目录：成功则更新路径与条目；失败仅记录 `browser_err`
    /// （保留当前视图，显示在面板内，不 panic）。
    fn load_browser_dir(&mut self, path: PathBuf) {
        match browser::list_dir(&path) {
            Ok(entries) => {
                self.browser_path = Some(path);
                self.browser_entries = entries;
                self.browser_err = None;
            }
            Err(e) => self.browser_err = Some(format!("无法进入目录：{e}")),
        }
    }

    /// 中央文件浏览器：只读路径行 +「上一级」+ 多列条目列表。
    fn show_browser(&mut self, ui: &mut egui::Ui, root: &Path) {
        ui.horizontal(|ui| {
            ui.strong("位置：");
            let path_text = self
                .browser_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "（未加载）".to_string());
            // 只读文本（Label 不可编辑）；固定宽度截断，避免长路径把按钮挤出面板。
            ui.add_sized(
                [(ui.available_width() - 96.0).max(0.0), 18.0],
                egui::Label::new(egui::RichText::new(&path_text).monospace())
                    .wrap_mode(egui::TextWrapMode::Truncate),
            );
            let at_root = self.browser_path.as_ref().is_none_or(|p| p == root);
            if ui
                .add_enabled(!at_root, egui::Button::new("上一级"))
                .clicked()
            {
                // 到达项目根时按钮禁用；这里只会在根目录之下被触发。
                if let Some(parent) = self.browser_path.as_ref().and_then(|p| p.parent()) {
                    self.load_browser_dir(parent.to_path_buf());
                }
            }
        });
        ui.separator();

        if let Some(err) = &self.browser_err {
            ui.colored_label(egui::Color32::RED, err);
            ui.add_space(4.0);
        }

        // 先克隆条目快照，点击处理器即可再借用 self（与项目列表同理由）。
        let rows: Vec<(String, bool, u64, Option<String>, bool)> = self
            .browser_entries
            .iter()
            .map(|e| {
                (
                    e.name.clone(),
                    e.is_dir,
                    e.size,
                    e.modified.clone(),
                    e.is_hidden,
                )
            })
            .collect();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if rows.is_empty() && self.browser_err.is_none() {
                    ui.label("（空目录）");
                }
                egui::Grid::new("file_browser_grid")
                    .striped(true)
                    .num_columns(4)
                    .spacing(egui::vec2(16.0, 6.0))
                    .max_col_width(320.0)
                    .show(ui, |ui| {
                        ui.strong("名称");
                        ui.strong("类型");
                        ui.strong("大小");
                        ui.strong("修改时间");
                        ui.end_row();

                        for (name, is_dir, size, modified, is_hidden) in rows {
                            // 隐藏项（.git 等）正常显示但整行视觉弱化，仍可进入。
                            let dim = |rt: egui::RichText| {
                                if is_hidden {
                                    rt.weak()
                                } else {
                                    rt
                                }
                            };
                            // 目录行：四格均可点击（点击行内任意位置进入）；
                            // 文件行：无动作（仅 hover 感应）。
                            let sense = if is_dir {
                                egui::Sense::click()
                            } else {
                                egui::Sense::hover()
                            };
                            let name_resp = ui.add(
                                egui::Label::new(dim(egui::RichText::new(&name)))
                                    .wrap_mode(egui::TextWrapMode::Truncate)
                                    .sense(sense),
                            );
                            let type_resp = ui.add(
                                egui::Label::new(dim(egui::RichText::new(if is_dir {
                                    "目录"
                                } else {
                                    "文件"
                                })))
                                .sense(sense),
                            );
                            let size_text = if is_dir {
                                "—".to_string()
                            } else {
                                browser::fmt_size(size)
                            };
                            let size_resp = ui
                                .add(egui::Label::new(dim(egui::RichText::new(size_text))).sense(sense));
                            let time_resp = ui.add(
                                egui::Label::new(dim(egui::RichText::new(
                                    modified.unwrap_or_else(|| "—".to_string()),
                                )))
                                .sense(sense),
                            );
                            ui.end_row();
                            if is_dir {
                                let row = name_resp
                                    .union(type_resp)
                                    .union(size_resp)
                                    .union(time_resp);
                                if row.clicked() {
                                    if let Some(target) =
                                        self.browser_path.as_ref().map(|p| p.join(&name))
                                    {
                                        self.load_browser_dir(target);
                                    }
                                }
                                row.on_hover_cursor(egui::CursorIcon::PointingHand);
                            }
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

    /// Log a git call whose output may contain injected credentials: the text
    /// is scrubbed with `pairs` before it is stored.
    fn log_git_output_redacted(
        &mut self,
        label: &str,
        out: &git::GitOutput,
        pairs: &[(String, String)],
    ) {
        let text = redact_text(git_output_text(out), pairs);
        self.log.push(LogEntry {
            ok: out.ok(),
            title: format!("{label}（退出码 {}）", out.code),
            text,
        });
        self.trim_log();
    }

    /// Prepare credential injection for one push/pull of the selected project.
    ///
    /// Returns `Some` only when a credential is stored under
    /// [`DEFAULT_PUSH_REMOTE`] and the configured remote URL is a plain https
    /// URL. Every failure degrades to `None` (the git call then runs without
    /// credentials); credential load failures are reported in the output log.
    /// Neither the secret nor the injected URL is ever logged or rendered.
    fn injected_auth(&mut self, project_id: i64, path: &Path) -> Option<InjectedAuth> {
        let secret = {
            let keys = self.keys.as_ref()?;
            let conn = self.conn.as_ref()?;
            match db::get_credential(conn, keys, project_id, DEFAULT_PUSH_REMOTE) {
                Ok(Some(secret)) => secret,
                Ok(None) => return None,
                Err(e) => {
                    self.log.push(LogEntry {
                        ok: false,
                        title: format!("读取凭据（{DEFAULT_PUSH_REMOTE}）失败"),
                        // CryptoError messages never contain the secret.
                        text: e.to_string(),
                    });
                    self.trim_log();
                    return None;
                }
            }
        };
        let remote_url = match git::run(path, &["remote", "get-url", DEFAULT_PUSH_REMOTE]) {
            Ok(out) if out.ok() => out.stdout.trim().to_string(),
            // git's own failure surfaces on the subsequent push/pull call.
            Ok(_) => return None,
            Err(e) => {
                self.log_git_error(&format!("git remote get-url {DEFAULT_PUSH_REMOTE}"), &e);
                return None;
            }
        };
        let (user, pass) = split_credential_secret(&secret);
        let encoded_password = percent_encode_userinfo(&pass);
        let url = inject_https_userinfo(&remote_url, &user, &pass)?;
        Some(InjectedAuth {
            url,
            plain_url: remote_url,
            password: pass,
            encoded_password,
        })
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

    /// Pull 按钮：凭据存在且 origin 为 https 时注入凭据拉取，随后刷新状态区。
    fn do_pull(&mut self) {
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let path = Path::new(&project.path);
        match self.injected_auth(project.id, path) {
            Some(auth) => {
                // `git pull <url>` without a refspec would merge the remote's
                // HEAD; passing the current branch keeps the default `git pull`
                // behaviour of merging the same-named upstream branch.
                const BRANCH_LABEL: &str = "git rev-parse --abbrev-ref HEAD";
                let branch = match git::run(path, &["rev-parse", "--abbrev-ref", "HEAD"]) {
                    Ok(out) => {
                        self.log_git_output(BRANCH_LABEL, &out);
                        if !out.ok() {
                            return; // e.g. detached HEAD; stderr is already logged
                        }
                        out.stdout.trim().to_string()
                    }
                    Err(e) => {
                        self.log_git_error(BRANCH_LABEL, &e);
                        return;
                    }
                };
                if branch.is_empty() {
                    self.log.push(LogEntry {
                        ok: false,
                        title: "git pull".to_string(),
                        text: "无法确定当前分支：rev-parse 输出为空".to_string(),
                    });
                    self.trim_log();
                    return;
                }
                let label = format!("git pull {DEFAULT_PUSH_REMOTE} {branch}（凭据已注入）");
                match git::run(path, &["pull", &auth.url, &branch]) {
                    Ok(out) => {
                        self.log_git_output_redacted(&label, &out, &auth.redact_pairs());
                    }
                    Err(e) => self.log_git_error(&label, &e),
                }
            }
            None => match git::pull(path) {
                Ok(out) => self.log_git_output("git pull", &out),
                Err(e) => self.log_git_error("git pull", &e),
            },
        }
        self.refresh_status();
    }

    /// Push 按钮：先取当前分支，再推送到 origin（凭据存在且 remote 为 https
    /// 时注入凭据）。
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
        match self.injected_auth(project.id, path) {
            Some(auth) => {
                let label = format!("{label}（凭据已注入）");
                match git::push(path, &auth.url, &branch) {
                    Ok(out) => {
                        self.log_git_output_redacted(&label, &out, &auth.redact_pairs());
                    }
                    Err(e) => self.log_git_error(&label, &e),
                }
            }
            None => match git::push(path, DEFAULT_PUSH_REMOTE, &branch) {
                Ok(out) => self.log_git_output(&label, &out),
                Err(e) => self.log_git_error(&label, &e),
            },
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

    // -------------------------------------------------------------- dialogs

    /// Top-bar「克隆仓库…」：打开克隆对话框（空白表单）。
    fn open_clone_dialog(&mut self) {
        self.error = None;
        self.clone_url.clear();
        self.clone_parent = None;
        self.clone_subdir.clear();
        self.clone_error = None;
        self.show_clone_dialog = true;
    }

    fn close_clone_dialog(&mut self) {
        self.show_clone_dialog = false;
        self.clone_url.clear();
        self.clone_parent = None;
        self.clone_subdir.clear();
        self.clone_error = None;
    }

    fn pick_clone_parent(&mut self) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("选择克隆父目录")
            .pick_folder()
        {
            self.clone_parent = Some(dir);
        }
    }

    fn show_clone_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_clone_dialog {
            return;
        }
        let mut open = self.show_clone_dialog;
        egui::Window::new("克隆仓库")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("克隆一个 git 仓库；成功后自动加入项目列表。");
                ui.add_space(6.0);
                ui.add_sized(
                    [DIALOG_FIELD_WIDTH, 22.0],
                    egui::TextEdit::singleline(&mut self.clone_url)
                        .hint_text("仓库 URL（https://… 或 git@…）"),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("选择父目录…").clicked() {
                        self.pick_clone_parent();
                    }
                    match &self.clone_parent {
                        Some(dir) => ui.monospace(dir.display().to_string()),
                        None => ui.label("（未选择父目录）"),
                    }
                });
                ui.add_space(6.0);
                ui.add_sized(
                    [DIALOG_FIELD_WIDTH, 22.0],
                    egui::TextEdit::singleline(&mut self.clone_subdir)
                        .hint_text("子目录名（必填，不能含路径分隔符）"),
                );
                if let Some(err) = &self.clone_error {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::RED, err);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        self.close_clone_dialog();
                    }
                    let ready = !self.clone_url.trim().is_empty()
                        && self.clone_parent.is_some()
                        && !self.clone_subdir.trim().is_empty();
                    if ui.add_enabled(ready, egui::Button::new("克隆")).clicked() {
                        self.do_clone();
                    }
                });
            });
        // Closed via the window's X button.
        if !open {
            self.close_clone_dialog();
        }
    }

    /// 克隆按钮：校验输入 → 查重 → `git::clone_repo` → 成功后自动入树。
    fn do_clone(&mut self) {
        self.clone_error = None;
        let url = self.clone_url.trim().to_string();
        let subdir = self.clone_subdir.trim().to_string();
        if url.is_empty() || subdir.is_empty() {
            self.clone_error = Some("请填写仓库 URL、父目录和子目录名".into());
            return;
        }
        if subdir.contains('/') || subdir.contains('\\') || subdir == "." || subdir == ".." {
            self.clone_error = Some("子目录名不能包含路径分隔符".into());
            return;
        }
        let Some(parent) = self.clone_parent.clone() else {
            self.clone_error = Some("请先选择克隆父目录".into());
            return;
        };
        let target = parent.join(&subdir);
        let target_str = target.to_string_lossy().into_owned();

        // 查重：目标目录不存在，且不在项目列表中。
        if target.exists() {
            self.clone_error = Some(format!("目标目录已存在：{target_str}"));
            return;
        }
        {
            let Some(conn) = self.conn.as_ref() else {
                self.clone_error = Some("数据库未打开".into());
                return;
            };
            match db::list_projects(conn) {
                Ok(projects) => {
                    if projects.iter().any(|p| p.path == target_str) {
                        self.clone_error = Some(format!("该项目已在列表中：{target_str}"));
                        return;
                    }
                }
                Err(e) => {
                    self.clone_error = Some(format!("读取项目列表失败：{e}"));
                    return;
                }
            }
        }

        let label = format!("git clone {url} {target_str}");
        match git::clone_repo(&url, &target) {
            Err(e) => {
                self.log_git_error(&label, &e);
                self.clone_error = Some(format!("无法运行 git：{e}"));
            }
            Ok(out) => {
                self.log_git_output(&label, &out);
                if !out.ok() {
                    let detail = git_output_text(&out);
                    self.clone_error = Some(format!("克隆失败（退出码 {}）：{detail}", out.code));
                    return;
                }
                let name = target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| subdir.clone());
                let Some(conn) = self.conn.as_ref() else {
                    self.clone_error = Some("数据库未打开".into());
                    return;
                };
                match db::add_project(conn, &name, &target_str) {
                    Ok(_) => self.close_clone_dialog(),
                    Err(e) => self.clone_error = Some(format!("添加项目失败：{e}")),
                }
                self.reload_projects();
            }
        }
    }

    /// 操作面板「凭据…」：打开凭据对话框，默认管理 origin 标签。
    fn open_cred_dialog(&mut self) {
        self.error = None;
        self.cred_label = DEFAULT_PUSH_REMOTE.to_string();
        self.cred_user.clear();
        self.cred_pass.clear();
        self.cred_error = None;
        self.show_cred_dialog = true;
    }

    fn close_cred_dialog(&mut self) {
        self.show_cred_dialog = false;
        self.cred_label.clear();
        self.cred_user.clear();
        self.cred_pass.clear();
        self.cred_error = None;
    }

    fn show_cred_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_cred_dialog {
            return;
        }
        let Some(project_name) = self.selected_project().map(|p| p.name.clone()) else {
            self.show_cred_dialog = false;
            return;
        };
        let mut open = self.show_cred_dialog;
        egui::Window::new(format!("凭据 — {project_name}"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("凭据加密后保存在本机数据库；push/pull 到 origin 的 https 远端时自动注入。");
                ui.add_space(6.0);
                ui.add_sized(
                    [DIALOG_FIELD_WIDTH, 22.0],
                    egui::TextEdit::singleline(&mut self.cred_label).hint_text("标签（如 origin）"),
                );
                ui.add_space(6.0);
                ui.add_sized(
                    [DIALOG_FIELD_WIDTH, 22.0],
                    egui::TextEdit::singleline(&mut self.cred_user).hint_text("用户名"),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [DIALOG_FIELD_WIDTH - 80.0, 22.0],
                        egui::TextEdit::singleline(&mut self.cred_pass)
                            .password(true)
                            .hint_text("密码（掩码显示）"),
                    );
                    if ui.button("生成…").clicked() {
                        self.open_gen_dialog();
                    }
                });
                if let Some(err) = &self.cred_error {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::RED, err);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("关闭").clicked() {
                        self.close_cred_dialog();
                    }
                    if ui.button("删除凭据").clicked() {
                        self.delete_credential();
                    }
                    if ui.button("保存").clicked() {
                        self.save_credential();
                    }
                });
            });
        // Closed via the window's X button.
        if !open {
            self.close_cred_dialog();
        }
    }

    /// 保存按钮：校验后 `store_credential`（用户名与密码合并为一条加密记录）。
    fn save_credential(&mut self) {
        self.cred_error = None;
        let label = self.cred_label.trim().to_string();
        let user = self.cred_user.trim().to_string();
        let pass = self.cred_pass.clone();
        if label.is_empty() || user.is_empty() || pass.is_empty() {
            self.cred_error = Some("请填写标签、用户名和密码".into());
            return;
        }
        let Some(project_id) = self.selected_project().map(|p| p.id) else {
            self.cred_error = Some("未选择项目".into());
            return;
        };
        let Some(keys) = self.keys.as_ref() else {
            self.cred_error = Some("保险库未解锁".into());
            return;
        };
        let Some(conn) = self.conn.as_ref() else {
            self.cred_error = Some("数据库未打开".into());
            return;
        };
        let secret = format!("{user}:{pass}");
        match db::store_credential(conn, keys, project_id, &label, &secret) {
            Ok(()) => self.close_cred_dialog(),
            Err(e) => self.cred_error = Some(format!("保存凭据失败：{e}")),
        }
    }

    /// 删除按钮：`delete_credential` 删除当前标签下的凭据。
    fn delete_credential(&mut self) {
        self.cred_error = None;
        let label = self.cred_label.trim().to_string();
        if label.is_empty() {
            self.cred_error = Some("请先填写要删除的标签".into());
            return;
        }
        let Some(project_id) = self.selected_project().map(|p| p.id) else {
            self.cred_error = Some("未选择项目".into());
            return;
        };
        let Some(conn) = self.conn.as_ref() else {
            self.cred_error = Some("数据库未打开".into());
            return;
        };
        match db::delete_credential(conn, project_id, &label) {
            Ok(_) => self.close_cred_dialog(),
            Err(e) => self.cred_error = Some(format!("删除凭据失败：{e}")),
        }
    }

    /// 凭据对话框「生成…」：打开密码生成器弹窗。
    fn open_gen_dialog(&mut self) {
        self.gen_error = None;
        self.gen_result.clear();
        self.show_gen_dialog = true;
    }

    /// 顶栏「工具 → 密码生成器」：打开独立生成器窗口（清空上次结果）。
    fn open_tool_generator(&mut self) {
        self.gen_error = None;
        self.gen_result.clear();
        self.show_tool_gen_window = true;
    }

    /// 关闭指定生成器窗口并清空生成结果（「关闭」按钮与 X 按钮都走这里）。
    fn close_generator(&mut self, window: GenWindow) {
        match window {
            GenWindow::CredDialog => self.show_gen_dialog = false,
            GenWindow::ToolWindow => self.show_tool_gen_window = false,
        }
        self.gen_result.clear();
    }

    fn show_gen_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_gen_dialog {
            return;
        }
        let mut open = self.show_gen_dialog;
        egui::Window::new("密码生成器")
            .id(egui::Id::new("cred_password_generator"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                self.show_generator_ui(ui, GenWindow::CredDialog);
            });
        // Closed via the window's X button: drop the generated password.
        if !open {
            self.close_generator(GenWindow::CredDialog);
        }
    }

    /// 顶栏「工具 → 密码生成器」打开的独立窗口（与凭据对话框内的弹窗共享
    /// 选项与结果状态，但打开/关闭互不干扰）。
    fn show_tool_generator(&mut self, ctx: &egui::Context) {
        if !self.show_tool_gen_window {
            return;
        }
        let mut open = self.show_tool_gen_window;
        egui::Window::new("密码生成器")
            .id(egui::Id::new("tool_password_generator"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                self.show_generator_ui(ui, GenWindow::ToolWindow);
            });
        // Closed via the window's X button: drop the generated password.
        if !open {
            self.close_generator(GenWindow::ToolWindow);
        }
    }

    /// 密码生成器共享 UI：长度滑块 8–64（默认 20）、大写/小写/数字固定启用、
    /// 特殊字符可选、生成 / 复制 / 关闭；仅凭据对话框内额外提供「填入密码框」。
    fn show_generator_ui(&mut self, ui: &mut egui::Ui, window: GenWindow) {
        ui.add(egui::Slider::new(&mut self.gen_opts.length, 8..=64).text("长度"));
        // upper/lower/digits 固定启用（生成器要求至少一类字符）。
        let mut fixed_upper = true;
        ui.add_enabled(
            false,
            egui::Checkbox::new(&mut fixed_upper, "大写字母（固定启用）"),
        );
        let mut fixed_lower = true;
        ui.add_enabled(
            false,
            egui::Checkbox::new(&mut fixed_lower, "小写字母（固定启用）"),
        );
        let mut fixed_digits = true;
        ui.add_enabled(
            false,
            egui::Checkbox::new(&mut fixed_digits, "数字（固定启用）"),
        );
        ui.checkbox(&mut self.gen_opts.special, "特殊字符");
        if ui.button("生成").clicked() {
            self.generate_password();
        }
        ui.add_sized(
            [DIALOG_FIELD_WIDTH, 22.0],
            egui::TextEdit::singleline(&mut self.gen_result)
                .password(true)
                .hint_text("（点击「生成」后显示在此，掩码显示）"),
        );
        if let Some(err) = &self.gen_error {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::RED, err);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let has_result = !self.gen_result.is_empty();
            if ui.add_enabled(has_result, egui::Button::new("复制")).clicked() {
                ui.ctx().copy_text(self.gen_result.clone());
            }
            if window == GenWindow::CredDialog
                && ui
                    .add_enabled(has_result, egui::Button::new("填入密码框"))
                    .clicked()
            {
                self.cred_pass = std::mem::take(&mut self.gen_result);
                self.close_generator(window);
            }
            if ui.button("关闭").clicked() {
                self.close_generator(window);
            }
        });
    }

    /// 生成按钮：`password::generate`，结果只保存在内存中。
    fn generate_password(&mut self) {
        self.gen_error = None;
        match password::generate(&self.gen_opts) {
            Ok(pw) => self.gen_result = pw,
            Err(e) => self.gen_error = Some(format!("生成密码失败：{e}")),
        }
    }

    // --------------------------------------------------------------- shared

    /// Apply the persisted theme every frame; a cheap no-op once applied, and
    /// it re-asserts the choice if anything else resets the style.
    fn apply_theme(&self, ctx: &egui::Context) {
        let visuals = match self.theme {
            Theme::Dark => egui::Visuals::dark(),
            Theme::Light => egui::Visuals::light(),
        };
        if ctx.style().visuals != visuals {
            ctx.set_visuals(visuals);
        }
    }

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
        self.apply_theme(ctx);
        match self.phase {
            Phase::Setup => self.show_setup(ctx),
            Phase::Unlock => self.show_unlock(ctx),
            Phase::Main => self.show_main(ctx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_userinfo_escapes_specials() {
        assert_eq!(percent_encode_userinfo(""), "");
        assert_eq!(percent_encode_userinfo("safe-_X9~"), "safe-_X9~");
        assert_eq!(percent_encode_userinfo("user@example.com"), "user%40example.com");
        assert_eq!(percent_encode_userinfo("p%ss:wo/rd?1"), "p%25ss%3Awo%2Frd%3F1");
    }

    #[test]
    fn split_credential_secret_parts() {
        let (user, pass) = split_credential_secret("alice:s3cret:pass");
        assert_eq!(user, "alice");
        assert_eq!(pass, "s3cret:pass");
        let (user, pass) = split_credential_secret("token-only");
        assert_eq!(user, "");
        assert_eq!(pass, "token-only");
    }

    #[test]
    fn inject_https_userinfo_and_guards() {
        let url = inject_https_userinfo("https://github.com/acme/app.git", "u", "p");
        assert_eq!(url.as_deref(), Some("https://u:p@github.com/acme/app.git"));

        // Non-https URLs are never touched.
        assert!(inject_https_userinfo("git@github.com:acme/app.git", "u", "p").is_none());
        assert!(inject_https_userinfo("http://github.com/acme/app.git", "u", "p").is_none());
        // URLs that already carry userinfo are left alone.
        assert!(inject_https_userinfo("https://old@github.com/acme/app.git", "u", "p").is_none());
    }

    #[test]
    fn redact_text_replaces_pairs_and_skips_empty() {
        let text = "To https://u:s3cret@host/repo.git\nbad password s3cret, retry".to_string();
        let pairs = vec![
            (
                "https://u:s3cret@host/repo.git".to_string(),
                "https://host/repo.git".to_string(),
            ),
            ("s3cret".to_string(), "******".to_string()),
        ];
        let out = redact_text(text, &pairs);
        assert_eq!(out, "To https://host/repo.git\nbad password ******, retry");

        // An empty needle must never be replaced (that would insert everywhere).
        let empty = redact_text("unchanged".to_string(), &[("".to_string(), "x".to_string())]);
        assert_eq!(empty, "unchanged");
    }

    #[test]
    fn injected_auth_redact_pairs_cover_url_and_passwords() {
        let auth = InjectedAuth {
            url: "https://u:p%40ss@host/repo.git".to_string(),
            plain_url: "https://host/repo.git".to_string(),
            password: "p@ss".to_string(),
            encoded_password: "p%40ss".to_string(),
        };
        let pairs = auth.redact_pairs();
        assert_eq!(pairs.len(), 3);
        let out = redact_text("fatal: unable to access 'https://u:p%40ss@host/repo.git'".into(), &pairs);
        assert_eq!(out, "fatal: unable to access 'https://host/repo.git'");
    }
}
