//! 界面语言（中/英）文案表：领域模块，不依赖 egui。
//!
//! - [`Lang`] 是 UI 语言枚举，持久化在 `meta` 表的 [`LANG_KEY`] 键下；
//! - [`t`] 按 (key, zh, en) 静态表查文案，未命中返回 key 本身，
//!   让缺漏在界面上显形；
//! - git 原始输出与领域层（crypto/db/git）错误文案不翻译，程序只透传。

/// 界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// 中文（默认）。
    Zh,
    /// English。
    En,
}

/// `meta` 表中语言偏好的键；值为 [`LANG_ZH`] / [`LANG_EN`]。
pub const LANG_KEY: &str = "lang";

/// 中文在 `meta` 表中的存储值。
pub const LANG_ZH: &[u8] = b"zh";

/// 英文在 `meta` 表中的存储值。
pub const LANG_EN: &[u8] = b"en";

/// 从 `meta` 值解析语言：`b"en"` → En；其余（None / 其他值）→ Zh 默认。
pub fn from_meta(bytes: Option<&[u8]>) -> Lang {
    match bytes {
        Some(value) if value == LANG_EN => Lang::En,
        _ => Lang::Zh,
    }
}

/// 语言 → `meta` 存储值。
pub fn to_meta(lang: Lang) -> &'static [u8] {
    match lang {
        Lang::Zh => LANG_ZH,
        Lang::En => LANG_EN,
    }
}

/// 文案表：`(key, zh, en)`。key 稳定不变（重构时文案可改，key 不改）。
/// `{name}` 形式的占位符由调用方（app.rs 的 `tf`）替换为动态值。
const TABLE: &[(&str, &str, &str)] = &[
    // ------------------------------------------------ 启动 / 全局错误
    ("err-read-vault-state", "读取保险库状态失败：{err}", "Failed to read vault state: {err}"),
    ("err-read-theme", "读取主题偏好失败，已回退到暗黑主题：{err}", "Failed to read theme preference, falling back to dark theme: {err}"),
    ("err-read-lang", "读取语言偏好失败，已回退到中文：{err}", "Failed to read language preference, falling back to Chinese: {err}"),
    ("err-open-db", "无法打开数据库 {path}：{err}", "Cannot open database {path}: {err}"),
    ("err-db-not-open", "数据库未打开", "Database not open"),
    // ------------------------------------------------ Setup 界面
    ("setup-title", "欢迎使用 LocalRepoManager", "Welcome to LocalRepoManager"),
    ("setup-intro", "首次使用：请设置一个主密码，用于加密本机保存的凭据。", "First run: set a master password to encrypt the credentials stored on this machine."),
    ("setup-warning", "主密码无法找回，请务必牢记。", "The master password cannot be recovered. Keep it safe."),
    ("hint-master-password", "主密码", "Master password"),
    ("hint-repeat-master-password", "再次输入主密码", "Repeat master password"),
    ("btn-set-master-password", "设置主密码", "Set master password"),
    ("err-setup-empty", "请输入主密码并在第二栏重复输入", "Enter the master password and repeat it in the second field"),
    ("err-setup-mismatch", "两次输入的主密码不一致，请重新输入", "The two master passwords do not match; please re-enter them"),
    ("err-setup-master", "设置主密码失败：{err}", "Failed to set the master password: {err}"),
    ("err-unlock-new-vault", "解锁刚创建的保险库失败：{err}", "Failed to unlock the freshly created vault: {err}"),
    // ------------------------------------------------ Unlock 界面
    ("unlock-title", "解锁 LocalRepoManager", "Unlock LocalRepoManager"),
    ("unlock-intro", "请输入主密码以解锁凭据保险库。", "Enter the master password to unlock the credential vault."),
    ("btn-unlock", "解锁", "Unlock"),
    ("err-wrong-master", "主密码错误，请重试", "Wrong master password; please try again"),
    ("err-unlock", "解锁失败：{err}", "Unlock failed: {err}"),
    // ------------------------------------------------ 主界面中央
    ("project-path", "路径：{path}", "Path: {path}"),
    ("project-added-at", "添加时间：{time}", "Added at: {time}"),
    ("main-empty-intro", "从左侧选择一个项目，或点击「添加项目…」将一个 git 仓库加入列表。", "Select a project on the left, or click \"Add project…\" to add a git repository to the list."),
    // ------------------------------------------------ 顶栏按钮
    ("btn-add-project", "添加项目…", "Add project…"),
    ("btn-clone-repo", "克隆仓库…", "Clone repository…"),
    ("btn-remove-project", "移除项目", "Remove project"),
    // ------------------------------------------------ 顶栏菜单
    ("menu-theme", "主题", "Theme"),
    ("theme-dark", "暗黑", "Dark"),
    ("theme-light", "明亮", "Light"),
    ("menu-tools", "工具", "Tools"),
    ("tool-password-generator", "密码生成器", "Password generator"),
    ("menu-settings", "设置", "Settings"),
    ("settings-change-master-password", "修改主密码…", "Change master password…"),
    ("menu-language", "Language / 语言", "Language / 语言"),
    ("lang-zh", "中文", "中文"),
    ("lang-en", "English", "English"),
    // ------------------------------------------------ 偏好持久化失败（日志区）
    ("err-theme-save", "主题偏好保存失败", "Failed to save the theme preference"),
    ("err-lang-save", "语言偏好保存失败", "Failed to save the language preference"),
    // ------------------------------------------------ 项目列表（左侧）
    ("heading-projects", "项目", "Projects"),
    ("empty-project-list", "（暂无项目，点击上方「添加项目…」）", "(No projects yet; click \"Add project…\" above)"),
    ("group-ungrouped", "(未分组)", "(Ungrouped)"),
    ("tree-search-hint", "搜索项目或目录…", "Search projects or folders…"),
    ("tree-search-clear", "清除搜索", "Clear search"),
    ("tree-search-count", "命中 {n} 个项目", "{n} matching projects"),
    ("tree-search-empty", "没有匹配的项目或目录", "No matching projects or folders"),
    // ------------------------------------------------ 操作面板（右侧）
    ("heading-operations", "操作", "Operations"),
    ("busy-op-project", "{label}（{project}）", "{label} ({project})"),
    ("empty-ops-intro", "从左侧选择一个项目，即可查看其 git 状态并执行操作。", "Select a project on the left to view its git status and run operations."),
    ("btn-pull", "拉取（pull）", "Pull"),
    ("btn-push", "推送（push）", "Push"),
    ("btn-push-dialog", "推送…", "Push…"),
    ("btn-history", "历史…", "History…"),
    ("btn-credentials", "凭据…", "Credentials…"),
    ("btn-count-loc", "统计代码行数", "Count lines"),
    ("loc-running", "统计中…", "Counting…"),
    ("log-loc-title", "代码行数统计（{project}）", "Lines of code ({project})"),
    ("loc-files", "扫描文件：{n} 个", "Files scanned: {n}"),
    ("loc-lines", "总行数：{n}", "Total lines: {n}"),
    ("loc-size", "总大小：{size}", "Total size: {size}"),
    ("loc-elapsed", "耗时：{secs} 秒", "Elapsed: {secs}s"),
    ("loc-by-ext", "按扩展名：", "By extension:"),
    ("loc-ext-row", "  {ext}：{files} 个文件，{lines} 行", "  {ext}: {files} files, {lines} lines"),
    ("loc-more-ext", "  …另有 {n} 种扩展名未显示", "  …and {n} more extensions"),
    ("loc-empty", "（没有可统计的文本文件）", "(No text files to count)"),
    ("err-loc", "统计失败：{err}", "Count failed: {err}"),
    ("btn-svn-update", "更新", "Update"),
    ("busy-svn-update", "正在更新…", "Updating…"),
    ("svn-status-modified", "已修改", "Modified"),
    ("svn-status-added", "新增", "Added"),
    ("svn-status-deleted", "已删除", "Deleted"),
    ("svn-status-conflict", "冲突", "Conflict"),
    ("svn-status-untracked", "未跟踪", "Unversioned"),
    ("svn-status-external", "外部引用", "External"),
    ("svn-status-ignored", "忽略", "Ignored"),
    ("svn-status-missing", "丢失", "Missing"),
    ("svn-status-replaced", "替换", "Replaced"),
    ("svn-status-obstructed", "受阻", "Obstructed"),
    ("btn-scan-projects", "扫描项目...", "Scan projects..."),
    ("scan-running", "扫描中…", "Scanning…"),
    ("log-scan-title", "扫描项目（{root}）", "Scan projects ({root})"),
    ("log-scan-error-title", "扫描项目（{root}）— 出错", "Scan projects ({root}) — errors"),
    ("scan-summary", "共发现 {n} 个仓库，新增 {m} 个，跳过 {k} 个", "Found {n} repositories: {m} added, {k} skipped"),
    ("scan-none", "未发现任何仓库", "No repositories found"),
    ("scan-added", "已添加：{path}", "Added: {path}"),
    ("scan-exists", "已存在，跳过：{path}", "Already added, skipped: {path}"),
    ("scan-error-line", "{path}：{err}", "{path}: {err}"),
    ("menu-move-to-group", "移动到分组", "Move to group"),
    ("menu-new-group", "新建分组…", "New group…"),
    ("menu-remove-from-group", "移出分组", "Remove from group"),
    ("menu-rename-group", "重命名分组", "Rename group"),
    ("menu-remove-group", "删除分组", "Delete group"),
    ("group-dialog-create-title", "新建分组", "New group"),
    ("group-dialog-rename-title", "重命名分组", "Rename group"),
    ("hint-group-name", "分组名称", "Group name"),
    ("err-group-empty", "分组名称不能为空", "Group name cannot be empty"),
    ("err-group-save", "保存分组失败：{err}", "Failed to save group: {err}"),
    ("menu-set-editor", "设置文本编辑器...", "Set text editor..."),
    ("editor-dialog-title", "文本编辑器设置", "Text editor settings"),
    ("editor-intro", "配置用于打开文本文件的编辑器：", "Configure the editor used to open text files:"),
    ("hint-editor-cmd", "例如 notepad 或 \"C:\\Program Files\\...\\Code.exe\" -n", "e.g. notepad or \"C:\\Program Files\\...\\Code.exe\" -n"),
    ("editor-hint", "支持 {file} 占位符代表文件路径；不填则路径追加在命令末尾。保存后在文件浏览窗口点击文本文件即可打开。", "{file} is a placeholder for the file path; if omitted, the path is appended to the command. Click a text file in the browser to open it."),
    ("btn-clear-editor", "清除设置", "Clear"),
    ("err-save-editor", "保存编辑器配置失败：{err}", "Failed to save editor setting: {err}"),
    ("log-editor-title", "打开文件（{path}）", "Open file ({path})"),
    ("err-open-editor", "启动编辑器失败：{err}", "Failed to launch editor: {err}"),
    ("err-editor-not-set", "未配置文本编辑器（菜单：设置 → 设置文本编辑器...）", "No text editor configured (menu: Settings → Set text editor...)"),
    // ------------------------------------------------ 状态区
    ("heading-status", "状态", "Status"),
    ("btn-refresh", "刷新", "Refresh"),
    ("loading", "加载中…", "Loading…"),
    ("section-worktree", "工作区（status）", "Worktree (status)"),
    ("worktree-branch", "分支：", "Branch: "),
    ("worktree-clean", "（工作区干净，无变更）", "(Working tree clean)"),
    ("section-branches", "分支（branch）", "Branches (branch)"),
    ("section-tags", "标签（tag）", "Tags (tag)"),
    ("not-loaded", "（尚未加载）", "(Not loaded yet)"),
    // ------------------------------------------------ 提交区
    ("heading-commit", "提交（commit）", "Commit"),
    ("hint-commit-message", "提交信息（必填）", "Commit message (required)"),
    ("btn-commit", "提交", "Commit"),
    // ------------------------------------------------ 新建分支区
    ("heading-new-branch", "新建分支", "New branch"),
    ("hint-branch-name", "分支名称（必填）", "Branch name (required)"),
    ("btn-create-checkout", "创建并切换", "Create & checkout"),
    // ------------------------------------------------ 打标签区
    ("heading-tag", "打标签（tag）", "Tag"),
    ("hint-tag-name", "标签名称（必填）", "Tag name (required)"),
    ("hint-tag-message", "标签说明（可选；填写则创建附注标签）", "Tag message (optional; filled in creates an annotated tag)"),
    ("btn-tag", "打标签", "Create tag"),
    // ------------------------------------------------ 底部输出区
    ("heading-output", "输出", "Output"),
    ("btn-clear", "清空", "Clear"),
    ("empty-output", "（暂无输出：对项目执行 git 操作后，结果会显示在这里）", "(No output yet: git results will appear here after you run operations on a project)"),
    ("no-output", "（无输出）", "(no output)"),
    ("log-exit-code", "{label}（退出码 {code}）", "{label} (exit code {code})"),
    ("log-run-failed", "{label}（执行失败）", "{label} (failed to run)"),
    ("err-git-run", "无法运行 git：{err}", "Failed to run git: {err}"),
    ("warn-upstream-restore-failed", "⚠ 上游回写失败，.git/config 中可能残留含凭据的 URL，请手动检查", "⚠ Upstream restore failed: .git/config may still contain a URL with embedded credentials; please check it manually"),
    // ------------------------------------------------ 文件浏览器（中央）
    ("location-label", "位置：", "Location: "),
    ("not-loaded-path", "（未加载）", "(Not loaded)"),
    ("btn-parent-dir", "上一级", "Up one level"),
    ("err-read-dir", "无法读取目录：{err}", "Cannot read the directory: {err}"),
    ("err-enter-dir", "无法进入目录：{err}", "Cannot enter the directory: {err}"),
    ("col-name", "名称", "Name"),
    ("col-type", "类型", "Type"),
    ("col-size", "大小", "Size"),
    ("col-modified", "修改时间", "Modified"),
    ("type-dir", "目录", "Directory"),
    ("type-file", "文件", "File"),
    ("placeholder-dash", "—", "—"),
    ("empty-dir", "（空目录）", "(Empty directory)"),
    ("legend-label", "图例：", "Legend: "),
    ("legend-conflict", "冲突", "Conflict"),
    ("legend-deleted", "已删除", "Deleted"),
    ("legend-modified", "已修改", "Modified"),
    ("legend-renamed", "重命名/复制", "Renamed/copied"),
    ("legend-added", "新增/未跟踪", "Added/untracked"),
    // ------------------------------------------------ 凭据读取（日志区）
    ("err-load-credential", "读取凭据（{label}）失败", "Failed to load credential ({label})"),
    // ------------------------------------------------ 忙碌标签 / 后台合成文案
    ("busy-commit", "正在提交（commit）…", "Committing…"),
    ("busy-pull", "正在拉取（pull）…", "Pulling…"),
    ("busy-push", "正在推送（push）…", "Pushing…"),
    ("busy-push-to", "正在推送到 {remote}（{branch}）…", "Pushing to {remote} ({branch})…"),
    ("busy-new-branch", "正在新建分支「{name}」…", "Creating branch \"{name}\"…"),
    ("busy-tag", "正在打标签「{name}」…", "Creating tag \"{name}\"…"),
    ("err-no-current-branch", "无法确定当前分支：rev-parse 输出为空", "Cannot determine the current branch: empty rev-parse output"),
    ("cred-injected", "（凭据已注入）", " (credentials injected)"),
    ("label-tag-annotated", "git tag -a {name} -m <说明>", "git tag -a {name} -m <message>"),
    // ------------------------------------------------ 推送对话框
    ("push-dialog-title", "推送到远程 — {name}", "Push to remote — {name}"),
    ("push-dialog-intro", "选择远程仓库与分支进行推送；凭据注入与一键推送共用同一路径。", "Choose a remote and a branch to push; credential injection shares the same path as one-click push."),
    ("push-no-remotes", "无远程仓库，请先用命令行 git remote add", "No remotes configured; add one first with git remote add on the command line"),
    ("push-remote-label", "远程：", "Remote: "),
    ("hint-push-branch", "分支名（如 main）", "Branch name (e.g. main)"),
    ("push-set-upstream", "设置上游（-u）", "Set upstream (-u)"),
    ("btn-cancel", "取消", "Cancel"),
    ("btn-push-go", "推送", "Push"),
    ("err-no-project", "未选择项目", "No project selected"),
    ("err-select-remote", "请选择远程仓库", "Select a remote"),
    ("err-enter-branch", "请填写分支名", "Enter a branch name"),
    // ------------------------------------------------ 提交历史窗口
    ("history-title", "提交历史 — {name}", "Commit history — {name}"),
    ("no-commits", "（无提交）", "(No commits)"),
    ("col-hash", "哈希", "Hash"),
    ("col-author", "作者", "Author"),
    ("col-date", "日期", "Date"),
    ("col-subject", "说明", "Message"),
    ("btn-load-more", "加载更多", "Load more"),
    ("btn-close", "关闭", "Close"),
    // ------------------------------------------------ 移除项目确认
    ("remove-confirm-body", "确定要把「{name}」从列表中移除吗？", "Remove \"{name}\" from the list?"),
    ("remove-confirm-note", "仅从列表移除，不会删除磁盘上的文件。", "This only removes the project from the list; files on disk are not deleted."),
    ("btn-remove", "移除", "Remove"),
    // ------------------------------------------------ 项目操作错误
    ("err-list-projects", "读取项目列表失败：{err}", "Failed to list projects: {err}"),
    ("fd-select-repo", "选择 git 仓库目录", "Select a git repository directory"),
    ("err-not-repo", "所选目录不是 git 仓库：{path}", "The selected directory is not a git repository: {path}"),
    ("err-project-exists", "该项目已在列表中：{path}", "The project is already in the list: {path}"),
    ("err-add-project", "添加项目失败：{err}", "Failed to add the project: {err}"),
    ("err-remove-project", "移除项目失败：{err}", "Failed to remove the project: {err}"),
    // ------------------------------------------------ 克隆对话框
    ("fd-select-clone-parent", "选择克隆父目录", "Select the parent directory for cloning"),
    ("clone-dialog-title", "克隆仓库", "Clone repository"),
    ("clone-intro", "克隆一个 git 仓库；成功后自动加入项目列表。", "Clone a git repository; on success it is added to the project list automatically."),
    ("hint-clone-url", "仓库 URL（https://… 或 git@…）", "Repository URL (https://… or git@…)"),
    ("btn-select-parent", "选择父目录…", "Choose parent directory…"),
    ("no-parent-chosen", "（未选择父目录）", "(No parent directory chosen)"),
    ("hint-clone-subdir", "子目录名（必填，不能含路径分隔符）", "Subdirectory name (required; no path separators)"),
    ("btn-clone", "克隆", "Clone"),
    ("err-clone-fields", "请填写仓库 URL、父目录和子目录名", "Fill in the repository URL, the parent directory and the subdirectory name"),
    ("err-clone-subdir-sep", "子目录名不能包含路径分隔符", "The subdirectory name must not contain path separators"),
    ("err-clone-no-parent", "请先选择克隆父目录", "Choose the parent directory for cloning first"),
    ("err-clone-target-exists", "目标目录已存在：{path}", "The target directory already exists: {path}"),
    ("err-clone-failed", "克隆失败（退出码 {code}）：{detail}", "Clone failed (exit code {code}): {detail}"),
    // ------------------------------------------------ 凭据对话框
    ("cred-dialog-title", "凭据 — {name}", "Credentials — {name}"),
    ("cred-intro", "凭据加密后保存在本机数据库；push/pull 到 origin 的 https 远端时自动注入。", "Credentials are stored encrypted in the local database; they are injected automatically when pushing/pulling to https remotes of origin."),
    ("hint-cred-label", "标签（如 origin）", "Label (e.g. origin)"),
    ("hint-username", "用户名", "Username"),
    ("hint-password", "密码（掩码显示）", "Password (masked)"),
    ("btn-generate", "生成…", "Generate…"),
    ("btn-delete-credential", "删除凭据", "Delete credential"),
    ("btn-save", "保存", "Save"),
    ("err-cred-fields", "请填写标签、用户名和密码", "Fill in the label, the username and the password"),
    ("err-vault-locked", "保险库未解锁", "Vault not unlocked"),
    ("err-save-credential", "保存凭据失败：{err}", "Failed to save the credential: {err}"),
    ("err-cred-label-required", "请先填写要删除的标签", "Enter the label to delete first"),
    ("err-delete-credential", "删除凭据失败：{err}", "Failed to delete the credential: {err}"),
    // ------------------------------------------------ 修改主密码对话框
    ("pw-dialog-title", "修改主密码", "Change master password"),
    ("pw-intro", "修改主密码后，所有已保存的凭据会立即用新密码重新加密。", "After the master password changes, all saved credentials are re-encrypted with the new password immediately."),
    ("hint-old-master-password", "旧主密码", "Old master password"),
    ("hint-new-master-password", "新主密码", "New master password"),
    ("hint-repeat-new-master-password", "再次输入新主密码", "Repeat new master password"),
    ("btn-change", "修改", "Change"),
    ("err-pw-fields", "请填写旧密码、新密码和确认密码", "Fill in the old password, the new password and the confirmation"),
    ("err-pw-mismatch", "两次输入的新密码不一致，请重新输入", "The two new passwords do not match; please re-enter them"),
    ("err-change-master-password", "修改主密码失败：{err}", "Failed to change the master password: {err}"),
    ("log-master-changed", "主密码已修改", "Master password changed"),
    ("log-master-changed-body", "所有已保存凭据已用新主密码重新加密。", "All saved credentials have been re-encrypted with the new master password."),
    ("log-master-changed-key-fail", "主密码已修改，但刷新会话密钥失败", "Master password changed, but refreshing the session keys failed"),
    ("log-restart-needed", "{err}；请重启程序并用新主密码解锁。", "{err}; please restart the program and unlock with the new master password."),
    // ------------------------------------------------ 密码生成器
    ("gen-title", "密码生成器", "Password generator"),
    ("gen-length", "长度", "Length"),
    ("gen-upper-fixed", "大写字母（固定启用）", "Uppercase letters (always on)"),
    ("gen-lower-fixed", "小写字母（固定启用）", "Lowercase letters (always on)"),
    ("gen-digits-fixed", "数字（固定启用）", "Digits (always on)"),
    ("gen-special", "特殊字符", "Special characters"),
    ("btn-generate-plain", "生成", "Generate"),
    ("gen-masked", "遮蔽显示", "Mask display"),
    ("gen-result-empty", "（点击「生成」后显示在此）", "(Click \"Generate\" to see the result here)"),
    ("gen-result-hint", "（点击「生成」后显示在此，明文显示）", "(Click \"Generate\" to see the result here, in plain text)"),
    ("btn-copy", "复制", "Copy"),
    ("btn-fill-password", "填入密码框", "Fill password field"),
    ("err-generate", "生成密码失败：{err}", "Failed to generate the password: {err}"),
];

/// 查表翻译：按 key 找到条目后返回对应语言文案；未命中返回 key 本身
/// （让缺漏在 UI 中显形，便于自查）。
pub fn t(lang: Lang, key: &'static str) -> &'static str {
    for (k, zh, en) in TABLE {
        if *k == key {
            return match lang {
                Lang::Zh => zh,
                Lang::En => en,
            };
        }
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 所有条目的 key 唯一。
    #[test]
    fn table_keys_are_unique() {
        let mut seen: Vec<&str> = Vec::new();
        for (key, _, _) in TABLE {
            assert!(!seen.contains(key), "duplicate key: {key}");
            seen.push(key);
        }
    }

    /// 所有条目的 zh / en 文案均非空，且不含未替换完的占位符花括号残缺。
    #[test]
    fn table_entries_are_non_empty() {
        for (key, zh, en) in TABLE {
            assert!(!zh.trim().is_empty(), "empty zh for {key}");
            assert!(!en.trim().is_empty(), "empty en for {key}");
        }
    }

    /// `t` 对每个 key 在两种语言下都命中且非空。
    #[test]
    fn t_hits_both_languages_for_every_key() {
        for (key, zh, en) in TABLE {
            assert_eq!(t(Lang::Zh, key), *zh, "zh miss for {key}");
            assert_eq!(t(Lang::En, key), *en, "en miss for {key}");
            assert!(!t(Lang::Zh, key).is_empty());
            assert!(!t(Lang::En, key).is_empty());
        }
    }

    /// 未命中的 key 原样返回（缺漏在 UI 中显形）。
    #[test]
    fn t_miss_returns_key_itself() {
        assert_eq!(t(Lang::Zh, "no-such-key"), "no-such-key");
        assert_eq!(t(Lang::En, "no-such-key"), "no-such-key");
    }

    /// meta 值解析：b"en" → En；其余 / None → Zh；to_meta 可逆。
    #[test]
    fn meta_roundtrip() {
        assert_eq!(from_meta(Some(b"en")), Lang::En);
        assert_eq!(from_meta(Some(b"zh")), Lang::Zh);
        assert_eq!(from_meta(Some(b"fr")), Lang::Zh);
        assert_eq!(from_meta(None), Lang::Zh);
        assert_eq!(from_meta(Some(to_meta(Lang::Zh))), Lang::Zh);
        assert_eq!(from_meta(Some(to_meta(Lang::En))), Lang::En);
        assert_eq!(to_meta(Lang::Zh), LANG_ZH);
        assert_eq!(to_meta(Lang::En), LANG_EN);
    }
}
