# LocalRepoManager / LocalRepoManager（中英双语说明 Bilingual README）

[中文](#中文) | [English](#english)

---

<a name="中文"></a>

## 中文（Chinese）

本地 git 项目管理器：用 Rust + egui 编写的桌面应用，把常用的 git 仓库集中在一个项目列表里管理，并提供加密的凭据保险库。所有数据保存在本机，不依赖任何在线服务。

### 运行

依赖：已安装 [Rust](https://rustup.rs/) 工具链，且 `git` 命令在 `PATH` 中可用。

```bash
cargo run --release
```

窗口默认 1440×900（最小 1200×800），可自由调整大小。

### 首次启动

- 第一次启动会进入「设置主密码」界面：输入主密码并重复确认即可。主密码用于加密本机保存的凭据，**无法找回，请务必牢记**。
- 之后每次启动都会要求输入主密码解锁，才能进入主界面。
- 不保存凭据也可以正常使用 LocalRepoManager 的其他功能（仅 push/pull 时不会自动注入凭据）。

### 数据文件

- 所有数据保存在程序**当前工作目录**下的 `gitpm.db`（SQLite 单文件数据库）。从项目根目录执行 `cargo run` 时，它就生成在项目根目录，不会写入 `target/` 或用户主目录。
- 文件内包含项目列表和加密后的凭据；凭据以 AES-256-GCM 加密，密钥由主密码经 Argon2id（64 MiB / t=3）派生。
- `gitpm.db`（含 SQLite 的 `-wal`/`-shm` 附属文件）已列入 `.gitignore`，不会被提交。
- 移动 `gitpm.db` 即可整体迁移数据；更换目录后用同一个主密码解锁即可。

### 功能

- **项目树**：把本机已有的 git 仓库加入左侧项目列表，点击切换；移除只删列表记录，不动磁盘文件。项目按父目录层级分组为可折叠树，名称左对齐。树上方工具栏提供搜索：随输入逐字过滤项目与目录（英文不区分大小写、按子串匹配），命中目录时显示其整棵子树，并自动展开命中路径；`Esc` 或 `✕` 清空。
- **SVN 项目支持**：添加项目时自动探测 `.svn` 目录（列表与标题中带 `[SVN]` 徽标）；SVN 工作副本提供 `svn status` 状态总览（图例译文）、`svn log` 提交历史（支持翻页）、`svn update` 更新与 `svn add --force . + svn commit` 提交，凭据可存入加密库并经 `--username/--password --non-interactive` 注入、输出脱敏；代码行数统计同样可用。SVN 项目不显示分支/标签/推送。需本机安装 `svn` 命令行工具。
- **项目分组**：右侧项目树顶部显示用户自定义分组节点；右键项目可「移动到分组」（含新建分组…）或「移出分组」，右键分组节点可重命名（全部成员随迁，改到既有名称即合并）或删除分组（成员移回未分组，项目本身不受影响）。分组归属持久化在数据库中，重启后保留；未分组项目仍按路径层级展示。分组标题以深橙色显示。
- **输出编码**：git/svn 子进程输出先按 UTF-8 解码，失败时按系统 ANSI 代码页（如简体中文 GBK）还原，旧编码提交信息与 svn 输出不再乱码。
- **文本编辑器**：菜单「设置 → 设置文本编辑器...」配置编辑器命令（支持 `{file}` 占位符与带引号的含空格路径，配置持久化保存）；在文件浏览窗口点击文本文件（按扩展名识别：txt/md/源码/配置等及 .gitignore、Makefile 等无扩展名文本文件）即打开——配置了编辑器用编辑器，未配置则用系统默认关联程序（等同资源管理器双击），启动结果写入底部输出区。
- **扫描项目**：工具栏「扫描项目...」选择一个根目录，后台线程递归查找其中的 git / svn 仓库并自动加入项目树；仓库内部嵌套的子仓库不再单独上报（仅添加父仓库），`node_modules` / `target` 等构建与依赖目录不进入；过程信息（新增、已存在跳过）写入输出窗口，扫描出错信息以红色条目显示。
- **状态总览**：选中项目后显示 `git status`、分支列表、标签列表，可手动刷新。
- **代码行数统计**：操作面板一键统计当前项目工作区的代码行数，报告（文件数、总行数、总大小、耗时、按扩展名聚合，最多展示 20 项）打印到底部输出区；自动跳过 `.git` / `.svn` / `target` / `node_modules` 等目录与二进制、超大文件，后台线程执行，不阻塞界面。
- **提交（commit）**：填写提交信息后一键 `git add -A` + `git commit`。
- **拉取 / 推送（pull / push）**：对当前分支执行 `git pull` / `git push origin`。
- **新建分支**：输入名称后创建分支并切换（`git branch` + `git checkout`）。
- **打标签（tag）**：填写标签说明则创建附注标签（`-a -m`），否则创建轻量标签。
- **克隆仓库**：选择父目录并填写子目录名，`git clone` 成功后自动加入项目列表。
- **加密凭据**：按项目保存凭据（标签默认为 `origin`），AES-256-GCM 加密落盘；push/pull 到 https 的 `origin` 远端时自动注入，用完即弃。注入凭据的 git 输出会先做脱敏再显示，URL、密码（含转义形式）都不会出现在界面或日志中。
- **密码生成器**：凭据对话框内一键生成强密码，长度 8–64 可调，大小写字母与数字固定包含，特殊字符可选；结果仅保存在内存中，可复制或直接填入密码框。
- **独立密码生成器**：菜单「工具 → 密码生成器」打开独立窗口，生成规则与凭据对话框内的生成器一致；结果仅保存在内存中，可复制，关闭窗口即丢弃。
- **主题切换**：菜单栏「主题」可在暗黑 / 明亮之间切换，立即生效；偏好写入本地数据库，重启后保持上次选择。
- **中央文件浏览器**：选中项目后，中央面板以多列列表展示工作区文件（名称 / 类型 / 大小 / 修改时间，列宽可拖拽调整）；目录整行可点击进入，「上一级」返回上级目录；`.git` 等隐藏项整行置灰但仍可进入；文件仅只读展示，不会修改任何内容。
- **git 操作后台执行**：提交、拉取、推送、新建分支、打标签等操作均在后台线程执行（克隆除外，为同步执行），操作期间相关按钮禁用并显示动态进度条，窗口不再冻结；选中项目刷新状态时同样显示「加载中…」进度条。
- **推送到远程**：操作面板「推送…」打开推送对话框，可选择远端与分支，并可勾选「设置上游（-u）」；凭据注入与一键推送共用同一路径。对话框内的远端列表为同步查询（查询快，不冻结界面），推送本身仍在后台执行。注入凭据且设上游时，git 推送成功会把含明文凭据的注入 URL 写入仓库配置，程序会自动把上游指回真实远端名，避免凭据残留在 `.git/config` 中（回写失败不影响推送结果，会在输出区追加警告）。
- **提交历史**：操作面板「历史…」打开只读提交历史窗口，初始加载为同步查询（快）；默认显示最近 10 条提交，点「加载更多」每次追加 20 条，取完后按钮置灰。
- **文件状态图标**：文件浏览器中每个文件 / 目录名前显示圆形状态图标（对照 TortoiseSVN/TortoiseGit 官方覆盖图标语义）——绿底白勾=无变更、红底!=已修改、蓝底+=新增、灰蓝底?=未版本控制、橙红底×=已删除、蓝底R=重命名/复制、黄底!=冲突、灰底−=已忽略；包含变更文件的目录显示其中最高优先级的状态图标（未跟踪/忽略子项不向父目录传播，与 TortoiseGit 一致；折叠的未跟踪/忽略目录——git `?? dir/`、svn `? dir`/`I dir`——其下子项继承该状态），一眼定位改动所在。git 通过 `status --ignored` 列出忽略项。SVN 项目同样支持（解析 svn status --no-ignore 原始输出，`?`=未版本控制、`I`=已忽略单独成态）。状态图标在右侧状态区首次加载后出现。
- **在资源管理器中显示**：文件浏览器内任意文件 / 目录行右键 →「在资源管理器中显示」，用资源管理器打开其所在位置并选中该条目（`explorer /select`），结果写入底部输出区。
- **修改主密码**：菜单「设置 → 修改主密码」，需先验证旧密码；修改成功后所有已保存的凭据会立即用新密码重新加密，无需逐条重录。
- **密码生成器明文 / 遮蔽切换**：生成结果默认明文显示（可选中复制），勾选「遮蔽显示」即切换为等宽 `•` 掩码，两种模式随时切换。
- **图标**：工具栏各操作按钮带图标，便于快速识别；程序图标已嵌入 exe，在文件属性中可见。
- **界面语言切换**：菜单「Language / 语言」可在中文 ⇄ English 间切换，即时生效，重启后保持上次选择；领域层错误信息与 git 原始输出保持原文，不做翻译。
- **跨平台**：同一套代码支持 Windows 与 Linux；Linux 下自动探测 Noto CJK / WQY 等中文字体，Windows 专属功能自动降级（exe 图标嵌入仅 Windows）。

### 底部输出区

每次 git 调用都会在底部只读输出区记录一条（含退出码），失败为红色，最多保留 200 条，可手动清空。

### 开发

```bash
cargo test                      # 单元测试（crypto / db / git / password / app）
cargo clippy -- -D warnings     # 静态检查，无警告
```

---

<a name="english"></a>

## English

A local git project manager: a desktop application written in Rust + egui that keeps your frequently used git repositories in a single project list, with an encrypted credential vault. All data stays on your machine — no online services involved.

### Running

Requirements: the [Rust](https://rustup.rs/) toolchain must be installed, and the `git` command must be available on your `PATH`.

```bash
cargo run --release
```

The window opens at 1440×900 by default (minimum 1200×800) and can be resized freely.

### First Launch

- On first launch you will see the "Set Master Password" screen: enter a master password and confirm it. The master password encrypts credentials stored on this machine — **it cannot be recovered, so keep it safe**.
- Every subsequent launch asks for the master password to unlock the vault before entering the main screen.
- LocalRepoManager works fine without stored credentials (push/pull simply won't auto-inject credentials).

### Data Files

- All data is stored in `gitpm.db` (a single-file SQLite database) in the program's **current working directory**. Running `cargo run` from the project root creates it there; it never writes into `target/` or your home directory.
- The database contains the project list and encrypted credentials. Credentials are encrypted with AES-256-GCM; the key is derived from the master password via Argon2id (64 MiB / t=3).
- `gitpm.db` (including SQLite's `-wal`/`-shm` sidecar files) is listed in `.gitignore` and will not be committed.
- To migrate your data, just move `gitpm.db`; unlock with the same master password from the new location.

### Features

- **Project tree**: add existing local git repositories to the list on the left and switch between them with a click; removing a project only deletes the list entry, never touches files on disk. Projects are grouped by parent directory into a collapsible tree, with names left-aligned. A toolbar above the tree searches projects and folders as you type (letter-by-letter substring match, case-insensitive for English); matching a folder keeps its whole subtree, and matching paths auto-expand; `Esc` or `✕` clears.
- **SVN project support**: adding a project auto-detects a `.svn` working copy (badged `[SVN]` in the list and heading); SVN projects get an `svn status` overview (translated legend), `svn log` history (paged), `svn update`, and `svn add --force . + svn commit`. Credentials can be stored in the encrypted vault and are injected via `--username/--password --non-interactive` with redacted output; the lines-of-code counter works as well. Branch/tag/push UI is hidden for SVN. Requires the `svn` command-line tools installed.
- **Project groups**: user-defined group nodes appear at the top of the project tree. Right-click a project to "Move to group" (including "New group…") or "Remove from group"; right-click a group node to rename it (all members follow; renaming onto an existing name merges the groups) or delete it (members return to ungrouped, projects themselves are untouched). Group membership is persisted in the database and survives restarts; ungrouped projects still hang under the path hierarchy. Group titles are dark orange.
- **Project scanner**: the toolbar's "Scan projects..." picks a root directory and recursively discovers git / svn repositories on a background thread, adding them to the project tree automatically. Repositories nested inside another repository are not reported separately (only the parent is added); build and dependency directories such as `node_modules` / `target` are skipped. Progress goes to the output log — added/skipped repos as normal entries, scan errors as red entries.
- **Output encoding**: git/svn child-process output is decoded as UTF-8 first and falls back to the system ANSI codepage (e.g. GBK on zh-CN systems), so legacy-encoded commit messages and svn output no longer show mojibake.
- **Text editor**: configure the editor via "Settings → Set text editor..." (supports a `{file}` placeholder and quoted paths with spaces; the setting is persisted). Clicking a text file in the file browser (recognized by extension: txt/md/source/config files, plus extensionless text files like `.gitignore` / `Makefile`) opens it — with the configured editor, or with the system's default program when none is configured (same as double-clicking in Explorer); launch results go to the output log.

## Windows 7 build

Regular builds target Windows 10+ (Rust 1.78+ dropped Win7 for the default msvc target). A dedicated Win7 64-bit build uses the tier-3 `x86_64-win7-windows-msvc` target (nightly, std built from source, CRT statically linked — no VC++ redistributable needed on the target machine):

```
rustup toolchain install nightly --profile minimal --component rust-src
$env:RUSTFLAGS = "-C target-feature=+crt-static -L native=<registry>\windows_x86_64_msvc-0.48.5\lib"
cargo +nightly build --release --target x86_64-win7-windows-msvc -Z build-std=std
# artifact: target\x86_64-win7-windows-msvc\release\git-pm.exe
```

The `-L native` path works around `windows_x86_64_msvc`'s build script not recognizing the win7 target; `<registry>` is the cargo registry src directory (e.g. `%USERPROFILE%\.cargo\registry\src\<mirror-or-index>`).

### Windows 7 in a VM without OpenGL 2.0+

`egui_glow` needs OpenGL 2.0+; VMs without 3D acceleration (or machines without a GPU driver) only offer GL 1.1, and the app exits with `egui_glow requires opengl 2.0+` (see `git-pm.log`). The fix is a software renderer: download Mesa llvmpipe from [mmozeiko/build-mesa](https://github.com/mmozeiko/build-mesa/releases) — **use version ≤ 21.3.x**; newer builds import `WaitOnAddress`/`GetSystemTimePreciseAsFileTime`, which don't exist on Win7 (21.3.7 was verified clean: PE header 6.00, all imports Vista-era). Drop its `opengl32.dll` (x64) next to `git-pm.exe` and the app picks it up automatically, rendering on the CPU. The 21.3.7 build was verified to load and drive the app's GL context successfully.

- **Status overview**: selecting a project shows `git status`, branches, and tags, with manual refresh.
- **Lines-of-code counter**: one click in the operations panel scans the project's working tree and prints a report (files scanned, total lines, total size, elapsed time, per-extension breakdown, top 20) to the output log. `.git` / `.svn` / `target` / `node_modules` and other build or dependency directories, binary and oversized files are skipped; scanning runs on a background thread so the UI stays responsive.
- **Commit**: enter a message, then one click runs `git add -A` + `git commit`.
- **Pull / push**: runs `git pull` / `git push origin` on the current branch.
- **Create branch**: enter a name to create and switch to a branch (`git branch` + `git checkout`).
- **Tags**: with a message creates an annotated tag (`-a -m`), otherwise a lightweight tag.
- **Clone repository**: pick a parent directory and a subfolder name; after `git clone` succeeds the repo is added to the project list automatically.
- **Encrypted credentials**: store credentials per project (label defaults to `origin`), encrypted at rest with AES-256-GCM; they are injected automatically for push/pull against an https `origin` remote and discarded after use. Git output from credential-injected commands is redacted before display — the URL and the password (including escaped forms) never appear in the UI or logs.
- **Password generator**: one-click strong password generation inside the credentials dialog; length 8–64, upper/lowercase letters and digits always included, special characters optional. The result lives only in memory and can be copied or inserted into the password field.
- **Standalone password generator**: menu "Tools → Password Generator" opens a standalone window with the same rules; the result lives only in memory, can be copied, and is discarded when the window closes.
- **Theme switcher**: the "Theme" menu toggles dark / light instantly; the preference is persisted in the local database and restored on restart.
- **Central file browser**: with a project selected, the central panel lists working-tree files in columns (name / type / size / modified time, with draggable column widths); click a directory row to enter it, "Up" goes back; hidden entries such as `.git` are grayed out but still clickable; files are read-only and never modified.
- **Background git operations**: commit, pull, push, branch creation, and tagging all run on background threads (cloning is the exception and runs synchronously). Relevant buttons are disabled with an animated progress bar while an operation runs, so the window never freezes; refreshing a project's status shows a "Loading…" progress bar too.
- **Push to remote**: the "Push…" button opens a push dialog where you choose the remote and branch, with an optional "set upstream (-u)" checkbox. Credential injection shares the same path as one-click push. The remote list in the dialog is fetched synchronously (fast, doesn't freeze the UI), while the push itself runs in the background. When injecting credentials with `-u`, git would record the injected URL (containing plaintext credentials) in the repository config; the app automatically points the upstream back to the real remote name so no credentials linger in `.git/config` (if the rewrite fails the push still succeeds and a warning is appended to the output area).
- **Commit history**: the "History…" button opens a read-only commit history window; the initial load is a synchronous query (fast). It shows the latest 10 commits by default; "Load more" appends 20 at a time, and the button disables once exhausted.
- **File status icons**: every file / directory name in the browser is preceded by a circular status icon following the official TortoiseSVN/TortoiseGit overlay semantics — green circle with check = no changes, red with ! = modified, blue with + = added, gray-blue with ? = unversioned, orange-red with × = deleted, blue with R = renamed/copied, yellow with ! = conflict, gray with − = ignored. Directories containing changed files show the highest-priority status of their contents (untracked/ignored children do not propagate to parent directories, matching TortoiseGit; children of collapsed untracked/ignored dir entries — git `?? dir/`, svn `? dir` / `I dir` — inherit that status), so you can spot changes at a glance. Git lists ignored items via `status --ignored`. SVN projects are supported too (parses raw `svn status --no-ignore` output; `?` = unversioned and `I` = ignored are separate states). Icons appear once the status area has loaded for the selected project.
- **Show in Explorer**: right-click any file / directory row in the browser → "Show in Explorer" opens its location in Windows Explorer with the item selected (`explorer /select`); results go to the output log.
- **Change master password**: menu "Settings → Change Master Password" verifies the old password first; on success, all stored credentials are immediately re-encrypted with the new password — no need to re-enter them.
- **Password generator plaintext / masking toggle**: results are shown in plaintext by default (selectable and copyable); checking "Mask display" switches to a monospaced `•` mask. Switch between the two modes at any time.
- **Icons**: every toolbar button carries an icon for quick recognition; the application icon is embedded in the exe and visible in file properties.
- **UI language switch**: the "Language / 语言" menu toggles between Chinese ⇄ English instantly, and the choice persists across restarts; domain-layer error messages and raw git output stay in their original language, untranslated.
- **Cross-platform**: the same codebase supports Windows and Linux; on Linux, CJK fonts such as Noto CJK / WQY are detected automatically, and Windows-only features degrade gracefully (exe icon embedding is Windows-only).

### Bottom Output Area

Every git invocation is logged in the read-only output area at the bottom (including the exit code); failures appear in red. Up to 200 entries are kept, and the area can be cleared manually.

### Development

```bash
cargo test                      # unit tests (crypto / db / git / password / app)
cargo clippy -- -D warnings     # static checks, zero warnings
```
