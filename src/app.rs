//! eframe main window: master-password setup/unlock flow, the project tree,
//! and the per-project git operations panel.
//!
//! This module is UI glue only: it binds the domain modules (`crypto`, `db`,
//! `git`) to egui widgets. Every fallible call is matched and its error is
//! rendered as a message; nothing here unwraps or panics.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::browser;
use crate::crypto::{self, CryptoError, VaultKeys};
use crate::db::{self, Project};
use crate::git;
use crate::i18n::{self, Lang};
use crate::password::{self, GenOptions};

/// t() + 占位符替换：把文案中的 `{name}` 依次替换为 pairs 的值，用于带
/// 动态参数的句子级文案；占位符名未提供时保留原文（便于发现缺漏）。
fn tf(lang: Lang, key: &'static str, pairs: &[(&str, &str)]) -> String {
    let mut out = i18n::t(lang, key).to_string();
    for (name, value) in pairs {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// egui 0.28 does not ship the `egui::icons::ICON_*` constants (the module
/// was added in later egui releases; verified against the vendored 0.28.1
/// source, which has no `icons` module). These local constants mirror the
/// upstream naming so upgrading is a mechanical substitution. Glyphs are
/// Unicode symbols rendered by the bundled NotoEmoji-Regular font or the CJK
/// fallback loaded in `main.rs`.
mod icons {
    /// 添加项目
    pub const ICON_ADD: &str = "＋";
    /// 移除项目
    pub const ICON_REMOVE: &str = "➖";
    /// 克隆仓库
    pub const ICON_CLONE: &str = "📥";
    /// 刷新状态
    pub const ICON_REFRESH: &str = "🔄";
    /// 提交（commit）
    pub const ICON_COMMIT: &str = "💾";
    /// 拉取（pull）
    pub const ICON_PULL: &str = "⬇";
    /// 推送（push）
    pub const ICON_PUSH: &str = "⬆";
    /// 凭据
    pub const ICON_KEY: &str = "🔑";
    /// 新建分支
    pub const ICON_BRANCH: &str = "🌿";
    /// 打标签
    pub const ICON_TAG: &str = "🏷";
    /// 提交历史
    pub const ICON_HISTORY: &str = "🕘";
}

/// `meta` key that marks an initialized vault (written by `crypto::setup_master`).
const VERIFIER_KEY: &str = "verifier";

/// `meta` key storing the persisted UI theme.
const THEME_KEY: &str = "theme";

/// Bytes stored under `meta[THEME_KEY]` for the light theme. Anything else
/// (missing key, other value, read failure) falls back to the dark default.
const THEME_LIGHT: &[u8] = b"light";

/// 项目树「未分组」节点的稳定 key：`group_projects_by_parent` 返回它，
/// UI 层再经 `t()` 翻译为当前语言的展示文案。
const GROUP_UNGROUPED_KEY: &str = "group-ungrouped";

/// Local database, created next to the program's working directory.
const DB_PATH: &str = "gitpm.db";

/// Width of the centered password inputs on the Setup/Unlock screens.
const FIELD_WIDTH: f32 = 320.0;

/// Maximum number of entries kept in the bottom output log.
const LOG_MAX: usize = 200;

/// Row height of the central file browser table (single-line labels).
const BROWSER_ROW_HEIGHT: f32 = 18.0;

/// Log label of the read-only `git status --porcelain=v1 -b` query.
const LABEL_STATUS: &str = "git status --porcelain=v1 -b";

/// Log label of the read-only `git branch --format=%(refname:short)` query.
const LABEL_BRANCHES: &str = "git branch --format=%(refname:short)";

/// Log label of the read-only `git tag --list` query.
const LABEL_TAGS: &str = "git tag --list";

/// Log label of `git rev-parse --abbrev-ref HEAD` (pull/push prerequisite).
const LABEL_HEAD: &str = "git rev-parse --abbrev-ref HEAD";

/// How often the UI thread wakes up to drain worker results while a git
/// operation runs (keeps the animated progress bar and channel polling going).
const BUSY_REPAINT_MS: u64 = 50;

/// Remote name used by Push/Pull and as the default credential label. When a
/// credential is stored under this label and the configured remote URL is
/// https, the UI injects it into the URL for that one git call.
const DEFAULT_PUSH_REMOTE: &str = "origin";

/// Width of the single-line inputs inside the modal dialogs.
const DIALOG_FIELD_WIDTH: f32 = 360.0;

/// 提交历史窗口首次打开时加载的条数。
const HISTORY_FIRST_PAGE: usize = 10;

/// 提交历史窗口「加载更多」每次追加的条数。
const HISTORY_PAGE_SIZE: usize = 20;

/// 修改/重命名状态标记的颜色（金琥珀色：纯黄在明亮主题下不可读）。
const COLOR_STATUS_CHANGED: egui::Color32 = egui::Color32::from_rgb(224, 176, 0);

/// 新增/未跟踪状态标记的颜色（绿色）。
const COLOR_STATUS_ADDED: egui::Color32 = egui::Color32::from_rgb(64, 200, 112);

/// 主色（青绿，取自程序图标）：选中态、按钮激活态、悬停描边。
const COLOR_ACCENT: egui::Color32 = egui::Color32::from_rgb(38, 166, 154);

/// 明亮主题下的主色（加深一档，保证浅底可读）。
const COLOR_ACCENT_DARK: egui::Color32 = egui::Color32::from_rgb(24, 128, 118);

/// 按主题生成定制配色：深色为蓝灰底 + 青绿主色，明亮为白底 + 淡蓝斑马纹。
/// 文件浏览器斑马纹（`TableBuilder::striped`）取 `faint_bg_color`，故该
/// 颜色按需求设为淡蓝色系；圆角与间距一并在此统一。
fn custom_visuals(theme: Theme) -> egui::Visuals {
    let mut v = match theme {
        Theme::Dark => egui::Visuals::dark(),
        Theme::Light => egui::Visuals::light(),
    };
    match theme {
        Theme::Dark => {
            v.panel_fill = egui::Color32::from_rgb(31, 37, 44);
            v.window_fill = egui::Color32::from_rgb(37, 44, 52);
            v.extreme_bg_color = egui::Color32::from_rgb(24, 29, 35);
            // 斑马纹 / 弱底色：偏蓝的深灰，与蓝色系界面呼应。
            v.faint_bg_color = egui::Color32::from_rgb(40, 53, 66);
            v.window_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(58, 70, 82));
            v.selection.bg_fill = COLOR_ACCENT;
            v.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
            v.hyperlink_color = egui::Color32::from_rgb(77, 208, 196);
            v.warn_fg_color = egui::Color32::from_rgb(255, 179, 0);
            v.error_fg_color = egui::Color32::from_rgb(255, 107, 107);
            v.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(34, 41, 49);
            v.widgets.noninteractive.bg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(52, 63, 75));
            v.widgets.noninteractive.fg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(205, 214, 223));
            v.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(46, 55, 65);
            v.widgets.inactive.bg_fill = egui::Color32::from_rgb(46, 55, 65);
            v.widgets.inactive.bg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(61, 74, 87));
            v.widgets.inactive.fg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(216, 224, 232));
            v.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(56, 69, 82);
            v.widgets.hovered.bg_fill = egui::Color32::from_rgb(56, 69, 82);
            v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, COLOR_ACCENT);
            v.widgets.hovered.fg_stroke =
                egui::Stroke::new(1.5_f32, egui::Color32::WHITE);
            v.widgets.active.weak_bg_fill = egui::Color32::from_rgb(30, 118, 110);
            v.widgets.active.bg_fill = COLOR_ACCENT;
            v.widgets.active.bg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(77, 208, 196));
            v.widgets.active.fg_stroke =
                egui::Stroke::new(1.5_f32, egui::Color32::WHITE);
        }
        Theme::Light => {
            v.panel_fill = egui::Color32::from_rgb(252, 253, 255);
            v.window_fill = egui::Color32::WHITE;
            v.extreme_bg_color = egui::Color32::from_rgb(242, 247, 252);
            // 淡蓝色斑马纹（用户指定：文件窗口间隔底色）。
            v.faint_bg_color = egui::Color32::from_rgb(228, 240, 252);
            v.window_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(206, 219, 232));
            v.selection.bg_fill = COLOR_ACCENT_DARK;
            v.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
            v.hyperlink_color = egui::Color32::from_rgb(20, 122, 112);
            v.warn_fg_color = egui::Color32::from_rgb(170, 112, 0);
            v.error_fg_color = egui::Color32::from_rgb(198, 48, 48);
            v.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(246, 250, 254);
            v.widgets.noninteractive.bg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(213, 224, 235));
            v.widgets.noninteractive.fg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(41, 54, 66));
            v.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(242, 247, 252);
            v.widgets.inactive.bg_fill = egui::Color32::from_rgb(242, 247, 252);
            v.widgets.inactive.bg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(202, 215, 228));
            v.widgets.inactive.fg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(41, 54, 66));
            v.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(222, 237, 252);
            v.widgets.hovered.bg_fill = egui::Color32::from_rgb(222, 237, 252);
            v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, COLOR_ACCENT_DARK);
            v.widgets.hovered.fg_stroke =
                egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(14, 30, 38));
            v.widgets.active.weak_bg_fill = egui::Color32::from_rgb(202, 233, 228);
            v.widgets.active.bg_fill = COLOR_ACCENT_DARK;
            v.widgets.active.bg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(16, 105, 96));
            v.widgets.active.fg_stroke =
                egui::Stroke::new(1.5_f32, egui::Color32::WHITE);
        }
    }
    let rounding = egui::Rounding::same(6.0);
    v.widgets.noninteractive.rounding = rounding;
    v.widgets.inactive.rounding = rounding;
    v.widgets.hovered.rounding = rounding;
    v.widgets.active.rounding = rounding;
    v.widgets.open.rounding = rounding;
    v.window_rounding = egui::Rounding::same(9.0);
    v.menu_rounding = egui::Rounding::same(7.0);
    v
}


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
#[derive(Default)]
struct StatusSnapshot {
    status: Option<StatusSection>,
    branches: Option<StatusSection>,
    tags: Option<StatusSection>,
}

/// Which third of the read-only batch a background result belongs to.
#[derive(Clone, Copy)]
enum StatusPart {
    Status,
    Branches,
    Tags,
}

/// Cached read-only snapshot of one project, stored in
/// `GitPmApp::status_cache` (key = `Project.id`). Lets a project switch render
/// instantly instead of waiting for three git subprocesses.
struct CachedStatus {
    /// `git status --porcelain=v1 -b` output text.
    porcelain: String,
    /// Branch names from `git branch --format=%(refname:short)`.
    branches: Vec<String>,
    /// Tag names from `git tag --list`.
    tags: Vec<String>,
    /// When this entry was last written.
    loaded_at: std::time::Instant,
}

impl CachedStatus {
    fn empty() -> Self {
        Self {
            porcelain: String::new(),
            branches: Vec::new(),
            tags: Vec::new(),
            loaded_at: std::time::Instant::now(),
        }
    }
}

/// A write operation (commit/pull/push/branch/tag) currently running on a
/// worker thread. While `busy` is `Some`, the operation panel's buttons are
/// disabled and an animated progress bar is shown.
#[derive(Clone)]
struct BusyOp {
    /// Project the operation belongs to.
    project_id: i64,
    /// Human-readable progress text shown above the progress bar.
    label: String,
}

/// Which git call a worker-thread result belongs to. Kinds that need a
/// specific output-log label carry it pre-built (labels never contain
/// credentials).
enum ResultKind {
    /// `git status --porcelain=v1 -b` (part of a status refresh batch).
    Status,
    /// `git branch --format=%(refname:short)` (part of a status refresh batch).
    BranchList,
    /// `git tag --list` (part of a status refresh batch).
    TagList,
    /// `git rev-parse --abbrev-ref HEAD` (pull/push prerequisite).
    Head,
    /// `git remote get-url <remote>` (credential injection prerequisite; only
    /// sent on io failure, mirroring the old silent-degrade behaviour).
    /// Payload = the remote name, for the output-log label.
    RemoteUrl(String),
    /// `git add -A` + `git commit -m <msg>`; payload = output-log label.
    Commit(String),
    /// `git pull [...]`; payload = output-log label.
    Pull(String),
    /// `git push [...]` (one-click push to `origin`); payload = output-log label.
    Push(String),
    /// `git push [-u] <remote> <branch>` from the push dialog; payload =
    /// output-log label.
    PushTo(String),
    /// `git branch <name>`; payload = output-log label.
    BranchCreate(String),
    /// `git checkout <name>`; payload = output-log label.
    BranchCheckout(String),
    /// `git tag [...] <name>`; payload = output-log label.
    Tag(String),
}

/// One finished git call from a worker thread, routed back to the UI thread
/// over a `std::sync::mpsc` channel and drained at the top of each frame.
struct GitResult {
    /// Project the call belongs to (cache key + backfill check).
    project_id: i64,
    /// Which call this was.
    kind: ResultKind,
    /// The git output — already redacted inside the worker when credentials
    /// were involved — or the io error when git could not run at all.
    output: std::io::Result<git::GitOutput>,
}

/// Whether a background result may update the on-screen status area: only
/// when it belongs to the project that is currently selected. Regardless of
/// the return value the caller still updates the per-project cache, so a
/// rapid project switch never mixes data between projects.
fn should_apply(result_project_id: i64, selected_id: Option<i64>) -> bool {
    selected_id == Some(result_project_id)
}

/// Scrub credential material from a git output inside the worker thread,
/// before the result ever reaches the UI thread or the output log.
fn redact_output(mut out: git::GitOutput, pairs: &[(String, String)]) -> git::GitOutput {
    out.stdout = redact_text(out.stdout, pairs);
    out.stderr = redact_text(out.stderr, pairs);
    out
}

/// Worker helper: run a plain (no injected credentials) `git pull` and send
/// the result back.
fn send_plain_pull(tx: &mpsc::Sender<GitResult>, project_id: i64, path: &Path) {
    let output = git::pull(path);
    let _ = tx.send(GitResult {
        project_id,
        kind: ResultKind::Pull("git pull".to_string()),
        output,
    });
}

/// Worker helper: run a plain `git push origin <branch>` and send the result
/// back.
fn send_plain_push(tx: &mpsc::Sender<GitResult>, project_id: i64, path: &Path, branch: &str) {
    let label = format!("git push {DEFAULT_PUSH_REMOTE} {branch}");
    let output = git::push(path, DEFAULT_PUSH_REMOTE, branch);
    let _ = tx.send(GitResult { project_id, kind: ResultKind::Push(label), output });
}

/// Worker helper: run a plain `git push [-u] <remote> <branch>` (push dialog,
/// no injected credentials) and send the result back.
fn send_plain_push_to(
    tx: &mpsc::Sender<GitResult>,
    project_id: i64,
    path: &Path,
    remote: &str,
    branch: &str,
    set_upstream: bool,
) {
    let flag = if set_upstream { "-u " } else { "" };
    let label = format!("git push {flag}{remote} {branch}");
    let output = if set_upstream {
        git::run(path, &["push", "-u", remote, branch])
    } else {
        git::push(path, remote, branch)
    };
    let _ = tx.send(GitResult { project_id, kind: ResultKind::PushTo(label), output });
}

/// 判定一次推送是否需要「上游回写」：只有**注入凭据**且带 `-u` 的推送
/// 才会让 git 在推送成功后把含明文凭据的注入 URL 持久化写入目标仓库
/// `.git/config` 的 `branch.<branch>.remote`（凭据落盘）；其余组合
/// （不注入，或不带 `-u`）没有凭据落盘问题，无需回写。
fn push_needs_upstream_restore(injected: bool, set_upstream: bool) -> bool {
    injected && set_upstream
}

/// 注入凭据 + `-u` 推送成功后的上游回写：git 已把含明文凭据的注入 URL
/// 写入 `.git/config` 的 `branch.<branch>.remote`，这里立即用真实
/// remote 名覆盖（`branch.<branch>.merge` 由 git 写入时即为正确的
/// `refs/heads/<branch>`，无需改动）。
///
/// - 仅在推送成功时回写：推送失败时 git 不写配置，无可回写；
/// - 回写失败**不改判**推送失败：在输出 stderr 追加
///   `warn-upstream-restore-failed` 文案（按 `lang` 取）与回写失败详情；
/// - 回写命令的输出并入后，合成结果整体再过一次 [`redact_output`]
///   （双保险），任何凭据残留都不会进入输出区/日志。
fn restore_upstream_remote(
    output: std::io::Result<git::GitOutput>,
    path: &Path,
    remote: &str,
    branch: &str,
    lang: Lang,
    auth: &InjectedAuth,
) -> std::io::Result<git::GitOutput> {
    // git 本就没运行（Err）：没有推送也没有配置写入，原样返回错误。
    let push_out = output?;
    if !push_out.ok() {
        return Ok(push_out); // 推送失败：git 未写配置，无需回写
    }
    let pairs = auth.redact_pairs();
    let key = format!("branch.{branch}.remote");
    let detail = match git::run(path, &["config", &key, remote]) {
        Ok(cfg) if cfg.ok() => return Ok(push_out),
        Ok(cfg) => git_output_text(lang, &cfg),
        Err(e) => tf(lang, "err-git-run", &[("err", &e.to_string())]),
    };
    let mut stderr = push_out.stderr;
    if !stderr.is_empty() {
        stderr.push('\n');
    }
    stderr.push_str(i18n::t(lang, "warn-upstream-restore-failed"));
    stderr.push('\n');
    stderr.push_str(&detail);
    // 双保险：合成后的完整输出整体再脱敏一次。
    Ok(redact_output(
        git::GitOutput { code: push_out.code, stdout: push_out.stdout, stderr },
        &pairs,
    ))
}

/// Split a successful read-only output into trimmed non-empty lines.
fn output_lines(out: &git::GitOutput) -> Vec<String> {
    out.stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Build a status-area snapshot from a cache entry (cached data only holds
/// successful queries, so all sections render as ok).
fn snapshot_from_cache(cache: &CachedStatus) -> StatusSnapshot {
    StatusSnapshot {
        status: Some(StatusSection { ok: true, text: cache.porcelain.clone() }),
        branches: Some(StatusSection { ok: true, text: cache.branches.join("\n") }),
        tags: Some(StatusSection { ok: true, text: cache.tags.join("\n") }),
    }
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
fn git_output_text(lang: Lang, out: &git::GitOutput) -> String {
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
        text.push_str(i18n::t(lang, "no-output"));
    }
    text
}

/// Convert a finished git call into a status-area section.
fn status_section(lang: Lang, out: &git::GitOutput) -> StatusSection {
    StatusSection {
        ok: out.ok(),
        text: git_output_text(lang, out),
    }
}

/// 把 `git status --porcelain=v1 -b` 原始输出格式化为人类可读文字：
/// `## ` 分支行译为「分支：…」，每个变更条目译为「[状态标签] 路径」，
/// 条目零变更时显示「工作区干净」。无法解析的行原样保留。
fn format_worktree_status(lang: Lang, porcelain: &str) -> String {
    let mut lines_out: Vec<String> = Vec::new();
    let mut has_entries = false;
    for line in porcelain.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(branch) = line.strip_prefix("## ") {
            lines_out.push(format!(
                "{}{}",
                i18n::t(lang, "worktree-branch"),
                branch.trim()
            ));
            continue;
        }
        let bytes = line.as_bytes();
        if bytes.len() < 4 {
            lines_out.push(line.to_string());
            continue;
        }
        let Some(path) = porcelain_entry_path(&line[3..]) else {
            lines_out.push(line.to_string());
            continue;
        };
        let status = status_from_xy(bytes[0], bytes[1]);
        lines_out.push(format!("[{}] {}", status_legend_text(lang, status), path));
        has_entries = true;
    }
    if !has_entries {
        lines_out.push(i18n::t(lang, "worktree-clean").to_string());
    }
    lines_out.join("\n")
}

/// Render one status-area section: plain text on success, red on failure.
fn show_section(
    ui: &mut egui::Ui,
    lang: Lang,
    title: &str,
    section: Option<&StatusSection>,
) {
    ui.add_space(4.0);
    // 小节标题用主色（hyperlink_color 在两个主题下都是青绿系），与正文区分。
    ui.label(
        egui::RichText::new(title)
            .strong()
            .color(ui.visuals().hyperlink_color),
    );
    match section {
        Some(s) if s.ok => {
            ui.monospace(&s.text);
        }
        Some(s) => {
            ui.colored_label(egui::Color32::RED, &s.text);
        }
        None => {
            ui.label(i18n::t(lang, "not-loaded"));
        }
    }
}

// ------------------------------------------------------ porcelain parsing
//
// `git status --porcelain=v1 -b` 解析纯函数（只依赖 std，不依赖 egui）：
// 浏览器文件状态标记的数据源是 `status_cache` 里的 porcelain 缓存文本。

/// 浏览器条目的工作区文件状态（由 porcelain 的 XY 状态对推导）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileStatus {
    /// 未合并冲突（UU/AA/DD/AU/UA/DU/UD）。
    Conflict,
    /// 已删除（XY 中含 D）。
    Deleted,
    /// 已修改（XY 中含 M 或 T）。
    Modified,
    /// 重命名/复制（XY 中含 R 或 C）。
    Renamed,
    /// 新增或未跟踪（A / ??）。
    Added,
}

impl FileStatus {
    /// 同一路径多个状态的取舍优先级：冲突 > 删除 > 修改 > 重命名 > 新增。
    fn rank(self) -> u8 {
        match self {
            FileStatus::Conflict => 5,
            FileStatus::Deleted => 4,
            FileStatus::Modified => 3,
            FileStatus::Renamed => 2,
            FileStatus::Added => 1,
        }
    }

    /// 名称前显示的标记字符。
    fn marker(self) -> &'static str {
        match self {
            FileStatus::Conflict => "!",
            FileStatus::Deleted => "−",
            FileStatus::Modified => "M",
            FileStatus::Renamed => "R",
            FileStatus::Added => "+",
        }
    }
}

/// 一对 porcelain XY 状态字符 → 文件状态。
///
/// 冲突对（git status 文档）优先；`??` 未跟踪视为新增；其余按
/// 冲突 > 删除 > 修改 > 重命名 > 新增 在 XY 两个字母之间取舍。
fn status_from_xy(x: u8, y: u8) -> FileStatus {
    if matches!(
        [x, y],
        [b'D', b'D']
            | [b'A', b'U']
            | [b'U', b'D']
            | [b'U', b'A']
            | [b'D', b'U']
            | [b'A', b'A']
            | [b'U', b'U']
    ) {
        return FileStatus::Conflict;
    }
    if x == b'?' && y == b'?' {
        return FileStatus::Added; // 未跟踪
    }
    let letters = [x, y];
    if letters.contains(&b'D') {
        return FileStatus::Deleted;
    }
    if letters.contains(&b'M') || letters.contains(&b'T') {
        return FileStatus::Modified;
    }
    if letters.contains(&b'R') || letters.contains(&b'C') {
        return FileStatus::Renamed;
    }
    if letters.contains(&b'A') {
        return FileStatus::Added;
    }
    FileStatus::Modified // 未知组合退回通用「已修改」标记
}

/// 扫描一个 C 风格引号字符串（`bytes[i]` 必须是开引号 `"`）。
///
/// 返回（解码后的字符串，闭引号之后的下标）。转义支持 git 使用的集合
/// `\a \b \f \n \r \t \v \\ \"` 与八进制 `\NNN`（原始 UTF-8 字节）。
/// 格式非法（无闭引号、未知转义、八进制超界）返回 None，调用方回退为
/// 原文，绝不 panic。
fn scan_quoted(bytes: &[u8], mut i: usize) -> Option<(String, usize)> {
    i += 1; // 跳过开引号
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return String::from_utf8(out).ok().map(|s| (s, i + 1)),
            b'\\' => {
                i += 1;
                match bytes.get(i)? {
                    b'a' => out.push(0x07),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0C),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'v' => out.push(0x0B),
                    b'\\' => out.push(b'\\'),
                    b'"' => out.push(b'"'),
                    b'0'..=b'7' => {
                        // 八进制转义：最多 3 位（当前这位已读入）。
                        let mut value = u32::from(bytes[i] - b'0');
                        let mut count = 1;
                        while count < 3 {
                            match bytes.get(i + 1) {
                                Some(d @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                            count += 1;
                        }
                        if value > u32::from(u8::MAX) {
                            return None;
                        }
                        out.push(value as u8);
                    }
                    _ => return None,
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    None // 没有闭引号
}

/// 引号包裹的路径 → 去引号并解码转义；引号不成对或格式非法时回退为
/// 去引号的原文（宁可匹配不上，也不 panic）。
fn unquoted_or_raw(path: &str) -> Option<String> {
    if path.as_bytes().first() == Some(&b'"') {
        return match scan_quoted(path.as_bytes(), 0)
            .filter(|(_, end)| path[*end..].trim().is_empty())
        {
            Some((decoded, _)) => Some(decoded),
            None => Some(path.trim_matches('"').to_string()),
        };
    }
    Some(path.to_string())
}

/// porcelain 路径字段 → 实际路径；rename 行取 `->` 右侧的新路径。
///
/// 兼容实测 git（Windows，`core.quotePath=true`）的两种形态：
/// - rename：任一侧含空格/非 ASCII 时该侧独立加引号，
///   `RM "src dir/a.txt" -> "src dir/b moved.txt"`；
/// - 普通条目：含空格或非 ASCII 时整体引号并转义，` M "with space.txt"`、
///   ` M "\346\226\207.txt"`。
fn porcelain_entry_path(field: &str) -> Option<String> {
    if field.as_bytes().first() == Some(&b'"') {
        // 引号开头：扫描第一个引号串。其后为空 → 路径本身；
        // 其后是 ` -> …` → rename，取右侧（实测 git 对 rename 逐侧加引号）。
        let (decoded, end) = scan_quoted(field.as_bytes(), 0)?;
        let rest = field[end..].trim_start();
        if rest.is_empty() {
            return Some(decoded);
        }
        let new_part = rest.strip_prefix("-> ")?;
        unquoted_or_raw(new_part.trim_start())
    } else {
        match field.rsplit_once(" -> ") {
            Some((_old, new)) => unquoted_or_raw(new),
            None => Some(field.to_string()),
        }
    }
}

/// 解析 `git status --porcelain=v1 [-b]` 输出 → 仓库相对路径 → 状态。
///
/// - 跳过 `## <branch>` 头行、空行与已忽略条目（`!!`）；
/// - `R  old -> new` 取新路径（见 [`porcelain_entry_path`]）；
/// - 引号路径去引号并解码转义（含 UTF-8 八进制转义）；
/// - 未跟踪目录 `?? dir/` 去掉尾部 `/`，使目录条目可精确匹配；
/// - 同一路径多行按 冲突 > 删除 > 修改 > 重命名 > 新增 取最高优先级。
fn parse_status_map(porcelain: &str) -> HashMap<String, FileStatus> {
    let mut map: HashMap<String, FileStatus> = HashMap::new();
    for line in porcelain.lines() {
        if line.starts_with("## ")
            || line.starts_with("!!")
            || line.trim().is_empty()
        {
            continue;
        }
        let bytes = line.as_bytes();
        if bytes.len() < 4 || bytes[2] != b' ' {
            continue; // 不是 XY␣path 形态，跳过（不 panic）
        }
        let status = status_from_xy(bytes[0], bytes[1]);
        let Some(path) = porcelain_entry_path(&line[3..]) else {
            continue; // 路径字段格式非法，跳过该行
        };
        let path = path.trim_end_matches('/');
        if path.is_empty() {
            continue;
        }
        let entry = map.entry(path.to_string()).or_insert(status);
        if status.rank() > entry.rank() {
            *entry = status;
        }
    }
    map
}

/// 浏览器条目的状态：文件按相对路径精确匹配；目录做前缀匹配，取其下
/// 所有变更条目里优先级最高的状态（porcelain 路径恒以 `/` 分隔）。
fn status_for_entry(
    map: &HashMap<String, FileStatus>,
    rel_path: &str,
    is_dir: bool,
) -> Option<FileStatus> {
    if let Some(status) = map.get(rel_path) {
        return Some(*status);
    }
    if !is_dir {
        return None;
    }
    let prefix = format!("{rel_path}/");
    map.iter()
        .filter(|(path, _)| path.starts_with(&prefix))
        .map(|(_, status)| *status)
        .max_by_key(|status| status.rank())
}

/// 从 `git status --porcelain=v1 -b` 输出解析当前分支名（`## <branch>` 行）。
///
/// 兼容 `master...origin/master [ahead 1]` 与未出生分支的
/// `No commits yet on master`（老 git：`Initial commit on master`）；
/// 分离 HEAD（`HEAD (no branch)`）或无头行返回 None。
fn current_branch_from_porcelain(porcelain: &str) -> Option<String> {
    let head = porcelain.lines().find_map(|l| l.strip_prefix("## "))?.trim();
    if head.starts_with("HEAD (no branch)") {
        return None;
    }
    if let Some(rest) = head.strip_prefix("No commits yet on ") {
        return Some(rest.to_string());
    }
    if let Some(rest) = head.strip_prefix("Initial commit on ") {
        return Some(rest.to_string());
    }
    let branch = head.split("...").next().unwrap_or_default();
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
}

/// 浏览器条目名前的状态标记：着色字符；冲突为红底加粗白字。
fn status_marker_rich(status: FileStatus) -> egui::RichText {
    let (color, conflict) = match status {
        FileStatus::Conflict => (egui::Color32::WHITE, true),
        FileStatus::Deleted => (egui::Color32::RED, false),
        FileStatus::Modified | FileStatus::Renamed => (COLOR_STATUS_CHANGED, false),
        FileStatus::Added => (COLOR_STATUS_ADDED, false),
    };
    let text = egui::RichText::new(status.marker()).color(color);
    if conflict {
        text.strong().background_color(egui::Color32::RED)
    } else {
        text
    }
}

/// 状态标记的图例说明（浏览器列表下方显示一次）。
fn status_legend_text(lang: Lang, status: FileStatus) -> &'static str {
    match status {
        FileStatus::Conflict => i18n::t(lang, "legend-conflict"),
        FileStatus::Deleted => i18n::t(lang, "legend-deleted"),
        FileStatus::Modified => i18n::t(lang, "legend-modified"),
        FileStatus::Renamed => i18n::t(lang, "legend-renamed"),
        FileStatus::Added => i18n::t(lang, "legend-added"),
    }
}

// ------------------------------------------------- project tree grouping
//
// 左侧项目树的纯分组逻辑（只依赖 std，不依赖 egui）：项目路径逐段展开
// 为目录树（盘符 / 每级目录名各成一层），项目挂在直接父目录节点下。
// 每层缩进 2 个字符宽由 UI 层负责。单元测试见文件末尾 tests 模块。

/// 项目树的一个目录节点（多层结构，递归嵌套）。
struct DirNode {
    /// 该层的展示名：盘符（如 `E:`）、POSIX 根（`/`）或目录名；
    /// 无法推导父目录的项目归入 [`GROUP_UNGROUPED_KEY`] 节点（展示时
    /// 由 UI 层按当前语言翻译）。
    label: String,
    /// 规范化完整路径 key（折叠态记忆与测试断言用；比较时大小写
    /// 归一化，Windows 模式分隔符统一为 `\`）。
    key: String,
    /// 子目录节点（按展示名排序）。
    dirs: Vec<DirNode>,
    /// 直接挂在该目录下的项目下标，保持输入顺序。
    projects: Vec<usize>,
}

/// 项目路径 → 父目录的路径组件（原样拼写；空路径 / 纯根路径 /
/// 单段相对路径等推导不出父目录时返回 None）。
fn parent_dir_components(path: &str) -> Option<Vec<String>> {
    let parent = Path::new(path).parent()?;
    let mut comps: Vec<String> = Vec::new();
    let mut saw_prefix = false;
    for comp in parent.components() {
        match comp {
            std::path::Component::Prefix(prefix) => {
                saw_prefix = true;
                comps.push(prefix.as_os_str().to_string_lossy().into_owned());
            }
            std::path::Component::RootDir => {
                // Windows 的 `E:\` 根分隔已由盘符前缀表达，不再单列一层；
                // POSIX 的 `/` 作为顶层节点保留。
                if !saw_prefix && comps.is_empty() {
                    comps.push("/".to_string());
                }
            }
            std::path::Component::Normal(seg) => {
                comps.push(seg.to_string_lossy().into_owned());
            }
            std::path::Component::CurDir | std::path::Component::ParentDir => {}
        }
    }
    if comps.is_empty() {
        None
    } else {
        Some(comps)
    }
}

/// 按组件链把项目插入目录树；`parent_key` 为父节点 key（根为空串）。
/// 同名（`case_insensitive` 时忽略大小写）目录复用同一节点，节点 key
/// 由组件链拼接、大小写归一化。
fn insert_into_tree(
    nodes: &mut Vec<DirNode>,
    comps: &[String],
    idx: usize,
    case_insensitive: bool,
    parent_key: &str,
) {
    let head = &comps[0];
    let norm = if case_insensitive {
        head.to_lowercase()
    } else {
        head.clone()
    };
    let pos = nodes.iter().position(|node| {
        if case_insensitive {
            node.label.to_lowercase() == norm
        } else {
            node.label == *head
        }
    });
    let pos = match pos {
        Some(pos) => pos,
        None => {
            let key = if parent_key.is_empty() {
                norm.clone()
            } else if case_insensitive {
                format!("{parent_key}\\{norm}")
            } else {
                format!("{parent_key}/{norm}")
            };
            nodes.push(DirNode {
                label: head.clone(),
                key,
                dirs: Vec::new(),
                projects: Vec::new(),
            });
            nodes.len() - 1
        }
    };
    let node = &mut nodes[pos];
    if comps.len() == 1 {
        node.projects.push(idx);
    } else {
        let key = node.key.clone();
        insert_into_tree(&mut node.dirs, &comps[1..], idx, case_insensitive, &key);
    }
}

/// 递归按展示名排序目录节点（`case_insensitive` 时忽略大小写）。
fn sort_tree(nodes: &mut [DirNode], case_insensitive: bool) {
    nodes.sort_by(|a, b| {
        if case_insensitive {
            a.label.to_lowercase().cmp(&b.label.to_lowercase())
        } else {
            a.label.cmp(&b.label)
        }
    });
    for node in nodes.iter_mut() {
        sort_tree(&mut node.dirs, case_insensitive);
    }
}

/// 项目树构建：每个项目路径逐段展开为目录层级，项目挂在直接父目录下；
/// 无法推导父目录的项目归入末尾的未分组节点（label 为
/// [`GROUP_UNGROUPED_KEY`] 稳定 key）；每个输入下标恰好出现一次
/// （同目录内保持输入顺序）。
fn build_project_tree(paths: &[String], case_insensitive: bool) -> Vec<DirNode> {
    let mut roots: Vec<DirNode> = Vec::new();
    let mut ungrouped: Vec<usize> = Vec::new();
    for (idx, path) in paths.iter().enumerate() {
        match parent_dir_components(path) {
            Some(comps) => insert_into_tree(&mut roots, &comps, idx, case_insensitive, ""),
            None => ungrouped.push(idx),
        }
    }
    sort_tree(&mut roots, case_insensitive);
    if !ungrouped.is_empty() {
        roots.push(DirNode {
            label: GROUP_UNGROUPED_KEY.to_string(),
            key: String::new(),
            dirs: Vec::new(),
            projects: ungrouped,
        });
    }
    roots
}

/// 目录行折叠箭头：展开时向下三角，折叠时向右三角（画在箭头槽内，
/// 使目录名与子层对齐于缩进线）。
fn paint_tree_arrow(ui: &egui::Ui, rect: egui::Rect, open: bool) {
    let center = rect.left_center() + egui::vec2(rect.width() * 0.35, 0.0);
    let r = 4.5;
    let points = if open {
        vec![
            center + egui::vec2(-r, -r * 0.6),
            center + egui::vec2(r, -r * 0.6),
            center + egui::vec2(0.0, r * 0.7),
        ]
    } else {
        vec![
            center + egui::vec2(-r * 0.6, -r),
            center + egui::vec2(-r * 0.6, r),
            center + egui::vec2(r * 0.7, 0.0),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        ui.visuals().weak_text_color(),
        egui::Stroke::NONE,
    ));
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
    /// UI language, restored from the meta table at startup (Zh on failure).
    lang: Lang,
    error: Option<String>,

    setup_pw1: String,
    setup_pw2: String,
    unlock_pw: String,

    // Session keys from the master password, used to read/write credentials.
    keys: Option<VaultKeys>,
    projects: Vec<Project>,
    /// 项目树中处于折叠态的目录节点 key（缺省全部展开，仅会话内记忆）。
    collapsed_tree_dirs: HashSet<String>,
    selected: Option<i64>,
    confirm_remove: bool,

    // Right panel: per-project git operations.
    /// Latest status/branch/tag snapshot for the selected project.
    status: Option<StatusSnapshot>,
    /// Project id the snapshot belongs to; refreshed on selection change.
    status_loaded_for: Option<i64>,
    /// Per-project read-only snapshot cache, keyed by `Project.id`; a project
    /// switch renders from it instantly instead of re-running three git calls.
    status_cache: HashMap<i64, CachedStatus>,
    /// Projects whose status refresh batch is currently running on a worker
    /// thread (duplicate-spawn guard).
    status_inflight: HashSet<i64>,
    /// Write operation currently running on a worker thread, if any.
    busy: Option<BusyOp>,
    /// Cloned into every spawned worker thread.
    result_tx: mpsc::Sender<GitResult>,
    /// Worker results drained at the top of each frame.
    result_rx: mpsc::Receiver<GitResult>,
    commit_msg: String,
    new_branch: String,
    tag_name: String,
    tag_msg: String,
    /// Bottom read-only log, one entry per git invocation.
    log: Vec<LogEntry>,

    // Push dialog（操作面板「推送…」）。
    show_push_dialog: bool,
    /// Configured remotes, loaded once when the dialog opens.
    push_remotes: Vec<String>,
    /// Index into `push_remotes` chosen in the dropdown.
    push_remote_sel: usize,
    /// Branch name input; defaults to the current branch from the status cache.
    push_branch: String,
    /// `-u`：推送并设置上游。
    push_set_upstream: bool,
    /// Validation error shown inside the push dialog.
    push_error: Option<String>,
    /// `remote_list` failure shown inside the push dialog (push disabled).
    push_remotes_error: Option<String>,

    // Commit history window（操作面板「历史…」，只读）。
    show_history_window: bool,
    /// Commits loaded for the current window session; cleared on close.
    history_commits: Vec<git::CommitInfo>,
    /// `log_recent` failure shown inside the window; older commits stay.
    history_error: Option<String>,
    /// True when the last page came back short: no more commits to load.
    history_exhausted: bool,

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

    // Master-password change dialog (菜单「设置 → 修改主密码」).
    show_pw_change: bool,
    pw_old: String,
    pw_new: String,
    pw_new2: String,
    /// Error shown inside the master-password change dialog.
    pw_error: Option<String>,

    // Password generator dialogs (one from the credential dialog, one
    // standalone from the top-bar「工具」menu), sharing options and result.
    show_gen_dialog: bool,
    show_tool_gen_window: bool,
    gen_opts: GenOptions,
    /// `false` = 明文显示生成结果（默认，可选中复制）；`true` = 用 `•` 遮蔽显示。
    gen_masked: bool,
    /// Last generated password, masked, until copied or filled into the
    /// credential dialog's password field. Cleared whenever either generator
    /// window closes.
    gen_result: String,
    gen_error: Option<String>,
}

impl GitPmApp {
    pub fn new() -> Self {
        let (result_tx, result_rx) = mpsc::channel();
        let mut app = Self {
            conn: None,
            open_error: None,
            phase: Phase::Setup,
            theme: Theme::Dark,
            lang: Lang::Zh,
            error: None,
            setup_pw1: String::new(),
            setup_pw2: String::new(),
            unlock_pw: String::new(),
            keys: None,
            projects: Vec::new(),
            collapsed_tree_dirs: HashSet::new(),
            selected: None,
            confirm_remove: false,
            status: None,
            status_loaded_for: None,
            status_cache: HashMap::new(),
            status_inflight: HashSet::new(),
            busy: None,
            result_tx,
            result_rx,
            commit_msg: String::new(),
            new_branch: String::new(),
            tag_name: String::new(),
            tag_msg: String::new(),
            log: Vec::new(),
            show_push_dialog: false,
            push_remotes: Vec::new(),
            push_remote_sel: 0,
            push_branch: String::new(),
            push_set_upstream: false,
            push_error: None,
            push_remotes_error: None,
            show_history_window: false,
            history_commits: Vec::new(),
            history_error: None,
            history_exhausted: false,
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
            show_pw_change: false,
            pw_old: String::new(),
            pw_new: String::new(),
            pw_new2: String::new(),
            pw_error: None,
            show_gen_dialog: false,
            show_tool_gen_window: false,
            gen_opts: GenOptions::default(),
            gen_masked: false,
            gen_result: String::new(),
            gen_error: None,
        };

        match db::init_db(Path::new(DB_PATH)) {
            Ok(conn) => {
                // 语言偏好先读：后续启动错误文案按所选语言渲染（读失败
                // 降级中文，不 panic）。
                match db::get_meta(&conn, i18n::LANG_KEY) {
                    Ok(value) => app.lang = i18n::from_meta(value.as_deref()),
                    Err(e) => {
                        // Cosmetic only; keep any more important startup error.
                        if app.error.is_none() {
                            app.error =
                                Some(tf(app.lang, "err-read-lang", &[("err", &e.to_string())]));
                        }
                    }
                }
                match db::get_meta(&conn, VERIFIER_KEY) {
                    Ok(Some(_)) => app.phase = Phase::Unlock,
                    Ok(None) => app.phase = Phase::Setup,
                    Err(e) => {
                        // We cannot tell whether the vault is initialized, so
                        // stay on Setup: `setup_master` refuses to re-initialize
                        // an existing vault and the error is shown there.
                        app.phase = Phase::Setup;
                        app.error = Some(tf(
                            app.lang,
                            "err-read-vault-state",
                            &[("err", &e.to_string())],
                        ));
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
                            app.error = Some(tf(
                                app.lang,
                                "err-read-theme",
                                &[("err", &e.to_string())],
                            ));
                        }
                    }
                }
                app.conn = Some(conn);
            }
            Err(e) => {
                let msg = tf(
                    app.lang,
                    "err-open-db",
                    &[("path", DB_PATH), ("err", &e.to_string())],
                );
                app.open_error = Some(msg.clone());
                app.error = Some(msg);
            }
        }
        app
    }

    // ---------------------------------------------------------------- setup

    fn show_setup(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(64.0);
                ui.heading(i18n::t(lang, "setup-title"));
                ui.add_space(8.0);
                ui.label(i18n::t(lang, "setup-intro"));
                ui.label(i18n::t(lang, "setup-warning"));
                ui.add_space(24.0);
                ui.add_sized(
                    [FIELD_WIDTH, 24.0],
                    egui::TextEdit::singleline(&mut self.setup_pw1)
                        .password(true)
                        .hint_text(i18n::t(lang, "hint-master-password")),
                );
                ui.add_space(8.0);
                let confirm = ui.add_sized(
                    [FIELD_WIDTH, 24.0],
                    egui::TextEdit::singleline(&mut self.setup_pw2)
                        .password(true)
                        .hint_text(i18n::t(lang, "hint-repeat-master-password")),
                );
                let pressed_enter =
                    confirm.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(12.0);
                let clicked = ui
                    .add_sized(
                        [FIELD_WIDTH, 32.0],
                        egui::Button::new(i18n::t(lang, "btn-set-master-password")),
                    )
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
            self.error = Some(i18n::t(self.lang, "err-setup-empty").into());
            return;
        }
        if self.setup_pw1 != self.setup_pw2 {
            self.error = Some(i18n::t(self.lang, "err-setup-mismatch").into());
            self.setup_pw2.clear();
            return;
        }
        let Some(conn) = self.conn.as_ref() else {
            self.error = Some(
                self.open_error
                    .clone()
                    .unwrap_or_else(|| i18n::t(self.lang, "err-db-not-open").into()),
            );
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
                self.error =
                    Some(tf(self.lang, "err-read-vault-state", &[("err", &e.to_string())]));
                return;
            }
        }

        if let Err(e) = crypto::setup_master(conn, &self.setup_pw1) {
            self.error = Some(tf(self.lang, "err-setup-master", &[("err", &e.to_string())]));
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
            Err(e) => {
                self.error =
                    Some(tf(self.lang, "err-unlock-new-vault", &[("err", &e.to_string())]))
            }
        }
    }

    // --------------------------------------------------------------- unlock

    fn show_unlock(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading(i18n::t(lang, "unlock-title"));
                ui.add_space(8.0);
                ui.label(i18n::t(lang, "unlock-intro"));
                ui.add_space(24.0);
                let field = ui.add_sized(
                    [FIELD_WIDTH, 24.0],
                    egui::TextEdit::singleline(&mut self.unlock_pw)
                        .password(true)
                        .hint_text(i18n::t(lang, "hint-master-password")),
                );
                let pressed_enter =
                    field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(12.0);
                let clicked = ui
                    .add_sized([FIELD_WIDTH, 32.0], egui::Button::new(i18n::t(lang, "btn-unlock")))
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
            self.error = Some(
                self.open_error
                    .clone()
                    .unwrap_or_else(|| i18n::t(self.lang, "err-db-not-open").into()),
            );
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
                    self.error = Some(i18n::t(self.lang, "err-wrong-master").into());
                    self.unlock_pw.clear();
                }
                other => {
                    self.error = Some(tf(self.lang, "err-unlock", &[("err", &other.to_string())]))
                }
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
        self.drain_results();
        self.show_top_bar(ctx);
        self.show_project_list(ctx);
        self.sync_status_for_selection();
        self.show_operations_panel(ctx);
        self.show_output_area(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            let lang = self.lang;
            match self.selected_project().cloned() {
                Some(project) => {
                    ui.heading(&project.name);
                    ui.label(tf(lang, "project-path", &[("path", &project.path)]));
                    ui.label(tf(lang, "project-added-at", &[("time", &project.added_at)]));
                    ui.separator();
                    // 防御性兜底：已选中项目但浏览器尚未初始化时，重置到项目根目录。
                    if self.browser_path.is_none() {
                        self.reset_browser_to(Path::new(&project.path));
                    }
                    self.show_browser(ui, Path::new(&project.path));
                }
                None => {
                    ui.heading("LocalRepoManager");
                    ui.label(i18n::t(lang, "main-empty-intro"));
                }
            }
            self.show_error(ui);
        });
        self.show_remove_confirm(ctx);
        self.show_clone_dialog(ctx);
        self.show_cred_dialog(ctx);
        self.show_pw_change_dialog(ctx);
        self.show_push_dialog(ctx);
        self.show_history_window(ctx);
        self.show_gen_dialog(ctx);
        self.show_tool_generator(ctx);
        // While a git call runs on a worker thread, keep waking the UI to
        // animate the progress bar and poll the result channel. Any in-flight
        // status batch counts, not just the selected project's, and the check
        // runs at the END of the frame: spawns happen mid-frame (selection
        // change,「刷新」button, post-operation refresh), and the「加载中」
        // bar is gone as soon as the first segment lands, so only this
        // condition reliably keeps frames alive until the batch completes.
        if self.busy.is_some() || !self.status_inflight.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(BUSY_REPAINT_MS));
        }
    }

    fn show_top_bar(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(
                    egui::RichText::new("LocalRepoManager")
                        .color(ui.visuals().hyperlink_color),
                );
                ui.separator();
                if ui
                    .button(format!(
                        "{} {}",
                        icons::ICON_ADD,
                        i18n::t(lang, "btn-add-project")
                    ))
                    .clicked()
                {
                    self.pick_and_add_project();
                }
                if ui
                    .button(format!(
                        "{} {}",
                        icons::ICON_CLONE,
                        i18n::t(lang, "btn-clone-repo")
                    ))
                    .clicked()
                {
                    self.open_clone_dialog();
                }
                let has_selection = self.selected.is_some();
                if ui
                    .add_enabled(
                        has_selection,
                        egui::Button::new(format!(
                            "{} {}",
                            icons::ICON_REMOVE,
                            i18n::t(lang, "btn-remove-project")
                        )),
                    )
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

    /// 顶栏菜单栏（egui::menu::bar）：主题切换（暗黑/明亮）、语言切换
    /// （中文/English）、工具（独立密码生成器）与设置（修改主密码）。
    fn show_menu_bar(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let mut chosen_theme: Option<Theme> = None;
        let mut chosen_lang: Option<Lang> = None;
        let mut open_generator = false;
        let mut open_pw_change = false;
        egui::menu::bar(ui, |ui| {
            ui.menu_button(i18n::t(lang, "menu-theme"), |ui| {
                if ui.button(i18n::t(lang, "theme-dark")).clicked() {
                    chosen_theme = Some(Theme::Dark);
                }
                if ui.button(i18n::t(lang, "theme-light")).clicked() {
                    chosen_theme = Some(Theme::Light);
                }
            });
            // 语言项用各自语言的本地化名称（中文 / English），任何界面
            // 语言下都可直接辨认。
            ui.menu_button(i18n::t(lang, "menu-language"), |ui| {
                if ui.button(i18n::t(lang, "lang-zh")).clicked() {
                    chosen_lang = Some(Lang::Zh);
                }
                if ui.button(i18n::t(lang, "lang-en")).clicked() {
                    chosen_lang = Some(Lang::En);
                }
            });
            ui.menu_button(i18n::t(lang, "menu-tools"), |ui| {
                if ui.button(i18n::t(lang, "tool-password-generator")).clicked() {
                    open_generator = true;
                }
            });
            ui.menu_button(i18n::t(lang, "menu-settings"), |ui| {
                if ui
                    .button(i18n::t(lang, "settings-change-master-password"))
                    .clicked()
                {
                    open_pw_change = true;
                }
            });
        });
        if let Some(theme) = chosen_theme {
            self.switch_theme(ui.ctx(), theme);
        }
        if let Some(value) = chosen_lang {
            self.switch_language(value);
        }
        if open_generator {
            self.open_tool_generator();
        }
        if open_pw_change {
            self.open_pw_change_dialog();
        }
    }

    /// 切换语言：立即生效（所有文案每帧经 t() 重新渲染），并尽力写入
    /// meta 表持久化；写失败仅记入日志区，不影响本次切换。
    fn switch_language(&mut self, lang: Lang) {
        self.lang = lang;
        let Some(conn) = self.conn.as_ref() else {
            let text = i18n::t(self.lang, "err-db-not-open").to_string();
            self.log_lang_save_error(&text);
            return;
        };
        if let Err(e) = db::set_meta(conn, i18n::LANG_KEY, i18n::to_meta(lang)) {
            self.log_lang_save_error(&e.to_string());
        }
    }

    /// 切换主题：立即生效，并尽力写入 meta 表持久化；写失败仅记入日志区，
    /// 不影响本次切换。
    fn switch_theme(&mut self, ctx: &egui::Context, theme: Theme) {
        self.theme = theme;
        self.apply_theme(ctx);
        let Some(conn) = self.conn.as_ref() else {
            let text = i18n::t(self.lang, "err-db-not-open").to_string();
            self.log_theme_save_error(&text);
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
            title: i18n::t(self.lang, "err-theme-save").to_string(),
            text: text.to_string(),
        });
        self.trim_log();
    }

    /// 语言偏好写库失败：仅在底部日志区提示一条失败记录。
    fn log_lang_save_error(&mut self, text: &str) {
        self.log.push(LogEntry {
            ok: false,
            title: i18n::t(self.lang, "err-lang-save").to_string(),
            text: text.to_string(),
        });
        self.trim_log();
    }

    fn show_project_list(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        egui::SidePanel::left("project_list").show(ctx, |ui| {
            ui.heading(i18n::t(lang, "heading-projects"));
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
                        ui.label(i18n::t(lang, "empty-project-list"));
                        return;
                    }
                    // 树状结构：路径逐段成层，每层缩进 2 个字符宽（按当前
                    // 字体实测 'M' 宽），目录名与项目名按层左对齐；目录节点
                    // 缺省展开，折叠态在会话内按节点 key 记忆。「未分组」
                    // 节点的 label 是稳定 key，展示时经 t() 翻译。
                    let paths: Vec<String> =
                        rows.iter().map(|(_, _, path)| path.clone()).collect();
                    let tree = build_project_tree(&paths, cfg!(windows));
                    let char_w = ui.fonts(|fonts| {
                        fonts.glyph_width(&egui::TextStyle::Body.resolve(ui.style()), 'M')
                    });
                    for node in &tree {
                        self.show_tree_node(ui, node, 0, &rows, lang, char_w);
                    }
                });
        });
    }

    /// 递归渲染项目树的一层：目录行（箭头 + 目录名）左对齐在当前层
    /// 缩进处，子目录与直属项目各缩进 2 个字符宽。目录节点缺省展开，
    /// 点击箭头或目录名切换折叠态（`collapsed_tree_dirs` 会话内记忆）。
    fn show_tree_node(
        &mut self,
        ui: &mut egui::Ui,
        node: &DirNode,
        depth: usize,
        rows: &[(i64, String, String)],
        lang: Lang,
        char_w: f32,
    ) {
        let indent = depth as f32 * 2.0 * char_w;
        let open = !self.collapsed_tree_dirs.contains(&node.key);
        let title = if node.key.is_empty() {
            i18n::t(lang, GROUP_UNGROUPED_KEY)
        } else {
            node.label.as_str()
        };
        let row_h = ui.spacing().interact_size.y;
        let mut toggle = false;
        ui.horizontal(|ui| {
            // 精确对齐：本行内去掉控件间距，目录名的 x 恒为 indent + 2 个字符
            // 宽（箭头槽），与子层项目名的起点一致。
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(indent);
            let (rect, arrow_resp) =
                ui.allocate_exact_size(egui::vec2(2.0 * char_w, row_h), egui::Sense::click());
            paint_tree_arrow(ui, rect, open);
            let name_resp = ui.add(
                egui::Label::new(egui::RichText::new(title).strong())
                    .sense(egui::Sense::click())
                    .truncate(),
            );
            if arrow_resp.clicked() || name_resp.clicked() {
                toggle = true;
            }
            let mut resp = arrow_resp.union(name_resp);
            if !node.key.is_empty() {
                resp = resp.on_hover_text(&node.key);
            }
            resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        });
        if toggle {
            if self.collapsed_tree_dirs.contains(&node.key) {
                self.collapsed_tree_dirs.remove(&node.key);
            } else {
                self.collapsed_tree_dirs.insert(node.key.clone());
            }
        }
        if !open {
            return;
        }
        let child_indent = (depth + 1) as f32 * 2.0 * char_w;
        for &idx in &node.projects {
            let (id, name, path) = &rows[idx];
            let selected = self.selected == Some(*id);
            let clicked = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(child_indent);
                // 自绘整行（保留整行可点击与选中高亮），文本严格左对齐：
                // egui 的 add_sized 内部是居中对齐布局，会把项目名画在行中间。
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 22.0),
                    egui::Sense::click(),
                );
                if ui.is_rect_visible(rect) {
                    let visuals = ui.style().interact_selectable(&resp, selected);
                    if selected || resp.hovered() {
                        let bg = rect.expand(visuals.expansion);
                        ui.painter().rect(
                            bg,
                            visuals.rounding,
                            visuals.weak_bg_fill,
                            visuals.bg_stroke,
                        );
                    }
                    let text_x = rect.left() + ui.spacing().button_padding.x;
                    let wrap_width =
                        (rect.right() - ui.spacing().button_padding.x - text_x).max(0.0);
                    let galley = ui.painter().layout(
                        name.clone(),
                        egui::TextStyle::Button.resolve(ui.style()),
                        visuals.text_color(),
                        wrap_width,
                    );
                    let pos = egui::pos2(text_x, rect.center().y - galley.size().y * 0.5);
                    ui.painter().galley(pos, galley, visuals.text_color());
                }
                let mut resp = resp;
                if !path.is_empty() {
                    resp = resp.on_hover_text(path.as_str());
                }
                resp.clicked()
            });
            let clicked = clicked.inner;
            if clicked && self.selected != Some(*id) {
                // 选中项目变化：浏览器重置为新项目的根目录。
                self.selected = Some(*id);
                self.reset_browser_to(Path::new(path));
            }
        }
        for child in &node.dirs {
            self.show_tree_node(ui, child, depth + 1, rows, lang, char_w);
        }
    }

    // ---------------------------------------------------- project operations

    /// Refresh the status area when the selection changes: render the cached
    /// snapshot instantly, or show「加载中」with an animated progress bar and
    /// spawn a background refresh when there is no cache yet.
    fn sync_status_for_selection(&mut self) {
        if self.selected == self.status_loaded_for {
            return;
        }
        match self.selected {
            Some(id) => {
                self.status_loaded_for = Some(id);
                match self.status_cache.get(&id) {
                    Some(cache) => self.status = Some(snapshot_from_cache(cache)),
                    None => {
                        self.status = None; //「加载中…」+ progress bar
                        if let Some(project) = self.projects.iter().find(|p| p.id == id) {
                            let path = PathBuf::from(&project.path);
                            self.spawn_status_refresh(id, path);
                        }
                    }
                }
            }
            None => {
                self.status_loaded_for = None;
                self.status = None;
            }
        }
    }

    fn show_operations_panel(&mut self, ctx: &egui::Context) {
        let busy = self.busy.clone();
        let lang = self.lang;
        egui::SidePanel::right("operations_panel")
            .default_width(380.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading(i18n::t(lang, "heading-operations"));
                ui.separator();
                if let Some(op) = &busy {
                    // Indeterminate animated progress bar while the write
                    // operation runs on its worker thread.
                    ui.add(egui::ProgressBar::new(0.0).animate(true));
                    let project_name = self
                        .projects
                        .iter()
                        .find(|p| p.id == op.project_id)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    if project_name.is_empty() {
                        ui.label(&op.label);
                    } else {
                        ui.label(tf(
                            lang,
                            "busy-op-project",
                            &[("label", &op.label), ("project", &project_name)],
                        ));
                    }
                    ui.add_space(4.0);
                    ui.separator();
                }
                if self.selected_project().is_none() {
                    ui.label(i18n::t(lang, "empty-ops-intro"));
                    return;
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.show_status_area(ui, busy.is_some());
                        ui.add_space(8.0);
                        ui.separator();
                        self.show_commit_area(ui, busy.is_some());
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !busy.is_some(),
                                    egui::Button::new(format!(
                                        "{} {}",
                                        icons::ICON_PULL,
                                        i18n::t(lang, "btn-pull")
                                    )),
                                )
                                .clicked()
                            {
                                self.do_pull();
                            }
                            if ui
                                .add_enabled(
                                    !busy.is_some(),
                                    egui::Button::new(format!(
                                        "{} {}",
                                        icons::ICON_PUSH,
                                        i18n::t(lang, "btn-push")
                                    )),
                                )
                                .clicked()
                            {
                                self.do_push();
                            }
                            // 一键推送保持不变；「推送…」打开可选远程/分支的对话框。
                            if ui
                                .add_enabled(
                                    !busy.is_some(),
                                    egui::Button::new(format!(
                                        "{} {}",
                                        icons::ICON_PUSH,
                                        i18n::t(lang, "btn-push-dialog")
                                    )),
                                )
                                .clicked()
                            {
                                self.open_push_dialog();
                            }
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !busy.is_some(),
                                    egui::Button::new(format!(
                                        "{} {}",
                                        icons::ICON_HISTORY,
                                        i18n::t(lang, "btn-history")
                                    )),
                                )
                                .clicked()
                            {
                                self.open_history_window();
                            }
                            if ui
                                .add_enabled(
                                    !busy.is_some(),
                                    egui::Button::new(format!(
                                        "{} {}",
                                        icons::ICON_KEY,
                                        i18n::t(lang, "btn-credentials")
                                    )),
                                )
                                .clicked()
                            {
                                self.open_cred_dialog();
                            }
                        });
                        ui.add_space(8.0);
                        ui.separator();
                        self.show_branch_area(ui, busy.is_some());
                        ui.add_space(8.0);
                        self.show_tag_area(ui, busy.is_some());
                    });
            });
    }

    /// 状态区：status --porcelain 输出 + 分支列表 + 标签列表。无快照且选中了
    /// 项目时显示「加载中…」+ 动态进度条（后台线程正在取数）。
    fn show_status_area(&mut self, ui: &mut egui::Ui, busy: bool) {
        let lang = self.lang;
        ui.horizontal(|ui| {
            ui.strong(i18n::t(lang, "heading-status"));
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(format!(
                        "{} {}",
                        icons::ICON_REFRESH,
                        i18n::t(lang, "btn-refresh")
                    )),
                )
                .clicked()
            {
                self.manual_refresh_status();
            }
        });
        let Some(snapshot) = self.status.as_ref() else {
            ui.label(i18n::t(lang, "loading"));
            ui.add(egui::ProgressBar::new(0.0).animate(true));
            return;
        };
        // 「工作区」区展示人类可读的状态文字（原始 porcelain 输出仍完整
        // 记录在底部输出区，便于排障）；分支/标签区维持原样。
        let worktree = snapshot.status.as_ref().map(|section| {
            if section.ok {
                StatusSection {
                    ok: true,
                    text: format_worktree_status(lang, &section.text),
                }
            } else {
                StatusSection {
                    ok: false,
                    text: section.text.clone(),
                }
            }
        });
        show_section(
            ui,
            lang,
            i18n::t(lang, "section-worktree"),
            worktree.as_ref(),
        );
        show_section(ui, lang, i18n::t(lang, "section-branches"), snapshot.branches.as_ref());
        show_section(ui, lang, i18n::t(lang, "section-tags"), snapshot.tags.as_ref());
    }

    /// Commit 消息输入框 + Commit 按钮（空消息禁用；busy 期间禁用）。
    fn show_commit_area(&mut self, ui: &mut egui::Ui, busy: bool) {
        let lang = self.lang;
        ui.strong(i18n::t(lang, "heading-commit"));
        ui.add_sized(
            [ui.available_width(), 22.0],
            egui::TextEdit::singleline(&mut self.commit_msg)
                .hint_text(i18n::t(lang, "hint-commit-message")),
        );
        let can_commit = !self.commit_msg.trim().is_empty() && !busy;
        if ui
            .add_enabled(
                can_commit,
                egui::Button::new(format!(
                    "{} {}",
                    icons::ICON_COMMIT,
                    i18n::t(lang, "btn-commit")
                )),
            )
            .clicked()
        {
            self.do_commit();
        }
    }

    /// “新建分支”：名称输入 → create_branch + checkout。
    fn show_branch_area(&mut self, ui: &mut egui::Ui, busy: bool) {
        let lang = self.lang;
        ui.strong(i18n::t(lang, "heading-new-branch"));
        ui.add_sized(
            [ui.available_width(), 22.0],
            egui::TextEdit::singleline(&mut self.new_branch)
                .hint_text(i18n::t(lang, "hint-branch-name")),
        );
        let can_create = !self.new_branch.trim().is_empty() && !busy;
        if ui
            .add_enabled(
                can_create,
                egui::Button::new(format!(
                    "{} {}",
                    icons::ICON_BRANCH,
                    i18n::t(lang, "btn-create-checkout")
                )),
            )
            .clicked()
        {
            self.do_new_branch();
        }
    }

    /// “打 tag”：名称 + 可选 message → create_tag。
    fn show_tag_area(&mut self, ui: &mut egui::Ui, busy: bool) {
        let lang = self.lang;
        ui.strong(i18n::t(lang, "heading-tag"));
        ui.add_sized(
            [ui.available_width(), 22.0],
            egui::TextEdit::singleline(&mut self.tag_name)
                .hint_text(i18n::t(lang, "hint-tag-name")),
        );
        ui.add_sized(
            [ui.available_width(), 22.0],
            egui::TextEdit::singleline(&mut self.tag_msg)
                .hint_text(i18n::t(lang, "hint-tag-message")),
        );
        let can_tag = !self.tag_name.trim().is_empty() && !busy;
        if ui
            .add_enabled(
                can_tag,
                egui::Button::new(format!(
                    "{} {}",
                    icons::ICON_TAG,
                    i18n::t(lang, "btn-tag")
                )),
            )
            .clicked()
        {
            self.do_tag();
        }
    }

    /// 底部只读输出区：每次 git 调用一条记录，失败为红色。
    fn show_output_area(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        egui::TopBottomPanel::bottom("output_area")
            .default_height(180.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(i18n::t(lang, "heading-output"));
                    if ui.button(i18n::t(lang, "btn-clear")).clicked() {
                        self.log.clear();
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.log.is_empty() {
                            ui.label(i18n::t(lang, "empty-output"));
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
                self.browser_err =
                    Some(tf(self.lang, "err-read-dir", &[("err", &e.to_string())]));
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
            Err(e) => {
                self.browser_err =
                    Some(tf(self.lang, "err-enter-dir", &[("err", &e.to_string())]))
            }
        }
    }

    /// 中央文件浏览器：只读路径行 +「上一级」+ 多列条目列表。
    fn show_browser(&mut self, ui: &mut egui::Ui, root: &Path) {
        let lang = self.lang;
        ui.horizontal(|ui| {
            ui.strong(i18n::t(lang, "location-label"));
            let path_text = self
                .browser_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| i18n::t(lang, "not-loaded-path").to_string());
            // 只读文本（Label 不可编辑）；固定宽度截断，避免长路径把按钮挤出面板。
            ui.add_sized(
                [(ui.available_width() - 96.0).max(0.0), 18.0],
                egui::Label::new(egui::RichText::new(&path_text).monospace())
                    .wrap_mode(egui::TextWrapMode::Truncate),
            );
            let at_root = self.browser_path.as_ref().is_none_or(|p| p == root);
            if ui
                .add_enabled(!at_root, egui::Button::new(i18n::t(lang, "btn-parent-dir")))
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

        // 文件状态标记：数据源是选中项目的 porcelain 缓存；未选中项目或
        // 无缓存时为空映射（不显示标记，不报错）。
        let status_map = self
            .selected_project()
            .and_then(|p| self.status_cache.get(&p.id))
            .map(|cache| parse_status_map(&cache.porcelain))
            .unwrap_or_default();
        // 当前浏览目录相对仓库根的路径（porcelain 路径恒以 `/` 分隔）。
        let rel_prefix: String = self
            .browser_path
            .as_ref()
            .and_then(|p| p.strip_prefix(root).ok())
            .map(|rel| {
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default();
        // 每行的状态：文件精确匹配，目录做后代前缀匹配。
        let row_status: Vec<Option<FileStatus>> = self
            .browser_entries
            .iter()
            .map(|e| {
                let rel = if rel_prefix.is_empty() {
                    e.name.clone()
                } else {
                    format!("{rel_prefix}/{}", e.name)
                };
                status_for_entry(&status_map, &rel, e.is_dir)
            })
            .collect();

        // 中央浏览器主体：egui_extras::TableBuilder。
        // 四列均可拖拽调宽（列宽由 egui memory 在会话内记忆，不落盘），
        // 表头常驻，纵向滚动 + 行虚拟化（rows() 只绘制视口内的行）。
        TableBuilder::new(ui)
            .striped(true)
            // 调用点处于自上而下布局，单元格内必须显式恢复水平布局，
            // 否则同一格内的状态标记与名称会纵向堆叠。
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            // 与旧 ScrollArea 的 auto_shrink([false, false]) 行为一致：
            // 填满中央面板剩余高度，且不限制最大滚动高度。
            .auto_shrink([false, false])
            .max_scroll_height(f32::INFINITY)
            .resizable(true)
            // 名称列占大头并裁剪过长文件名；其余列窄一些。
            .column(Column::initial(240.0).resizable(true).clip(true).at_least(80.0))
            .column(Column::initial(90.0).resizable(true).at_least(40.0))
            .column(Column::initial(90.0).resizable(true).at_least(40.0))
            .column(Column::initial(130.0).resizable(true).at_least(60.0))
            .header(20.0, |mut header| {
                header.col(|ui| {
                    ui.strong(i18n::t(lang, "col-name"));
                });
                header.col(|ui| {
                    ui.strong(i18n::t(lang, "col-type"));
                });
                header.col(|ui| {
                    ui.strong(i18n::t(lang, "col-size"));
                });
                header.col(|ui| {
                    ui.strong(i18n::t(lang, "col-modified"));
                });
            })
            .body(|body| {
                body.rows(BROWSER_ROW_HEIGHT, rows.len(), |mut row| {
                    let i = row.index();
                    let (name, is_dir, size, modified, is_hidden) = &rows[i];
                    let entry_status = row_status[i];

                    // 隐藏项（.git 等）正常显示但整行视觉弱化，仍可进入。
                    let dim = |rt: egui::RichText| {
                        if *is_hidden {
                            rt.weak()
                        } else {
                            rt
                        }
                    };
                    // 目录行：四格均可点击（点击行内任意位置进入）；
                    // 文件行：无动作（仅 hover 感应）。
                    let sense = if *is_dir {
                        egui::Sense::click()
                    } else {
                        egui::Sense::hover()
                    };
                    // 名称格：git 状态标记（着色）在前，名称随后；
                    // 两者感应一致，目录行点标记也能进入。
                    let mut name_resp: Option<egui::Response> = None;
                    row.col(|ui| {
                        let marker = entry_status.map(|st| {
                            ui.add(egui::Label::new(status_marker_rich(st)).sense(sense))
                        });
                        let name_label = ui.add(
                            egui::Label::new(dim(egui::RichText::new(name)))
                                .wrap_mode(egui::TextWrapMode::Truncate)
                                .sense(sense),
                        );
                        name_resp = Some(match marker {
                            Some(marker_resp) => name_label.union(marker_resp),
                            None => name_label,
                        });
                    });
                    let (_, type_resp) = row.col(|ui| {
                        ui.add(
                            egui::Label::new(dim(egui::RichText::new(if *is_dir {
                                i18n::t(lang, "type-dir")
                            } else {
                                i18n::t(lang, "type-file")
                            })))
                            .sense(sense),
                        );
                    });
                    let (_, size_resp) = row.col(|ui| {
                        let size_text = if *is_dir {
                            i18n::t(lang, "placeholder-dash").to_string()
                        } else {
                            browser::fmt_size(*size)
                        };
                        ui.add(egui::Label::new(dim(egui::RichText::new(size_text))).sense(sense));
                    });
                    let (_, time_resp) = row.col(|ui| {
                        ui.add(
                            egui::Label::new(dim(egui::RichText::new(
                                modified.as_deref().unwrap_or(i18n::t(lang, "placeholder-dash")),
                            )))
                            .sense(sense),
                        );
                    });

                    if *is_dir {
                        // 表格整格响应默认只有 hover 感应，因此沿用各格内
                        // Label 的响应并集来判定「点击行内任意位置进入」。
                        let cells =
                            [name_resp, Some(type_resp), Some(size_resp), Some(time_resp)];
                        if let Some(row_resp) =
                            cells.into_iter().flatten().reduce(|acc, r| acc.union(r))
                        {
                            if row_resp.clicked() {
                                if let Some(target) =
                                    self.browser_path.as_ref().map(|p| p.join(name))
                                {
                                    self.load_browser_dir(target);
                                }
                            }
                            row_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                        }
                    }
                });
            });

        if rows.is_empty() && self.browser_err.is_none() {
            ui.label(i18n::t(lang, "empty-dir"));
        }

        // 行尾图例：porcelain 有数据（存在状态标记）时显示一次。
        if !status_map.is_empty() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(i18n::t(lang, "legend-label")).weak());
                for st in [
                    FileStatus::Conflict,
                    FileStatus::Deleted,
                    FileStatus::Modified,
                    FileStatus::Renamed,
                    FileStatus::Added,
                ] {
                    ui.label(status_marker_rich(st));
                    ui.label(egui::RichText::new(status_legend_text(lang, st)).weak());
                    ui.add_space(6.0);
                }
            });
        }
    }

    // -------------------------------------------------- git operation handlers

    /// Append a finished git invocation to the bottom output log.
    fn log_git_output(&mut self, label: &str, out: &git::GitOutput) {
        self.log.push(LogEntry {
            ok: out.ok(),
            title: tf(
                self.lang,
                "log-exit-code",
                &[("label", label), ("code", &out.code.to_string())],
            ),
            text: git_output_text(self.lang, out),
        });
        self.trim_log();
    }

    /// Append a failed-to-run entry (git could not be spawned / cwd missing).
    fn log_git_error(&mut self, label: &str, err: &std::io::Error) {
        self.log.push(LogEntry {
            ok: false,
            title: tf(self.lang, "log-run-failed", &[("label", label)]),
            text: tf(self.lang, "err-git-run", &[("err", &err.to_string())]),
        });
        self.trim_log();
    }

    fn trim_log(&mut self) {
        if self.log.len() > LOG_MAX {
            let excess = self.log.len() - LOG_MAX;
            self.log.drain(0..excess);
        }
    }

    /// Drain every result the worker threads have delivered so far, in send
    /// order. All UI-state mutation (output log, status snapshot, busy flag)
    /// happens here on the UI thread only.
    fn drain_results(&mut self) {
        while let Ok(result) = self.result_rx.try_recv() {
            self.apply_result(result);
        }
    }

    /// Route one worker result: update the per-project cache unconditionally,
    /// but only touch the on-screen display when the result belongs to the
    /// currently selected project (`should_apply`).
    fn apply_result(&mut self, result: GitResult) {
        let project_id = result.project_id;
        match result.kind {
            ResultKind::Status => self.apply_status_part(result, StatusPart::Status),
            ResultKind::BranchList => self.apply_status_part(result, StatusPart::Branches),
            ResultKind::TagList => self.apply_status_part(result, StatusPart::Tags),
            ResultKind::Head => match &result.output {
                Ok(out) => {
                    self.log_git_output(LABEL_HEAD, out);
                    // A failed rev-parse aborts the pull/push early: the
                    // terminal Pull/Push message never comes, so clear busy
                    // here. An empty-branch abort arrives as a synthesized
                    // Pull/Push message instead.
                    if !out.ok() {
                        self.clear_busy_if(project_id);
                    }
                }
                Err(e) => {
                    self.log_git_error(LABEL_HEAD, e);
                    self.clear_busy_if(project_id);
                }
            },
            ResultKind::RemoteUrl(remote) => {
                // Only io failures are sent (and logged); successful and
                // git-failed get-url results degrade to a plain pull/push
                // silently, like the old synchronous flow.
                if let Err(e) = &result.output {
                    let label = format!("git remote get-url {remote}");
                    self.log_git_error(&label, e);
                }
            }
            ResultKind::Commit(label) => {
                self.log_write_result(&label, &result.output);
                if let Ok(out) = &result.output {
                    if out.ok() && should_apply(project_id, self.selected) {
                        self.commit_msg.clear();
                    }
                }
                self.clear_busy_if(project_id);
                self.trigger_status_refresh(project_id);
            }
            ResultKind::Pull(label) | ResultKind::Push(label) | ResultKind::PushTo(label) => {
                self.log_write_result(&label, &result.output);
                self.clear_busy_if(project_id);
                self.trigger_status_refresh(project_id);
            }
            ResultKind::BranchCreate(label) => {
                self.log_write_result(&label, &result.output);
                // On failure the worker stops after this call; on success the
                // following checkout message ends the operation.
                match &result.output {
                    Ok(out) if out.ok() => {}
                    _ => self.clear_busy_if(project_id),
                }
            }
            ResultKind::BranchCheckout(label) => {
                self.log_write_result(&label, &result.output);
                if let Ok(out) = &result.output {
                    if out.ok() && should_apply(project_id, self.selected) {
                        self.new_branch.clear();
                    }
                }
                self.clear_busy_if(project_id);
                self.trigger_status_refresh(project_id);
            }
            ResultKind::Tag(label) => {
                self.log_write_result(&label, &result.output);
                if let Ok(out) = &result.output {
                    if out.ok() && should_apply(project_id, self.selected) {
                        self.tag_name.clear();
                        self.tag_msg.clear();
                    }
                }
                self.clear_busy_if(project_id);
                self.trigger_status_refresh(project_id);
            }
        }
    }

    /// Apply one part (status/branches/tags) of a read-only refresh batch:
    /// cache successful output, log + display it when the project is selected.
    fn apply_status_part(&mut self, result: GitResult, part: StatusPart) {
        let project_id = result.project_id;
        // The in-flight guard stays closed until the batch's last segment
        // (TagList, always sent last by the worker) lands: clearing it on the
        // first message would end the batch-driven repaints right after the
        // status text fills in, leaving BranchList/TagList stuck in the
        // channel until the next user input. It also keeps the duplicate-
        // spawn guard closed for the whole batch.
        if matches!(part, StatusPart::Tags) {
            self.status_inflight.remove(&project_id);
        }
        let label = match part {
            StatusPart::Status => LABEL_STATUS,
            StatusPart::Branches => LABEL_BRANCHES,
            StatusPart::Tags => LABEL_TAGS,
        };
        let applies = should_apply(project_id, self.selected);
        if applies {
            match &result.output {
                Ok(out) => self.log_git_output(label, out),
                Err(e) => self.log_git_error(label, e),
            }
        }
        if let Ok(out) = &result.output {
            if out.ok() {
                let entry = self.status_cache.entry(project_id).or_insert_with(CachedStatus::empty);
                match part {
                    StatusPart::Status => entry.porcelain = git_output_text(self.lang, out),
                    StatusPart::Branches => entry.branches = output_lines(out),
                    StatusPart::Tags => entry.tags = output_lines(out),
                }
                entry.loaded_at = std::time::Instant::now();
            }
        }
        if applies {
            let section = match &result.output {
                Ok(out) => Some(status_section(self.lang, out)),
                Err(e) => Some(StatusSection {
                    ok: false,
                    text: tf(self.lang, "err-git-run", &[("err", &e.to_string())]),
                }),
            };
            let snapshot = self.status.get_or_insert_with(StatusSnapshot::default);
            match part {
                StatusPart::Status => snapshot.status = section,
                StatusPart::Branches => snapshot.branches = section,
                StatusPart::Tags => snapshot.tags = section,
            }
            self.status_loaded_for = Some(project_id);
        }
    }

    /// Spawn one worker thread that runs all three read-only queries for a
    /// project and sends each result back tagged with the project id.
    fn spawn_status_refresh(&mut self, project_id: i64, path: PathBuf) {
        if !self.status_inflight.insert(project_id) {
            return; // a batch for this project is already running
        }
        let tx = self.result_tx.clone();
        std::thread::spawn(move || {
            let send = |kind: ResultKind, output: std::io::Result<git::GitOutput>| {
                let _ = tx.send(GitResult { project_id, kind, output });
            };
            send(ResultKind::Status, git::status_porcelain(&path));
            send(ResultKind::BranchList, git::branch_list(&path));
            send(ResultKind::TagList, git::tag_list(&path));
        });
    }

    /// Re-run the read-only batch for a project (after a write operation or
    /// via the「刷新」button); skipped when a batch is already in flight.
    fn trigger_status_refresh(&mut self, project_id: i64) {
        if let Some(project) = self.projects.iter().find(|p| p.id == project_id) {
            let path = PathBuf::from(&project.path);
            self.spawn_status_refresh(project_id, path);
        }
    }

    /// 「刷新」button: force a background refresh of the selected project.
    fn manual_refresh_status(&mut self) {
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let path = PathBuf::from(&project.path);
        self.spawn_status_refresh(project.id, path);
    }

    /// End the busy indicator when a terminal message of the running write
    /// operation arrives (project id must match so a stale message cannot
    /// cut short a newer operation).
    fn clear_busy_if(&mut self, project_id: i64) {
        if self.busy.as_ref().is_some_and(|op| op.project_id == project_id) {
            self.busy = None;
        }
    }

    /// Log a write-operation result (output, or the io error when git could
    /// not run at all).
    fn log_write_result(&mut self, label: &str, output: &std::io::Result<git::GitOutput>) {
        match output {
            Ok(out) => self.log_git_output(label, out),
            Err(e) => self.log_git_error(label, e),
        }
    }

    /// Decrypt the stored credential for a project on the UI thread (the
    /// credential label is usually `origin`, or the remote chosen in the push
    /// dialog). The secret only moves into the worker thread, where the URL
    /// is built and scrubbed; it is never logged, rendered, or stored
    /// anywhere. Returns `None` when absent or on failure (the failure is
    /// logged here).
    fn load_credential_parts(&mut self, project_id: i64, label: &str) -> Option<(String, String)> {
        let secret = {
            let keys = self.keys.as_ref()?;
            let conn = self.conn.as_ref()?;
            match db::get_credential(conn, keys, project_id, label) {
                Ok(Some(secret)) => secret,
                Ok(None) => return None,
                Err(e) => {
                    self.log.push(LogEntry {
                        ok: false,
                        title: tf(self.lang, "err-load-credential", &[("label", label)]),
                        // CryptoError messages never contain the secret.
                        text: e.to_string(),
                    });
                    self.trim_log();
                    return None;
                }
            }
        };
        Some(split_credential_secret(&secret))
    }

    /// Commit 按钮：`commit_all` 在后台线程执行；完成后由结果回填清 busy、
    /// 记日志并刷新状态区。
    fn do_commit(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let message = self.commit_msg.trim().to_string();
        if message.is_empty() {
            return; // the button is disabled; defensive only
        }
        self.busy = Some(BusyOp {
            project_id: project.id,
            label: i18n::t(self.lang, "busy-commit").to_string(),
        });
        let tx = self.result_tx.clone();
        let path = PathBuf::from(&project.path);
        let project_id = project.id;
        let label = format!("git add -A + git commit -m {message}");
        std::thread::spawn(move || {
            let output = git::commit_all(&path, &message);
            let _ = tx.send(GitResult { project_id, kind: ResultKind::Commit(label), output });
        });
    }

    /// Pull 按钮：凭据存在且 origin 为 https 时在后台线程注入凭据拉取（URL
    /// 拼接与输出脱敏都在线程内完成，明文不回流 UI 线程）。
    fn do_pull(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let auth_parts = self.load_credential_parts(project.id, DEFAULT_PUSH_REMOTE);
        self.busy = Some(BusyOp {
            project_id: project.id,
            label: i18n::t(self.lang, "busy-pull").to_string(),
        });
        let tx = self.result_tx.clone();
        let path = PathBuf::from(&project.path);
        let project_id = project.id;
        let lang = self.lang;
        std::thread::spawn(move || {
            let head = git::run(&path, &["rev-parse", "--abbrev-ref", "HEAD"]);
            let branch = match &head {
                Ok(out) if out.ok() => out.stdout.trim().to_string(),
                _ => String::new(),
            };
            let _ = tx.send(GitResult { project_id, kind: ResultKind::Head, output: head });
            if branch.is_empty() {
                // Detached HEAD or git failure: abort like the old synchronous
                // flow, with the same message carried as a synthesized output.
                let output = Ok(git::GitOutput {
                    code: 1,
                    stdout: String::new(),
                    stderr: i18n::t(lang, "err-no-current-branch").to_string(),
                });
                let _ = tx.send(GitResult {
                    project_id,
                    kind: ResultKind::Pull("git pull".to_string()),
                    output,
                });
                return;
            }
            let Some((user, pass)) = auth_parts else {
                send_plain_pull(&tx, project_id, &path);
                return;
            };
            let plain_url = match git::run(&path, &["remote", "get-url", DEFAULT_PUSH_REMOTE]) {
                Ok(out) if out.ok() => out.stdout.trim().to_string(),
                // git's own failure surfaces on the subsequent pull call.
                Ok(_) => {
                    send_plain_pull(&tx, project_id, &path);
                    return;
                }
                Err(e) => {
                    let _ = tx.send(GitResult {
                        project_id,
                        kind: ResultKind::RemoteUrl(DEFAULT_PUSH_REMOTE.to_string()),
                        output: Err(e),
                    });
                    send_plain_pull(&tx, project_id, &path);
                    return;
                }
            };
            let Some(url) = inject_https_userinfo(&plain_url, &user, &pass) else {
                send_plain_pull(&tx, project_id, &path);
                return;
            };
            let auth = InjectedAuth {
                url,
                plain_url,
                password: pass.clone(),
                encoded_password: percent_encode_userinfo(&pass),
            };
            // `git pull <url>` without a refspec would merge the remote's
            // HEAD; passing the current branch keeps the default `git pull`
            // behaviour of merging the same-named upstream branch.
            let label = format!(
                "git pull {DEFAULT_PUSH_REMOTE} {branch}{}",
                i18n::t(lang, "cred-injected")
            );
            let output = git::run(&path, &["pull", &auth.url, &branch])
                .map(|out| redact_output(out, &auth.redact_pairs()));
            let _ = tx.send(GitResult { project_id, kind: ResultKind::Pull(label), output });
        });
    }

    /// Push 按钮：先取当前分支，再推送到 origin（凭据存在且 remote 为 https
    /// 时在后台线程注入凭据，输出在线程内脱敏后回流）。
    fn do_push(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let auth_parts = self.load_credential_parts(project.id, DEFAULT_PUSH_REMOTE);
        self.busy = Some(BusyOp {
            project_id: project.id,
            label: i18n::t(self.lang, "busy-push").to_string(),
        });
        let tx = self.result_tx.clone();
        let path = PathBuf::from(&project.path);
        let project_id = project.id;
        let lang = self.lang;
        std::thread::spawn(move || {
            let head = git::run(&path, &["rev-parse", "--abbrev-ref", "HEAD"]);
            let branch = match &head {
                Ok(out) if out.ok() => out.stdout.trim().to_string(),
                _ => String::new(),
            };
            let _ = tx.send(GitResult { project_id, kind: ResultKind::Head, output: head });
            if branch.is_empty() {
                let output = Ok(git::GitOutput {
                    code: 1,
                    stdout: String::new(),
                    stderr: i18n::t(lang, "err-no-current-branch").to_string(),
                });
                let _ = tx.send(GitResult {
                    project_id,
                    kind: ResultKind::Push("git push".to_string()),
                    output,
                });
                return;
            }
            let Some((user, pass)) = auth_parts else {
                send_plain_push(&tx, project_id, &path, &branch);
                return;
            };
            let plain_url = match git::run(&path, &["remote", "get-url", DEFAULT_PUSH_REMOTE]) {
                Ok(out) if out.ok() => out.stdout.trim().to_string(),
                Ok(_) => {
                    send_plain_push(&tx, project_id, &path, &branch);
                    return;
                }
                Err(e) => {
                    let _ = tx.send(GitResult {
                        project_id,
                        kind: ResultKind::RemoteUrl(DEFAULT_PUSH_REMOTE.to_string()),
                        output: Err(e),
                    });
                    send_plain_push(&tx, project_id, &path, &branch);
                    return;
                }
            };
            let Some(url) = inject_https_userinfo(&plain_url, &user, &pass) else {
                send_plain_push(&tx, project_id, &path, &branch);
                return;
            };
            let auth = InjectedAuth {
                url,
                plain_url,
                password: pass.clone(),
                encoded_password: percent_encode_userinfo(&pass),
            };
            let label = format!(
                "git push {DEFAULT_PUSH_REMOTE} {branch}{}",
                i18n::t(lang, "cred-injected")
            );
            let output = git::push(&path, &auth.url, &branch)
                .map(|out| redact_output(out, &auth.redact_pairs()));
            let _ = tx.send(GitResult { project_id, kind: ResultKind::Push(label), output });
        });
    }

    // ---------------------------------------------------------- push dialog

    /// 操作面板「推送…」：打开推送对话框。remote 列表同步读取（查询快、
    /// 秒回，裁定不走 busy 异步）；默认分支从状态缓存 porcelain 的
    /// `## <branch>` 行取当前分支。
    fn open_push_dialog(&mut self) {
        self.error = None;
        self.push_error = None;
        self.push_remotes_error = None;
        self.push_set_upstream = false;
        self.push_remote_sel = 0;
        self.push_remotes.clear();
        self.push_branch = self
            .selected_project()
            .and_then(|p| self.status_cache.get(&p.id))
            .map(|cache| current_branch_from_porcelain(&cache.porcelain).unwrap_or_default())
            .unwrap_or_default();
        let Some(project) = self.selected_project().cloned() else {
            return; // 按钮仅在选中项目后可见；防御性兜底
        };
        match git::remote_list(Path::new(&project.path)) {
            Ok(remotes) => self.push_remotes = remotes,
            Err(e) => self.push_remotes_error = Some(e),
        }
        self.show_push_dialog = true;
    }

    fn close_push_dialog(&mut self) {
        self.show_push_dialog = false;
        self.push_remotes.clear();
        self.push_remote_sel = 0;
        self.push_branch.clear();
        self.push_set_upstream = false;
        self.push_error = None;
        self.push_remotes_error = None;
    }

    fn show_push_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_push_dialog {
            return;
        }
        let lang = self.lang;
        let Some(project_name) = self.selected_project().map(|p| p.name.clone()) else {
            self.show_push_dialog = false;
            return;
        };
        let busy = self.busy.is_some();
        let no_remotes = self.push_remotes.is_empty();
        let mut open = self.show_push_dialog;
        egui::Window::new(tf(
            lang,
            "push-dialog-title",
            &[("name", &project_name)],
        ))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(i18n::t(lang, "push-dialog-intro"));
            ui.add_space(6.0);
            if let Some(err) = &self.push_remotes_error {
                ui.colored_label(egui::Color32::RED, err);
            } else if no_remotes {
                ui.label(i18n::t(lang, "push-no-remotes"));
            } else {
                egui::ComboBox::from_label(i18n::t(lang, "push-remote-label"))
                    .selected_text(
                        self.push_remotes
                            .get(self.push_remote_sel)
                            .map(String::as_str)
                            .unwrap_or_default(),
                    )
                    .show_ui(ui, |ui| {
                        for (idx, remote) in self.push_remotes.iter().enumerate() {
                            ui.selectable_value(&mut self.push_remote_sel, idx, remote);
                        }
                    });
            }
            ui.add_space(6.0);
            ui.add_sized(
                [DIALOG_FIELD_WIDTH, 22.0],
                egui::TextEdit::singleline(&mut self.push_branch)
                    .hint_text(i18n::t(lang, "hint-push-branch")),
            );
            ui.checkbox(&mut self.push_set_upstream, i18n::t(lang, "push-set-upstream"));
            if let Some(err) = &self.push_error {
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::RED, err);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(i18n::t(lang, "btn-cancel")).clicked() {
                    self.close_push_dialog();
                }
                let can_push = !busy
                    && !no_remotes
                    && !self.push_branch.trim().is_empty()
                    && self.push_remotes_error.is_none();
                if ui
                    .add_enabled(
                        can_push,
                        egui::Button::new(format!(
                            "{} {}",
                            icons::ICON_PUSH,
                            i18n::t(lang, "btn-push-go")
                        )),
                    )
                    .clicked()
                {
                    self.submit_push_dialog();
                }
            });
        });
        // Closed via the window's X button.
        if !open {
            self.close_push_dialog();
        }
    }

    /// 对话框「推送」按钮：校验后走 busy 异步推送（结果进底部输出区），
    /// 并关闭对话框。
    fn submit_push_dialog(&mut self) {
        self.push_error = None;
        let Some(project) = self.selected_project().cloned() else {
            self.push_error = Some(i18n::t(self.lang, "err-no-project").into());
            return;
        };
        let Some(remote) = self.push_remotes.get(self.push_remote_sel).cloned() else {
            self.push_error = Some(i18n::t(self.lang, "err-select-remote").into());
            return;
        };
        let branch = self.push_branch.trim().to_string();
        if branch.is_empty() {
            self.push_error = Some(i18n::t(self.lang, "err-enter-branch").into());
            return;
        }
        let set_upstream = self.push_set_upstream;
        self.do_push_to(&project, &remote, &branch, set_upstream);
        self.close_push_dialog();
    }

    /// 推送对话框后台执行：与一键推送同一凭据注入路径，但凭据标签与
    /// `remote get-url` 换成所选远程名；https URL 注入与输出三重脱敏仍在
    /// 线程内完成。分支名由对话框给定，无需再 rev-parse。
    fn do_push_to(&mut self, project: &Project, remote: &str, branch: &str, set_upstream: bool) {
        if self.busy.is_some() {
            return;
        }
        let auth_parts = self.load_credential_parts(project.id, remote);
        self.busy = Some(BusyOp {
            project_id: project.id,
            label: tf(
                self.lang,
                "busy-push-to",
                &[("remote", remote), ("branch", branch)],
            ),
        });
        let tx = self.result_tx.clone();
        let path = PathBuf::from(&project.path);
        let project_id = project.id;
        let remote = remote.to_string();
        let branch = branch.to_string();
        let lang = self.lang;
        std::thread::spawn(move || {
            let Some((user, pass)) = auth_parts else {
                send_plain_push_to(&tx, project_id, &path, &remote, &branch, set_upstream);
                return;
            };
            let plain_url = match git::run(&path, &["remote", "get-url", &remote]) {
                Ok(out) if out.ok() => out.stdout.trim().to_string(),
                // git's own failure surfaces on the subsequent push call.
                Ok(_) => {
                    send_plain_push_to(&tx, project_id, &path, &remote, &branch, set_upstream);
                    return;
                }
                Err(e) => {
                    let _ = tx.send(GitResult {
                        project_id,
                        kind: ResultKind::RemoteUrl(remote.clone()),
                        output: Err(e),
                    });
                    send_plain_push_to(&tx, project_id, &path, &remote, &branch, set_upstream);
                    return;
                }
            };
            let Some(url) = inject_https_userinfo(&plain_url, &user, &pass) else {
                send_plain_push_to(&tx, project_id, &path, &remote, &branch, set_upstream);
                return;
            };
            let auth = InjectedAuth {
                url,
                plain_url,
                password: pass.clone(),
                encoded_password: percent_encode_userinfo(&pass),
            };
            let flag = if set_upstream { " -u" } else { "" };
            let label = format!(
                "git push{flag} {remote} {branch}{}",
                i18n::t(lang, "cred-injected")
            );
            let args: Vec<&str> = if set_upstream {
                vec!["push", "-u", &auth.url, &branch]
            } else {
                vec!["push", &auth.url, &branch]
            };
            let mut output = git::run(&path, &args)
                .map(|out| redact_output(out, &auth.redact_pairs()));
            // 注入凭据 + `-u`：git 推送成功会把含明文凭据的注入 URL 持久化
            // 写入 `.git/config` 的 `branch.<branch>.remote`；必须在 send
            // 结果之前用真实 remote 名覆盖，缩短凭据落盘窗口。
            if push_needs_upstream_restore(true, set_upstream) {
                output = restore_upstream_remote(output, &path, &remote, &branch, lang, &auth);
            }
            let _ = tx.send(GitResult { project_id, kind: ResultKind::PushTo(label), output });
        });
    }

    // ------------------------------------------------------- history window

    /// 操作面板「历史…」：打开只读提交历史窗口。同步取最近 10 条；关闭时
    /// 清空已加载列表，下次打开重新取。
    fn open_history_window(&mut self) {
        self.show_history_window = true;
        self.history_commits.clear();
        self.history_error = None;
        self.history_exhausted = false;
        let Some(project) = self.selected_project().cloned() else {
            return; // 按钮仅在选中项目后可见；防御性兜底
        };
        match git::log_recent(Path::new(&project.path), 0, HISTORY_FIRST_PAGE) {
            Ok(commits) => {
                if commits.len() < HISTORY_FIRST_PAGE {
                    self.history_exhausted = true;
                }
                self.history_commits = commits;
            }
            Err(e) => self.history_error = Some(e),
        }
    }

    fn close_history_window(&mut self) {
        self.show_history_window = false;
        self.history_commits.clear();
        self.history_error = None;
        self.history_exhausted = false;
    }

    /// 「加载更多」：skip = 已加载数，每次追加 20 条；返回不足一页即认为
    /// 已到底并禁用按钮。失败保留已加载内容，窗口内红字提示。
    fn history_load_more(&mut self) {
        if self.history_exhausted {
            return;
        }
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let skip = self.history_commits.len();
        match git::log_recent(Path::new(&project.path), skip, HISTORY_PAGE_SIZE) {
            Ok(mut commits) => {
                if commits.len() < HISTORY_PAGE_SIZE {
                    self.history_exhausted = true;
                }
                self.history_commits.append(&mut commits);
                self.history_error = None;
            }
            Err(e) => self.history_error = Some(e),
        }
    }

    fn show_history_window(&mut self, ctx: &egui::Context) {
        if !self.show_history_window {
            return;
        }
        let lang = self.lang;
        let Some(project_name) = self.selected_project().map(|p| p.name.clone()) else {
            self.show_history_window = false;
            return;
        };
        let exhausted = self.history_exhausted;
        let mut open = self.show_history_window;
        egui::Window::new(tf(
            lang,
            "history-title",
            &[("name", &project_name)],
        ))
        .collapsible(false)
        .default_width(560.0)
        .open(&mut open)
        .show(ctx, |ui| {
            if let Some(err) = &self.history_error {
                ui.colored_label(egui::Color32::RED, err);
                ui.add_space(4.0);
            }
            if self.history_commits.is_empty() {
                if self.history_error.is_none() {
                    ui.label(i18n::t(lang, "no-commits"));
                }
            } else {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(360.0)
                    .show(ui, |ui| {
                        egui::Grid::new("history_grid")
                            .striped(true)
                            .num_columns(4)
                            .spacing(egui::vec2(16.0, 4.0))
                            .max_col_width(360.0)
                            .show(ui, |ui| {
                                ui.strong(i18n::t(lang, "col-hash"));
                                ui.strong(i18n::t(lang, "col-author"));
                                ui.strong(i18n::t(lang, "col-date"));
                                ui.strong(i18n::t(lang, "col-subject"));
                                ui.end_row();
                                    for commit in &self.history_commits {
                                        let short = commit.hash.get(..8).unwrap_or(&commit.hash);
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(short).monospace(),
                                            )
                                            .wrap_mode(egui::TextWrapMode::Truncate),
                                        );
                                        ui.add(
                                            egui::Label::new(egui::RichText::new(&commit.author))
                                                .wrap_mode(egui::TextWrapMode::Truncate),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&commit.date).monospace(),
                                            )
                                            .wrap_mode(egui::TextWrapMode::Truncate),
                                        );
                                        ui.add(
                                            egui::Label::new(egui::RichText::new(&commit.subject))
                                                .wrap_mode(egui::TextWrapMode::Truncate),
                                        )
                                        .on_hover_text(&commit.subject);
                                        ui.end_row();
                                    }
                                });
                        });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!exhausted, egui::Button::new(i18n::t(lang, "btn-load-more")))
                        .clicked()
                    {
                        self.history_load_more();
                    }
                    if ui.button(i18n::t(lang, "btn-close")).clicked() {
                        self.close_history_window();
                    }
                });
            });
        // Closed via the window's X button.
        if !open {
            self.close_history_window();
        }
    }

    /// “新建分支”：后台线程先 `create_branch` 成功后 `checkout`，结果回填
    /// 清 busy 并刷新状态区。
    fn do_new_branch(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(project) = self.selected_project().cloned() else {
            return;
        };
        let name = self.new_branch.trim().to_string();
        if name.is_empty() {
            return; // the button is disabled; defensive only
        }
        self.busy = Some(BusyOp {
            project_id: project.id,
            label: tf(self.lang, "busy-new-branch", &[("name", &name)]),
        });
        let tx = self.result_tx.clone();
        let path = PathBuf::from(&project.path);
        let project_id = project.id;
        std::thread::spawn(move || {
            let create_label = format!("git branch {name}");
            let created = git::create_branch(&path, &name);
            let created_ok = created.as_ref().map(git::GitOutput::ok).unwrap_or(false);
            let _ = tx.send(GitResult {
                project_id,
                kind: ResultKind::BranchCreate(create_label),
                output: created,
            });
            if !created_ok {
                return;
            }
            let checkout_label = format!("git checkout {name}");
            let output = git::checkout(&path, &name);
            let _ = tx.send(GitResult {
                project_id,
                kind: ResultKind::BranchCheckout(checkout_label),
                output,
            });
        });
    }

    /// “打 tag”：有 message 则附注标签，否则轻量标签，结果回填清 busy 并
    /// 刷新状态区。
    fn do_tag(&mut self) {
        if self.busy.is_some() {
            return;
        }
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
            tf(self.lang, "label-tag-annotated", &[("name", &name)])
        } else {
            format!("git tag {name}")
        };
        self.busy = Some(BusyOp {
            project_id: project.id,
            label: tf(self.lang, "busy-tag", &[("name", &name)]),
        });
        let tx = self.result_tx.clone();
        let path = PathBuf::from(&project.path);
        let project_id = project.id;
        std::thread::spawn(move || {
            let output = git::create_tag(&path, &name, message.as_deref());
            let _ = tx.send(GitResult { project_id, kind: ResultKind::Tag(label), output });
        });
    }

    fn show_remove_confirm(&mut self, ctx: &egui::Context) {
        if !self.confirm_remove {
            return;
        }
        let lang = self.lang;
        let Some((name, id)) = self.selected_project().map(|p| (p.name.clone(), p.id)) else {
            self.confirm_remove = false;
            return;
        };
        let mut open = self.confirm_remove;
        egui::Window::new(i18n::t(lang, "btn-remove-project"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(tf(lang, "remove-confirm-body", &[("name", &name)]));
                ui.label(i18n::t(lang, "remove-confirm-note"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(i18n::t(lang, "btn-cancel")).clicked() {
                        self.confirm_remove = false;
                    }
                    if ui.button(i18n::t(lang, "btn-remove")).clicked() {
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
            self.error = Some(i18n::t(self.lang, "err-db-not-open").into());
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
            Err(e) => {
                self.error = Some(tf(self.lang, "err-list-projects", &[("err", &e.to_string())]))
            }
        }
    }

    fn pick_and_add_project(&mut self) {
        self.error = None;
        let Some(conn) = self.conn.as_ref() else {
            self.error = Some(
                self.open_error
                    .clone()
                    .unwrap_or_else(|| i18n::t(self.lang, "err-db-not-open").into()),
            );
            return;
        };
        let Some(dir) = rfd::FileDialog::new()
            .set_title(i18n::t(self.lang, "fd-select-repo"))
            .pick_folder()
        else {
            return; // user cancelled the dialog; not an error
        };
        let path = dir.to_string_lossy().into_owned();
        if !git::is_repo(&dir) {
            self.error = Some(tf(self.lang, "err-not-repo", &[("path", &path)]));
            return;
        }
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        match db::list_projects(conn) {
            Ok(projects) => {
                if projects.iter().any(|p| p.path == path) {
                    self.error = Some(tf(self.lang, "err-project-exists", &[("path", &path)]));
                    return;
                }
            }
            Err(e) => {
                self.error =
                    Some(tf(self.lang, "err-list-projects", &[("err", &e.to_string())]));
                return;
            }
        }
        match db::add_project(conn, &name, &path) {
            Ok(_) => self.reload_projects(),
            Err(e) => {
                self.error = Some(tf(self.lang, "err-add-project", &[("err", &e.to_string())]))
            }
        }
    }

    fn remove_selected(&mut self, id: i64) {
        self.confirm_remove = false;
        let Some(conn) = self.conn.as_ref() else {
            self.error = Some(i18n::t(self.lang, "err-db-not-open").into());
            return;
        };
        // Only the database row is deleted; the directory on disk is untouched.
        match db::remove_project(conn, id) {
            Ok(_) => {
                self.error = None;
                self.reload_projects();
            }
            Err(e) => {
                self.error =
                    Some(tf(self.lang, "err-remove-project", &[("err", &e.to_string())]));
            }
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
            .set_title(i18n::t(self.lang, "fd-select-clone-parent"))
            .pick_folder()
        {
            self.clone_parent = Some(dir);
        }
    }

    fn show_clone_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_clone_dialog {
            return;
        }
        let lang = self.lang;
        let mut open = self.show_clone_dialog;
        egui::Window::new(i18n::t(lang, "clone-dialog-title"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(i18n::t(lang, "clone-intro"));
                ui.add_space(6.0);
                ui.add_sized(
                    [DIALOG_FIELD_WIDTH, 22.0],
                    egui::TextEdit::singleline(&mut self.clone_url)
                        .hint_text(i18n::t(lang, "hint-clone-url")),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(i18n::t(lang, "btn-select-parent")).clicked() {
                        self.pick_clone_parent();
                    }
                    match &self.clone_parent {
                        Some(dir) => ui.monospace(dir.display().to_string()),
                        None => ui.label(i18n::t(lang, "no-parent-chosen")),
                    }
                });
                ui.add_space(6.0);
                ui.add_sized(
                    [DIALOG_FIELD_WIDTH, 22.0],
                    egui::TextEdit::singleline(&mut self.clone_subdir)
                        .hint_text(i18n::t(lang, "hint-clone-subdir")),
                );
                if let Some(err) = &self.clone_error {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::RED, err);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(i18n::t(lang, "btn-cancel")).clicked() {
                        self.close_clone_dialog();
                    }
                    let ready = !self.clone_url.trim().is_empty()
                        && self.clone_parent.is_some()
                        && !self.clone_subdir.trim().is_empty();
                    if ui
                        .add_enabled(ready, egui::Button::new(i18n::t(lang, "btn-clone")))
                        .clicked()
                    {
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
            self.clone_error = Some(i18n::t(self.lang, "err-clone-fields").into());
            return;
        }
        if subdir.contains('/') || subdir.contains('\\') || subdir == "." || subdir == ".." {
            self.clone_error = Some(i18n::t(self.lang, "err-clone-subdir-sep").into());
            return;
        }
        let Some(parent) = self.clone_parent.clone() else {
            self.clone_error = Some(i18n::t(self.lang, "err-clone-no-parent").into());
            return;
        };
        let target = parent.join(&subdir);
        let target_str = target.to_string_lossy().into_owned();

        // 查重：目标目录不存在，且不在项目列表中。
        if target.exists() {
            self.clone_error =
                Some(tf(self.lang, "err-clone-target-exists", &[("path", &target_str)]));
            return;
        }
        {
            let Some(conn) = self.conn.as_ref() else {
                self.clone_error = Some(i18n::t(self.lang, "err-db-not-open").into());
                return;
            };
            match db::list_projects(conn) {
                Ok(projects) => {
                    if projects.iter().any(|p| p.path == target_str) {
                        self.clone_error = Some(tf(
                            self.lang,
                            "err-project-exists",
                            &[("path", &target_str)],
                        ));
                        return;
                    }
                }
                Err(e) => {
                    self.clone_error =
                        Some(tf(self.lang, "err-list-projects", &[("err", &e.to_string())]));
                    return;
                }
            }
        }

        let label = format!("git clone {url} {target_str}");
        match git::clone_repo(&url, &target) {
            Err(e) => {
                self.log_git_error(&label, &e);
                self.clone_error =
                    Some(tf(self.lang, "err-git-run", &[("err", &e.to_string())]));
            }
            Ok(out) => {
                self.log_git_output(&label, &out);
                if !out.ok() {
                    let detail = git_output_text(self.lang, &out);
                    self.clone_error = Some(tf(
                        self.lang,
                        "err-clone-failed",
                        &[("code", &out.code.to_string()), ("detail", &detail)],
                    ));
                    return;
                }
                let name = target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| subdir.clone());
                let Some(conn) = self.conn.as_ref() else {
                    self.clone_error = Some(i18n::t(self.lang, "err-db-not-open").into());
                    return;
                };
                match db::add_project(conn, &name, &target_str) {
                    Ok(_) => self.close_clone_dialog(),
                    Err(e) => {
                        self.clone_error =
                            Some(tf(self.lang, "err-add-project", &[("err", &e.to_string())]))
                    }
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
        let lang = self.lang;
        let Some(project_name) = self.selected_project().map(|p| p.name.clone()) else {
            self.show_cred_dialog = false;
            return;
        };
        let mut open = self.show_cred_dialog;
        egui::Window::new(tf(
            lang,
            "cred-dialog-title",
            &[("name", &project_name)],
        ))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(i18n::t(lang, "cred-intro"));
            ui.add_space(6.0);
            ui.add_sized(
                [DIALOG_FIELD_WIDTH, 22.0],
                egui::TextEdit::singleline(&mut self.cred_label)
                    .hint_text(i18n::t(lang, "hint-cred-label")),
            );
            ui.add_space(6.0);
            ui.add_sized(
                [DIALOG_FIELD_WIDTH, 22.0],
                egui::TextEdit::singleline(&mut self.cred_user)
                    .hint_text(i18n::t(lang, "hint-username")),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [DIALOG_FIELD_WIDTH - 80.0, 22.0],
                    egui::TextEdit::singleline(&mut self.cred_pass)
                        .password(true)
                        .hint_text(i18n::t(lang, "hint-password")),
                );
                if ui.button(i18n::t(lang, "btn-generate")).clicked() {
                    self.open_gen_dialog();
                }
            });
            if let Some(err) = &self.cred_error {
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::RED, err);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(i18n::t(lang, "btn-close")).clicked() {
                    self.close_cred_dialog();
                }
                if ui.button(i18n::t(lang, "btn-delete-credential")).clicked() {
                    self.delete_credential();
                }
                if ui.button(i18n::t(lang, "btn-save")).clicked() {
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
            self.cred_error = Some(i18n::t(self.lang, "err-cred-fields").into());
            return;
        }
        let Some(project_id) = self.selected_project().map(|p| p.id) else {
            self.cred_error = Some(i18n::t(self.lang, "err-no-project").into());
            return;
        };
        let Some(keys) = self.keys.as_ref() else {
            self.cred_error = Some(i18n::t(self.lang, "err-vault-locked").into());
            return;
        };
        let Some(conn) = self.conn.as_ref() else {
            self.cred_error = Some(i18n::t(self.lang, "err-db-not-open").into());
            return;
        };
        let secret = format!("{user}:{pass}");
        match db::store_credential(conn, keys, project_id, &label, &secret) {
            Ok(()) => self.close_cred_dialog(),
            Err(e) => {
                self.cred_error =
                    Some(tf(self.lang, "err-save-credential", &[("err", &e.to_string())]))
            }
        }
    }

    /// 删除按钮：`delete_credential` 删除当前标签下的凭据。
    fn delete_credential(&mut self) {
        self.cred_error = None;
        let label = self.cred_label.trim().to_string();
        if label.is_empty() {
            self.cred_error = Some(i18n::t(self.lang, "err-cred-label-required").into());
            return;
        }
        let Some(project_id) = self.selected_project().map(|p| p.id) else {
            self.cred_error = Some(i18n::t(self.lang, "err-no-project").into());
            return;
        };
        let Some(conn) = self.conn.as_ref() else {
            self.cred_error = Some(i18n::t(self.lang, "err-db-not-open").into());
            return;
        };
        match db::delete_credential(conn, project_id, &label) {
            Ok(_) => self.close_cred_dialog(),
            Err(e) => {
                self.cred_error = Some(tf(
                    self.lang,
                    "err-delete-credential",
                    &[("err", &e.to_string())],
                ))
            }
        }
    }

    // -------------------------------------------------- master password change

    /// 菜单「设置 → 修改主密码」：打开对话框（空白表单）。
    fn open_pw_change_dialog(&mut self) {
        self.error = None;
        self.pw_old.clear();
        self.pw_new.clear();
        self.pw_new2.clear();
        self.pw_error = None;
        self.show_pw_change = true;
    }

    /// 关闭对话框并清空全部密码输入（取消按钮、X 按钮与成功后都走这里）。
    fn close_pw_change_dialog(&mut self) {
        self.show_pw_change = false;
        self.pw_old.clear();
        self.pw_new.clear();
        self.pw_new2.clear();
        self.pw_error = None;
    }

    fn show_pw_change_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_pw_change {
            return;
        }
        let lang = self.lang;
        let mut open = self.show_pw_change;
        egui::Window::new(i18n::t(lang, "pw-dialog-title"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(i18n::t(lang, "pw-intro"));
                ui.add_space(6.0);
                ui.add_sized(
                    [DIALOG_FIELD_WIDTH, 22.0],
                    egui::TextEdit::singleline(&mut self.pw_old)
                        .password(true)
                        .hint_text(i18n::t(lang, "hint-old-master-password")),
                );
                ui.add_space(6.0);
                ui.add_sized(
                    [DIALOG_FIELD_WIDTH, 22.0],
                    egui::TextEdit::singleline(&mut self.pw_new)
                        .password(true)
                        .hint_text(i18n::t(lang, "hint-new-master-password")),
                );
                ui.add_space(6.0);
                let confirm = ui.add_sized(
                    [DIALOG_FIELD_WIDTH, 22.0],
                    egui::TextEdit::singleline(&mut self.pw_new2)
                        .password(true)
                        .hint_text(i18n::t(lang, "hint-repeat-new-master-password")),
                );
                let pressed_enter =
                    confirm.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if let Some(err) = &self.pw_error {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::RED, err);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(i18n::t(lang, "btn-cancel")).clicked() {
                        self.close_pw_change_dialog();
                    }
                    if ui.button(i18n::t(lang, "btn-change")).clicked() || pressed_enter {
                        self.submit_master_password_change();
                    }
                });
            });
        // Closed via the window's X button.
        if !open {
            self.close_pw_change_dialog();
        }
    }

    /// 「修改」按钮：校验三框非空且新==确认后，同步调用
    /// `db::change_master_password`（操作秒级，不进 busy 通道）。成功后用新
    /// 主密码重新解锁以刷新会话密钥（盐已轮换，旧密钥不再可用），并清空、
    /// 关窗、在输出区提示；失败则在窗口内红色显示原因，保留输入。
    fn submit_master_password_change(&mut self) {
        self.pw_error = None;
        if self.pw_old.is_empty() || self.pw_new.is_empty() || self.pw_new2.is_empty() {
            self.pw_error = Some(i18n::t(self.lang, "err-pw-fields").into());
            return;
        }
        if self.pw_new != self.pw_new2 {
            self.pw_error = Some(i18n::t(self.lang, "err-pw-mismatch").into());
            self.pw_new2.clear();
            return;
        }
        let Some(conn) = self.conn.as_ref() else {
            self.pw_error = Some(
                self.open_error
                    .clone()
                    .unwrap_or_else(|| i18n::t(self.lang, "err-db-not-open").into()),
            );
            return;
        };
        match db::change_master_password(conn, &self.pw_old, &self.pw_new) {
            Ok(()) => {
                // 盐已轮换：内存中的旧会话密钥不再能解密新密文，必须用新
                // 主密码重新解锁。
                match crypto::unlock(conn, &self.pw_new) {
                    Ok(keys) => {
                        self.keys = Some(keys);
                        self.log.push(LogEntry {
                            ok: true,
                            title: i18n::t(self.lang, "log-master-changed").to_string(),
                            text: i18n::t(self.lang, "log-master-changed-body").to_string(),
                        });
                        self.trim_log();
                        self.close_pw_change_dialog();
                    }
                    Err(e) => {
                        // 理论上不可达（change_master_password 刚验证过新
                        // 密码）；旧会话密钥已失效，不能继续沿用。
                        self.keys = None;
                        self.log.push(LogEntry {
                            ok: false,
                            title: i18n::t(self.lang, "log-master-changed-key-fail").to_string(),
                            text: tf(
                                self.lang,
                                "log-restart-needed",
                                &[("err", &e.to_string())],
                            ),
                        });
                        self.trim_log();
                        self.close_pw_change_dialog();
                    }
                }
            }
            Err(e) => {
                self.pw_error =
                    Some(tf(self.lang, "err-change-master-password", &[("err", &e.to_string())]))
            }
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
        self.gen_error = None;
        self.gen_result.clear();
    }

    fn show_gen_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_gen_dialog {
            return;
        }
        let lang = self.lang;
        let mut open = self.show_gen_dialog;
        egui::Window::new(i18n::t(lang, "gen-title"))
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
        let lang = self.lang;
        let mut open = self.show_tool_gen_window;
        egui::Window::new(i18n::t(lang, "gen-title"))
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
    /// 特殊字符可选、生成 / 复制 / 关闭；结果默认明文显示（可选中复制），
    /// 可勾选「遮蔽显示」切换为 `•` 等宽掩码；仅凭据对话框内额外提供
    /// 「填入密码框」。
    fn show_generator_ui(&mut self, ui: &mut egui::Ui, window: GenWindow) {
        let lang = self.lang;
        ui.add(
            egui::Slider::new(&mut self.gen_opts.length, 8..=64)
                .text(i18n::t(lang, "gen-length")),
        );
        // upper/lower/digits 固定启用（生成器要求至少一类字符）。
        let mut fixed_upper = true;
        ui.add_enabled(
            false,
            egui::Checkbox::new(&mut fixed_upper, i18n::t(lang, "gen-upper-fixed")),
        );
        let mut fixed_lower = true;
        ui.add_enabled(
            false,
            egui::Checkbox::new(&mut fixed_lower, i18n::t(lang, "gen-lower-fixed")),
        );
        let mut fixed_digits = true;
        ui.add_enabled(
            false,
            egui::Checkbox::new(&mut fixed_digits, i18n::t(lang, "gen-digits-fixed")),
        );
        ui.checkbox(&mut self.gen_opts.special, i18n::t(lang, "gen-special"));
        if ui.button(i18n::t(lang, "btn-generate-plain")).clicked() {
            self.generate_password();
        }
        // 明文（默认）用可编辑 TextEdit 显示（可选中复制）；遮蔽时以 `•`
        // 重复等宽显示，长度与明文一致。
        ui.checkbox(&mut self.gen_masked, i18n::t(lang, "gen-masked"));
        if self.gen_masked {
            let dots = "•".repeat(self.gen_result.chars().count());
            if dots.is_empty() {
                ui.label(i18n::t(lang, "gen-result-empty"));
            } else {
                ui.add_sized(
                    [DIALOG_FIELD_WIDTH, 22.0],
                    egui::Label::new(egui::RichText::new(dots).monospace()),
                );
            }
        } else {
            ui.add_sized(
                [DIALOG_FIELD_WIDTH, 22.0],
                egui::TextEdit::singleline(&mut self.gen_result)
                    .hint_text(i18n::t(lang, "gen-result-hint")),
            );
        }
        if let Some(err) = &self.gen_error {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::RED, err);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let has_result = !self.gen_result.is_empty();
            if ui
                .add_enabled(has_result, egui::Button::new(i18n::t(lang, "btn-copy")))
                .clicked()
            {
                ui.ctx().copy_text(self.gen_result.clone());
            }
            if window == GenWindow::CredDialog
                && ui
                    .add_enabled(
                        has_result,
                        egui::Button::new(i18n::t(lang, "btn-fill-password")),
                    )
                    .clicked()
            {
                self.cred_pass = std::mem::take(&mut self.gen_result);
                self.close_generator(window);
            }
            if ui.button(i18n::t(lang, "btn-close")).clicked() {
                self.close_generator(window);
            }
        });
    }

    /// 生成按钮：`password::generate`，结果只保存在内存中。
    fn generate_password(&mut self) {
        self.gen_error = None;
        match password::generate(&self.gen_opts) {
            Ok(pw) => self.gen_result = pw,
            Err(e) => {
                self.gen_error =
                    Some(tf(self.lang, "err-generate", &[("err", &e.to_string())]))
            }
        }
    }

    // --------------------------------------------------------------- shared

    /// Apply the persisted theme every frame; a cheap no-op once applied, and
    /// it re-asserts the choice if anything else resets the style.
    fn apply_theme(&self, ctx: &egui::Context) {
        let visuals = custom_visuals(self.theme);
        if ctx.style().visuals != visuals {
            let mut style = (*ctx.style()).clone();
            style.visuals = visuals;
            // 统一间距与控件内边距，让界面呼吸感更好（间距只随主题
            // 一起设置一次；主题不变时不重复写 style）。
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(8.0, 4.0);
            style.spacing.interact_size.y = 24.0;
            ctx.set_style(style);
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
    use tempfile::tempdir;

    #[test]
    fn should_apply_matches_selected_project() {
        // Result belongs to the currently selected project: apply it.
        assert!(should_apply(7, Some(7)));
    }

    #[test]
    fn should_apply_rejects_other_project() {
        // A stale result from a previously selected project must not touch
        // the on-screen status area (the cache is still updated by the caller).
        assert!(!should_apply(7, Some(8)));
        // No project selected at all: nothing may be applied to the display.
        assert!(!should_apply(7, None));
    }

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

    #[test]
    fn redact_output_scrubs_stdout_and_stderr() {
        // The worker thread must scrub stdout/stderr before sending results
        // back: no credential material may cross to the UI thread.
        let auth = InjectedAuth {
            url: "https://u:s3cret@host/repo.git".to_string(),
            plain_url: "https://host/repo.git".to_string(),
            password: "s3cret".to_string(),
            encoded_password: "s3cret".to_string(),
        };
        let out = git::GitOutput {
            code: 128,
            stdout: "From https://u:s3cret@host/repo.git".to_string(),
            stderr: "fatal: authentication failed for 'https://u:s3cret@host/repo.git'".to_string(),
        };
        let scrubbed = redact_output(out, &auth.redact_pairs());
        assert_eq!(scrubbed.stdout, "From https://host/repo.git");
        assert_eq!(
            scrubbed.stderr,
            "fatal: authentication failed for 'https://host/repo.git'"
        );
        assert!(!scrubbed.stdout.contains("s3cret"));
        assert!(!scrubbed.stderr.contains("s3cret"));
    }

    // ------------------------------------------------ project tree structure

    fn paths(value: &[&str]) -> Vec<String> {
        value.iter().map(|s| s.to_string()).collect()
    }

    /// 沿展示名链找到节点（测试辅助）。
    fn node_at<'a>(nodes: &'a [DirNode], segs: &[&str]) -> &'a DirNode {
        assert!(!segs.is_empty(), "node_at needs at least one segment");
        let mut cur: &[DirNode] = nodes;
        for (i, seg) in segs.iter().enumerate() {
            let node = cur
                .iter()
                .find(|n| n.label == *seg)
                .unwrap_or_else(|| panic!("missing tree segment {seg:?}"));
            if i + 1 == segs.len() {
                return node;
            }
            cur = &node.dirs;
        }
        unreachable!()
    }

    fn all_projects(nodes: &[DirNode], out: &mut Vec<usize>) {
        for node in nodes {
            out.extend(node.projects.iter().copied());
            all_projects(&node.dirs, out);
        }
    }

    #[test]
    fn tree_expands_each_path_segment_into_a_level() {
        // 同一父目录下的项目挂在同一条目录链末尾。
        let tree = build_project_tree(
            &paths(&["E:\\ws\\a\\repo1", "E:\\ws\\a\\repo2", "E:\\ws\\a\\repo3"]),
            true,
        );
        assert_eq!(tree.len(), 1);
        let node = node_at(&tree, &["E:", "ws", "a"]);
        assert_eq!(node.projects, vec![0, 1, 2]);
    }

    #[test]
    fn tree_nests_deep_paths() {
        // 深层路径逐层展开：E: → ws → deep → nest，项目挂在 nest 下。
        let tree = build_project_tree(&paths(&["E:\\ws\\deep\\nest\\repo"]), true);
        let nest = node_at(&tree, &["E:", "ws", "deep", "nest"]);
        assert_eq!(nest.projects, vec![0]);
        // 未在路径上出现的中间层不会被凭空创建。
        assert!(node_at(&tree, &["E:", "ws"]).projects.is_empty());
    }

    #[test]
    fn tree_keeps_sibling_directories_separate_and_sorted() {
        // 无前缀关系的兄弟父目录各自成节点；子目录按名排序。
        let tree =
            build_project_tree(&paths(&["E:\\ws\\beta\\r2", "E:\\ws\\alpha\\r1"]), true);
        let ws = node_at(&tree, &["E:", "ws"]);
        let labels: Vec<&str> = ws.dirs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, vec!["alpha", "beta"]);
        assert_eq!(node_at(&tree, &["E:", "ws", "alpha"]).projects, vec![1]);
        assert_eq!(node_at(&tree, &["E:", "ws", "beta"]).projects, vec![0]);
    }

    #[test]
    fn tree_different_drives_form_separate_roots() {
        // 不同盘符是不同根节点，按名排序。
        let tree = build_project_tree(&paths(&["C:\\data\\r1", "D:\\stuff\\r2"]), true);
        let roots: Vec<&str> = tree.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(roots, vec!["C:", "D:"]);
        assert_eq!(node_at(&tree, &["C:", "data"]).projects, vec![0]);
        assert_eq!(node_at(&tree, &["D:", "stuff"]).projects, vec![1]);
    }

    #[test]
    fn tree_mixed_realistic_layout() {
        // 真实布局：多个盘符与多层目录并存，各项目挂在对应层。
        let tree = build_project_tree(
            &paths(&[
                "E:\\Workspace\\opencode\\git-gui",
                "E:\\Workspace\\opencode\\tools\\deploy",
                "E:\\Workspace\\rust\\cargo-scan",
                "C:\\Users\\me\\notes",
            ]),
            true,
        );
        let roots: Vec<&str> = tree.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(roots, vec!["C:", "E:"]);
        assert_eq!(node_at(&tree, &["C:", "Users", "me"]).projects, vec![3]);
        assert_eq!(
            node_at(&tree, &["E:", "Workspace", "opencode"]).projects,
            vec![0]
        );
        assert_eq!(
            node_at(&tree, &["E:", "Workspace", "opencode", "tools"]).projects,
            vec![1]
        );
        assert_eq!(
            node_at(&tree, &["E:", "Workspace", "rust"]).projects,
            vec![2]
        );
    }

    #[test]
    fn tree_is_case_insensitive_in_windows_mode() {
        // Windows 路径大小写不敏感：仅大小写不同的目录合并到同一节点，
        // 展示名取首次出现的拼写。
        let tree = build_project_tree(
            &paths(&["E:\\Workspace\\opencode\\r1", "e:\\workspace\\OPENCODE\\r2"]),
            true,
        );
        assert_eq!(tree.len(), 1);
        let node = node_at(&tree, &["E:", "Workspace", "opencode"]);
        assert_eq!(node.projects, vec![0, 1]);
    }

    #[test]
    fn tree_is_case_sensitive_outside_windows_mode() {
        // POSIX 模式（case_insensitive=false）：大小写不同的目录分开。
        let tree =
            build_project_tree(&paths(&["/home/u/Code/r1", "/home/u/code/r2"]), false);
        let u = node_at(&tree, &["/", "home", "u"]);
        let labels: Vec<&str> = u.dirs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, vec!["Code", "code"]);
        assert_eq!(node_at(&tree, &["/", "home", "u", "Code"]).projects, vec![0]);
        assert_eq!(node_at(&tree, &["/", "home", "u", "code"]).projects, vec![1]);
    }

    #[test]
    fn tree_normalizes_separators_in_windows_mode() {
        // Windows 模式下 `/` 与 `\` 视为同一分隔符（克隆产生的路径可能是
        // 正斜杠），同一目录不因分隔符写法不同而拆成两个节点。
        let tree = build_project_tree(&paths(&["E:\\ws\\r1", "E:/ws/r2"]), true);
        let node = node_at(&tree, &["E:", "ws"]);
        assert_eq!(node.projects, vec![0, 1]);
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn tree_handles_ungroupable_paths() {
        // 空路径 / 纯根路径 / 单段相对路径推导不出父目录：归入单一的
        // 未分组节点（label 为稳定 key，排在最后），其余正常成树。
        let tree = build_project_tree(
            &paths(&["", "C:\\", "relative-repo", "E:\\ws\\normal"]),
            true,
        );
        assert_eq!(tree.len(), 2);
        assert_eq!(node_at(&tree, &["E:", "ws"]).projects, vec![3]);
        let ungrouped = tree.last().expect("ungrouped node");
        assert_eq!(ungrouped.label, GROUP_UNGROUPED_KEY);
        assert_eq!(ungrouped.key, "");
        assert_eq!(ungrouped.projects, vec![0, 1, 2]);
    }

    #[test]
    fn tree_member_indices_cover_every_input_exactly_once() {
        // 契约：每个输入下标恰好出现在一个节点里，同节点内保持输入顺序。
        let input = paths(&[
            "E:\\ws\\a\\r1",
            "D:\\r2",
            "E:\\ws\\a\\b\\r3",
            "E:\\ws\\a\\r4",
            "D:\\deep\\r5",
        ]);
        let tree = build_project_tree(&input, true);
        let mut seen: Vec<usize> = Vec::new();
        all_projects(&tree, &mut seen);
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);
    }

    // ------------------------------------------------ porcelain parsing

    #[test]
    fn format_worktree_status_translates_entries() {
        let text = format_worktree_status(
            Lang::Zh,
            "## main...origin/main\n M a.txt\n?? b.txt\nUU c.txt\n",
        );
        assert!(text.contains("分支：main...origin/main"));
        assert!(text.contains("[已修改] a.txt"));
        assert!(text.contains("[新增/未跟踪] b.txt"));
        assert!(text.contains("[冲突] c.txt"));
        assert!(!text.contains("?? b.txt"));
    }

    #[test]
    fn format_worktree_status_clean_and_empty() {
        let text = format_worktree_status(Lang::Zh, "## main\n");
        assert!(text.contains("工作区干净"));
        assert!(format_worktree_status(Lang::En, "").contains("clean"));
    }

    #[test]
    fn status_from_xy_maps_pairs() {
        assert_eq!(status_from_xy(b'M', b' '), FileStatus::Modified);
        assert_eq!(status_from_xy(b'M', b'M'), FileStatus::Modified);
        assert_eq!(status_from_xy(b' ', b'T'), FileStatus::Modified);
        assert_eq!(status_from_xy(b'?', b'?'), FileStatus::Added);
        assert_eq!(status_from_xy(b'A', b' '), FileStatus::Added);
        assert_eq!(status_from_xy(b' ', b'D'), FileStatus::Deleted);
        assert_eq!(status_from_xy(b'R', b' '), FileStatus::Renamed);
        assert_eq!(status_from_xy(b'C', b' '), FileStatus::Renamed);
        // 同一 XY 内按 冲突 > 删除 > 修改 > 重命名 > 新增 取舍。
        assert_eq!(status_from_xy(b'A', b'M'), FileStatus::Modified);
        assert_eq!(status_from_xy(b'A', b'D'), FileStatus::Deleted);
        assert_eq!(status_from_xy(b'R', b'M'), FileStatus::Modified);
        for (x, y) in [
            (b'U', b'U'),
            (b'A', b'A'),
            (b'D', b'D'),
            (b'A', b'U'),
            (b'U', b'A'),
            (b'D', b'U'),
            (b'U', b'D'),
        ] {
            assert_eq!(status_from_xy(x, y), FileStatus::Conflict, "{x}{y}");
        }
    }

    #[test]
    fn parse_status_map_basic() {
        let map = parse_status_map(
            "## main...origin/main [ahead 1]\n M a.txt\n?? b.txt\nD  c.txt\nMM d.txt\n T e.txt\nR  f.txt -> g.txt\n",
        );
        assert_eq!(map.get("a.txt"), Some(&FileStatus::Modified));
        assert_eq!(map.get("b.txt"), Some(&FileStatus::Added));
        assert_eq!(map.get("c.txt"), Some(&FileStatus::Deleted));
        assert_eq!(map.get("d.txt"), Some(&FileStatus::Modified));
        assert_eq!(map.get("e.txt"), Some(&FileStatus::Modified));
        // rename 行取新路径，旧路径不出现。
        assert_eq!(map.get("g.txt"), Some(&FileStatus::Renamed));
        assert!(!map.contains_key("f.txt"));
        assert_eq!(map.len(), 6);
    }

    #[test]
    fn parse_status_map_unquotes_paths() {
        // 含空格路径被引号包裹（实测 porcelain 对含空格路径恒加引号）。
        let map = parse_status_map(" M \"with space.txt\"\n");
        assert_eq!(map.get("with space.txt"), Some(&FileStatus::Modified));
        // UTF-8 八进制转义（core.quotePath 默认对非 ASCII 字节转义）。
        let map = parse_status_map("?? \"\\346\\226\\207\\346\\241\\243.txt\"\n");
        assert_eq!(map.get("文档.txt"), Some(&FileStatus::Added));
        // 转义的引号与反斜杠。
        let map = parse_status_map("?? \"quo\\\"te\\\\dir.txt\"\n");
        assert_eq!(map.get("quo\"te\\dir.txt"), Some(&FileStatus::Added));
    }

    #[test]
    fn parse_status_map_rename_with_quoted_sides() {
        // 实测格式：rename 两侧各自独立加引号。
        let map = parse_status_map("R  \"src dir/a.txt\" -> \"src dir/b moved.txt\"\n");
        assert_eq!(map.get("src dir/b moved.txt"), Some(&FileStatus::Renamed));
        assert!(!map.contains_key("src dir/a.txt"));
        // 仅新路径一侧加引号。
        let map = parse_status_map("R  old.txt -> \"new dir/b.txt\"\n");
        assert_eq!(map.get("new dir/b.txt"), Some(&FileStatus::Renamed));
        // RM（重命名暂存 + 工作区又改）：XY 内 修改 > 重命名 → M 标记，
        // 但路径仍取 `->` 右侧。
        let map = parse_status_map("RM \"src dir/a.txt\" -> \"src dir/b moved.txt\"\n");
        assert_eq!(map.get("src dir/b moved.txt"), Some(&FileStatus::Modified));
    }

    #[test]
    fn parse_status_map_priority_across_lines() {
        // 同一路径两行：删除 > 新增（两种行序结果一致）。
        let map = parse_status_map("D  x.txt\n?? x.txt\n");
        assert_eq!(map.get("x.txt"), Some(&FileStatus::Deleted));
        let map = parse_status_map("?? x.txt\nD  x.txt\n");
        assert_eq!(map.get("x.txt"), Some(&FileStatus::Deleted));
        // 新增 + 修改 → 修改。
        let map = parse_status_map("A  y.txt\n M y.txt\n");
        assert_eq!(map.get("y.txt"), Some(&FileStatus::Modified));
        // 冲突优先于一切。
        let map = parse_status_map(" M z.txt\nUU z.txt\n");
        assert_eq!(map.get("z.txt"), Some(&FileStatus::Conflict));
    }

    #[test]
    fn parse_status_map_untracked_dir_strips_slash() {
        let map = parse_status_map("?? newdir/\n");
        assert_eq!(map.get("newdir"), Some(&FileStatus::Added));
        assert!(!map.contains_key("newdir/"));
    }

    #[test]
    fn parse_status_map_skips_headers_and_garbage() {
        let map = parse_status_map("## HEAD (no branch)\nUU both.txt\n\nshort\n!!!!\n#");
        assert_eq!(map.get("both.txt"), Some(&FileStatus::Conflict));
        assert_eq!(map.len(), 1);
        // 空输入 → 空映射。
        assert!(parse_status_map("").is_empty());
        assert!(parse_status_map("## main\n").is_empty());
    }

    #[test]
    fn status_for_entry_exact_and_dir_prefix() {
        let map = parse_status_map(" M src/main.rs\nD  src/lib/dead.rs\n?? other.txt\n");
        // 文件精确匹配。
        assert_eq!(
            status_for_entry(&map, "src/main.rs", false),
            Some(FileStatus::Modified)
        );
        // 目录取后代最高优先级：删除 > 修改。
        assert_eq!(status_for_entry(&map, "src", true), Some(FileStatus::Deleted));
        // 前缀匹配不误伤同级名字（src2 ≠ src/…）。
        assert_eq!(status_for_entry(&map, "src2", true), None);
        // 无变更的目录 → None；文件无精确匹配 → None。
        assert_eq!(status_for_entry(&map, "empty", true), None);
        assert_eq!(status_for_entry(&map, "other", false), None);
        // 未跟踪目录（尾部 / 已剥离）可直接匹配。
        let map = parse_status_map("?? newdir/\n");
        assert_eq!(
            status_for_entry(&map, "newdir", true),
            Some(FileStatus::Added)
        );
    }

    #[test]
    fn current_branch_from_porcelain_cases() {
        assert_eq!(
            current_branch_from_porcelain("## master...origin/master [ahead 1]\n M a\n"),
            Some("master".to_string())
        );
        assert_eq!(
            current_branch_from_porcelain("## dev\n"),
            Some("dev".to_string())
        );
        assert_eq!(
            current_branch_from_porcelain("## No commits yet on main\n?? x\n"),
            Some("main".to_string())
        );
        assert_eq!(
            current_branch_from_porcelain("## Initial commit on master\n"),
            Some("master".to_string())
        );
        // 分离 HEAD / 空输出 / 无头行 → None。
        assert_eq!(current_branch_from_porcelain("## HEAD (no branch)\n"), None);
        assert_eq!(current_branch_from_porcelain(""), None);
        assert_eq!(current_branch_from_porcelain(" M a.txt\n"), None);
    }

    #[test]
    fn file_status_ranks_and_markers() {
        // 优先级全序：冲突 > 删除 > 修改 > 重命名 > 新增。
        assert!(FileStatus::Conflict.rank() > FileStatus::Deleted.rank());
        assert!(FileStatus::Deleted.rank() > FileStatus::Modified.rank());
        assert!(FileStatus::Modified.rank() > FileStatus::Renamed.rank());
        assert!(FileStatus::Renamed.rank() > FileStatus::Added.rank());
        // 标记字符与图例一一对应。
        assert_eq!(FileStatus::Modified.marker(), "M");
        assert_eq!(FileStatus::Added.marker(), "+");
        assert_eq!(FileStatus::Deleted.marker(), "−");
        assert_eq!(FileStatus::Conflict.marker(), "!");
        assert_eq!(FileStatus::Renamed.marker(), "R");
        assert!(!status_legend_text(Lang::Zh, FileStatus::Added).is_empty());
    }

    // -------------------------------------- injected -u push upstream restore

    #[test]
    fn push_needs_upstream_restore_only_for_injected_upstream() {
        // 注入凭据 + `-u`：git 会把含明文凭据的 URL 持久化写入
        // .git/config 的 branch.<branch>.remote → 推送成功后必须回写。
        assert!(push_needs_upstream_restore(true, true));
        // 其余组合均无凭据落盘问题：不回写。
        assert!(!push_needs_upstream_restore(true, false));
        assert!(!push_needs_upstream_restore(false, true));
        assert!(!push_needs_upstream_restore(false, false));
    }

    #[test]
    fn restore_upstream_remote_overwrites_config_with_real_remote() {
        // 集成：模拟注入 -u 推送后的落盘状态（branch.main.remote 为含凭据
        // 的 URL），回写后必须变回真实 remote 名，且输出中无凭据。
        let guard = tempdir().expect("tempdir");
        let repo = guard.path().to_path_buf();
        let init = git::run(&repo, &["init"]).expect("git init in tempdir");
        assert!(init.ok(), "git init failed: {}", init.stderr);
        let set = git::run(
            &repo,
            &["config", "branch.main.remote", "https://u:s3cret@host/repo.git"],
        )
        .expect("git config set");
        assert!(set.ok(), "git config failed: {}", set.stderr);

        let auth = InjectedAuth {
            url: "https://u:s3cret@host/repo.git".to_string(),
            plain_url: "https://host/repo.git".to_string(),
            password: "s3cret".to_string(),
            encoded_password: "s3cret".to_string(),
        };
        let push_out = git::GitOutput { code: 0, stdout: String::new(), stderr: String::new() };
        let out = restore_upstream_remote(Ok(push_out), &repo, "origin", "main", Lang::Zh, &auth)
            .expect("restore returns the push output");
        assert!(out.ok(), "回写成功不得改判推送结果");
        assert!(!out.stdout.contains("s3cret"), "输出不得含凭据");
        assert!(!out.stderr.contains("s3cret"), "输出不得含凭据");

        let check = git::run(&repo, &["config", "--get", "branch.main.remote"])
            .expect("git config get");
        assert!(check.ok(), "git config --get failed: {}", check.stderr);
        assert_eq!(check.stdout.trim(), "origin", "必须覆盖为真实 remote 名");
    }

    #[test]
    fn restore_upstream_failure_keeps_push_ok_and_warns() {
        // 回写失败（git 无法运行，如目录不存在）：推送仍记为成功，
        // 仅追加警告文案；输出不得含凭据。
        let auth = InjectedAuth {
            url: "https://u:s3cret@host/repo.git".to_string(),
            plain_url: "https://host/repo.git".to_string(),
            password: "s3cret".to_string(),
            encoded_password: "s3cret".to_string(),
        };
        let push_out = git::GitOutput {
            code: 0,
            stdout: "pushed".to_string(),
            stderr: String::new(),
        };
        let missing = std::env::temp_dir().join("git-pm-definitely-missing-repo");
        let out =
            restore_upstream_remote(Ok(push_out), &missing, "origin", "main", Lang::Zh, &auth)
                .expect("push result must stay Ok");
        assert!(out.ok(), "回写失败不得把推送改判为失败");
        assert!(out.stdout.contains("pushed"));
        assert!(
            out.stderr.contains(i18n::t(Lang::Zh, "warn-upstream-restore-failed")),
            "缺少警告：{}",
            out.stderr
        );
        assert!(!out.stderr.contains("s3cret"), "输出不得含凭据");
    }
}
