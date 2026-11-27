# SVN 项目支持（v5）实施计划

日期：2026-11-27
状态：待执行
前置：v4 + 行数统计已完成（HEAD `4667af0`）

## 目标

在现有 git-only 结构上支持 SVN 项目：

1. **识别**：添加项目时自动探测 `.svn` 目录 → 该项目标记为 `vcs = "svn"`。
2. **只读**：状态总览（`svn status`）、提交历史（`svn log`）、代码行数统计（`loc.rs` 天然支持，无需改动）。
3. **写操作**：更新（`svn update`）、提交（`svn add --force .` + `svn commit -m`），可选注入凭据（`--username/--password --non-interactive`，输出脱敏）。
4. **UI 裁剪**：SVN 项目隐藏分支/标签/推送/远端凭据相关 UI；git 项目行为完全不变。

约束：只调命令行 `svn`（本机需安装；未安装时相关操作报错并记日志，程序不崩）。延续既有架构：纯逻辑抽纯函数 + 单元测试、TDD（先红后绿）、i18n 双语、每任务一提交。

## 设计决策（已定，不要改动）

- **不做 VCS trait 抽象**。`git.rs` / `svn.rs` 两个平级模块，`app.rs` 按 `project.vcs == "svn"` 分派。只有两种 VCS，trait 是过度设计（YAGNI）。
- **复用 `git::GitOutput`**：`svn::run` 返回 `git::GitOutput`（就是个退出码+双流的输出类型），不新建类型。把 `git.rs` 里 `capture` 改为 `pub(crate)`。
- **数据库迁移**：`projects` 表加 `vcs TEXT NOT NULL DEFAULT 'git'` 列。新库直接带列；旧库用 `ALTER TABLE`，若报 "duplicate column name" 则忽略（幂等）。
- **探测规则**（`app.rs::detect_vcs(dir)`）：`dir/.svn` 是目录 → `"svn"`；否则 `git::is_repo(dir)` → `"git"`；都不是 → 报 `err-not-repo`（沿用现有错误）。
- **SVN 凭据**：复用现有凭据对话框，标签默认 `DEFAULT_PUSH_REMOTE`（"origin"）不变——SVN 项目里它只是"默认凭据标签"。`do_svn_update` / `do_svn_commit` 用 `load_credential_parts` 读取，明文只进 worker 线程，在 worker 内拼 `--password` 并对输出脱敏（复用 `redact_output` + `redact_text`）。
- **SVN 没有 push**：commit 即"发布"。操作区按钮：`更新`(update)、`提交`(commit，复用现有提交信息输入框)；隐藏：推送、分支、标签、历史改为 `svn log`、凭据按钮保留。
- **历史分页**：SVN `svn log -r HEAD:1 --limit N` 不支持 skip。取 `--limit (skip+limit)`，解析后丢弃前 `skip` 条，可实现翻页且翻到底时 `exhausted` 判定正确（返回条数 < skip+limit）。
- **svn status 着色**：底部输出区只显示原始输出（与 git status 相同待遇）；右侧状态区用 `format_svn_status` 把首列代码译为图例文字（复用现有 legend key 中已有的：modified/added/deleted/conflict/untracked；SVN 特有代码译为新 key）。

## 文件与接口

### 1. `src/db.rs`

```rust
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub added_at: String,
    pub vcs: String,          // "git" | "svn"
}

pub fn add_project(conn: &Connection, name: &str, path: &str, vcs: &str) -> rusqlite::Result<i64>;
```

- `SCHEMA` 的 `projects` 建表语句加 `vcs TEXT NOT NULL DEFAULT 'git'`。
- `init_db`：建表后执行迁移：

```rust
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let r = conn.execute("ALTER TABLE projects ADD COLUMN vcs TEXT NOT NULL DEFAULT 'git'", []);
    match r {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation => Ok(()), // 列已存在
        Err(e) => Err(e),
    }
}
```

（SQLite "duplicate column name: vcs" 报错码即 ConstraintViolation；写一条注释说明。执行前先建表 `CREATE TABLE IF NOT EXISTS`，新库建完再 ALTER 会命中 duplicate → 忽略，同样幂等。）
- `list_projects` 的 SELECT 加 `vcs` 列；`add_project` INSERT 加 vcs 参数。

**测试**（db.rs 已有 `#[cfg(test)]` 风格，沿用）：
- `add_and_list_project_round_trips_vcs`：add_project(vcs="svn") → list 出 `vcs == "svn"`；add 不传 vcs 的旧调用点无（改签名后编译期保证），另加 `add_project_defaults_vcs_to_git`：直接用 SQL 插一行不带 vcs 的（`INSERT INTO projects(name,path) VALUES(...)`），list 出 `vcs == "git"`。
- `migrate_old_db_without_vcs_column`：temp 路径手工建旧 schema（只有 id/name/path/added_at 四列 + 插一行），关连接后 `init_db` 打开 → 不报错且能 list 出 `vcs=="git"`。
- `migrate_idempotent`：对已是新 schema 的库二次 init → Ok。

### 2. `src/svn.rs`（新文件）

```rust
use std::path::Path;
use crate::git::GitOutput;

pub fn run(dir: &Path, args: &[&str]) -> std::io::Result<GitOutput>;
pub fn run_with_credentials(
    dir: &Path, args: &[&str], user: &str, password: &str,
) -> std::io::Result<GitOutput>;
// 实现：args = [..args, "--username", user, "--password", password,
//                "--non-interactive"]；密码只进子进程参数，不进日志。

pub fn status(dir: &Path) -> std::io::Result<GitOutput>;        // svn status
pub fn update(dir: &Path) -> std::io::Result<GitOutput>;        // svn update --non-interactive
pub fn update_with_credentials(dir, user, password) -> ...;
pub fn commit_all(dir: &Path, message: &str) -> std::io::Result<GitOutput>;
    // svn add --force . -q → 失败返回 add 输出；否则 svn commit -m <msg> --non-interactive
pub fn commit_all_with_credentials(dir, message, user, password) -> ...;
pub fn log_recent(dir: &Path, skip: usize, limit: usize) -> Result<Vec<CommitInfo>, String>;
    // CommitInfo = git::CommitInfo（复用，hash 存 rXXX）
pub fn info_url(dir: &Path) -> Result<String, String>;   // svn info → "URL: " 行
```

纯解析函数（可单测，不依赖 svn CLI）：

```rust
/// 解析 `svn status` 多行输出 → (状态码, 路径) 列表。
/// 行格式：首 1 字符状态码 + 若干空格 + 路径；空行跳过。
fn parse_status_lines(stdout: &str) -> Vec<(char, String)>;

/// 解析 `svn log` 输出（分隔线为 72 个 '-'）→ Vec<CommitInfo>。
/// 块首行：`r123 | author | 2026-01-01 12:34:56 +0800 (Thu, 01 Jan 2026) | 1 line`
/// 随后空行 + 多行消息（消息到下一个分隔线前，跳过末尾空行）。
fn parse_log(stdout: &str) -> Vec<CommitInfo>;
//   hash = "r123"，date 取第一个 '|' 后第二段的前 19 字符（"YYYY-MM-DD HH:MM:SS"
//   → 截成 "YYYY-MM-DD HH:MM" 与 git 一致）。author 去两侧空白。

/// 解析 `svn info` 输出中 `URL: ` 行的值。
fn parse_info_url(stdout: &str) -> Option<String>;
```

**单元测试**（纯解析，必有）：
- `parse_status_lines_basic`：`"M       src/a.rs\n?       b.txt\n"` → `[(M,src/a.rs),(?,b.txt)]`。
- `parse_status_lines_empty`：`""` → 空 vec；纯空行 → 空 vec。
- `parse_log_single_and_multi_line_messages`：手工构造两个块（一条单行消息、一条三行消息），断言 hash/author/date/subject（多行消息以空格连接——与 git 解析器 `parse_commit_lines` 行为对齐，注释说明）。
- `parse_log_skips_trailing_separator`。
- `parse_info_url_found_and_missing`。

**集成测试**（需要本机 `svn` CLI，缺失则跳过——`if svn_missing() { eprintln!("skip"); return; }`，探测方式 `Command::new("svn").arg("--version").output()` 失败即缺失）：
- `status_reports_unversioned_file`：`svnadmin create` 建 file:/// 仓库 → `svn checkout` 到 tempdir → 写文件 → `svn::status` 输出含 `?`。
- `log_recent_after_commit`：提交两条 → log_recent(0,10) 得 2 条，新 r 号在前；log_recent(1,1) 返回旧那条。
- `update_pulls_changes`：第二个 workdir 提交 → 第一个 workdir update 后文件出现。
（若本机无 svnadmin/svn，这些测试自动跳过，CI 与无 svn 环境安全。）

### 3. `src/git.rs`

- `fn capture(` → `pub(crate) fn capture(`（仅此一处改动）。

### 4. `src/app.rs`

#### 4a. 常量与结果类型

- 新 `ResultKind` 变体：
  - `SvnStatus`（只读批次）
  - `SvnUpdate(String)`（payload = 日志标签，如 `"svn update"` 或脱敏后的 `"svn update --username alice"`）
  - 复用 `Commit(String)`：SVN 提交也发 `Commit(label)`，label 如 `"svn add --force . + svn commit -m <msg>"`。
- 常量：`LABEL_SVN_STATUS: &str = "svn status"`。

#### 4b. 添加项目（`pick_and_add_project`）

```rust
fn detect_vcs(dir: &Path) -> Option<&'static str> {
    if dir.join(".svn").is_dir() { Some("svn") }
    else if git::is_repo(dir) { Some("git") }
    else { None }
}
```
- 探测 None → 现有 `err-not-repo`；Some(vcs) → `db::add_project(conn, &name, &path, vcs)`。
- 顶部项目列表每项名称后追加小徽标：SVN 项目显示 `(SVN)` 弱化文本（新 i18n 不需要，纯字面量 `SVN`；中文英文一致）。中央面板项目路径行后同样追加。
- 克隆对话框仅支持 git，本次不改（SVN checkout 属后续需求）。

#### 4c. 状态刷新（只读批次）

- `spawn_status_refresh`：worker 里按项目 vcs 分派（把 `vcs` 克隆进线程）：
  - git：现状不变（Status/BranchList/TagList 三条）。
  - svn：只发一条 `ResultKind::SvnStatus`（`svn::status(&path)`）。
- `apply_result` 增加：
  - `ResultKind::SvnStatus` → 同 `apply_status_part` 的 Status 分支路径，但注意 in-flight 守卫：SVN 批次只有一条消息，本条处理完就 `status_inflight.remove(&project_id)`。缓存仍写 `entry.porcelain`（内容为 svn status 原文经 `format_svn_status` 翻译后的文本），`branches/tags` 置空。
  - `ResultKind::SvnUpdate(label)` → `log_write_result` + `clear_busy_if` + `trigger_status_refresh`（与 Pull 分支同）。
- 新纯函数（放 app.rs 底部，与 `format_worktree_status` 并排）：

```rust
/// svn status 首列代码 → 图例文字。未识别代码原样保留。
fn format_svn_status(lang: Lang, raw: &str) -> String
// 'M'→legend-modified, 'A'→legend-added, 'D'→legend-deleted,
// 'C'→legend-conflict, '?'→legend-untracked,
// 'X'/'I'/'~'/'!'/'R' → 新 key（svn-status-external 等，见 i18n）。
// 无任何条目 → worktree-clean。
```

**测试**：`format_svn_status_translates_entries` / `format_svn_status_clean`（仿照 `format_worktree_status_*` 两个测试）。

#### 4d. 操作区裁剪（`show_operations_panel`）

- 取 `let is_svn = selected_project().map(|p| p.vcs == "svn").unwrap_or(false);`
- SVN 时：
  - 提交按钮 → `do_svn_commit()`（复用 `commit_msg` 输入框与禁用逻辑）。
  - 新增「更新」按钮（在提交上方，图标用现有 ICON_PULL 若有，否则 🔄 字面量）→ `do_svn_update()`。
  - 隐藏：推送按钮/推送对话框入口、分支区（`show_branch_area`）、标签区（`show_tag_area`）、状态区里的分支/标签两个 section（`show_branch_area` 调用点整块跳过；右侧状态列表里 branches/tags 两个 section 在 SVN 时跳过——SVN 快照里它们恒为 None，天然隐藏，无需额外分支）。
  - 历史、凭据、刷新、行数统计按钮保留（历史走 svn log，凭据可存 svn 密码）。

#### 4e. 写操作

```rust
fn do_svn_update(&mut self) {
    // busy 守卫同 do_pull；busy label = "busy-svn-update"
    // 读凭据（load_credential_parts(project.id, DEFAULT_PUSH_REMOTE)）
    // worker：有凭据 → svn::update_with_credentials；无 → svn::update
    // label：有凭据 "svn update --username <user>"（不含密码）；无 "svn update"
    // 输出经 redact_output(pairs=[(password, "***")]) 后回传，kind=SvnUpdate(label)
}

fn do_svn_commit(&mut self) {
    // 同 do_commit 骨架；worker：svn::commit_all[_with_credentials]
    // kind=Commit(label)，label = "svn add --force . + svn commit -m <msg>"
    // 有凭据时对输出脱敏
}
```

注意：`redact_text`/`redact_output` 已是模块级函数，直接用。凭据缺失时静默降级为无凭据调用（与现有 pull/push 行为一致）。

#### 4f. 历史

- `history_load_more`（找到该函数，读取其内部对 `git::log_recent` 的调用点）：按 `selected_project().vcs` 分派 → svn 调 `svn::log_recent(path, skip, limit)`。`exhausted` 判定共用现有逻辑（svn 的 skip 语义已在 svn::log_recent 内消化）。

### 5. `src/i18n.rs`

新增 key（zh / en）：

```
btn-svn-update      「更新」                    / "Update"
busy-svn-update     「正在更新…」               / "Updating…"
svn-status-external 「外部引用」                 / "External"
svn-status-missing  「丢失」                    / "Missing"
svn-status-ignored  「忽略」                    / "Ignored"
svn-status-replaced 「替换」                    / "Replaced"
svn-status-obstructed「受阻」                    / "Obstructed"
svn-status-modified-props 等其余 svn 特有代码如有需要一并补齐
```
（解析器只认首列；`X/I/!/~` 之外的代码统一走"未知代码原样保留"，避免 key 爆炸。最终 key 集以实现时为准，需覆盖 `format_svn_status` 测试里出现的所有代码。）

### 6. `README.md`

- 中文、英文功能列表各加一条「SVN 项目支持」说明（探测、状态、历史、更新、提交、行数统计、凭据）。

## 任务序列（每任务一次提交，TDD）

1. **db vcs 列**：改 `db.rs`（struct/add_project/SCHEMA/migrate）+ 4 个测试。跑 `cargo test db::`。提交 `feat: vcs column in projects table`。
2. **svn.rs 解析器**：新文件 + `run`/`run_with_credentials` + 3 个纯解析函数 + 解析单元测试（此时集成测试占位注释可留到任务 3）。跑 `cargo test svn::`。提交 `feat: svn cli module with parsers`。
3. **svn.rs 集成测试**：svnadmin/checkout/status/log/update 四条，带缺失跳过守卫。本机有 svn 则验证真跑通；无则确认 skip 路径。提交 `test: svn integration tests with graceful skip`。
4. **app.rs 识别与展示**：`detect_vcs` + `pick_and_add_project` 传 vcs + 项目列表/中央面板 SVN 徽标。提交 `feat: detect and tag svn projects`。
5. **app.rs 只读**：`SvnStatus` 结果流 + `format_svn_status`（先写测试）+ spawn/apply 分派。提交 `feat: svn status overview`。
6. **app.rs 历史与写操作**：历史分派 svn log；`do_svn_update`/`do_svn_commit` + UI 按钮与区域裁剪。提交 `feat: svn update, commit and history`。
7. **i18n + README**（若前面任务已顺手加了 key，此任务只做 README 与查漏）。提交 `docs: svn support in readme`。
8. **全量验证**：`cargo test` 全绿 → `cargo clippy --all-targets -- -D warnings` 零告警 → `cargo build --release` → `git push`。

## 验证标准

- 既有 git 项目全流程回归不受影响（全部 87+ 旧测试通过）。
- 手工验证：无 `.svn`、无 `.git` 的目录添加 → 报「不是仓库」；SVN 工作副本添加成功并显示 SVN 徽标；状态/历史/更新/提交/行数统计可用；git 项目按钮无变化。
- 无 `svn` 命令的环境：SVN 项目执行操作时输出区记录「svn 运行失败」而非崩溃。

## 明确不做（YAGNI）

- VCS trait 抽象、SVN checkout/克隆、分支/标签模拟（svn copy）、`svn:ignore` 编辑、属性查看。
