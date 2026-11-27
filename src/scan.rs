//! 项目扫描：递归查找目录树中的 git / svn 仓库根。
//!
//! 规则：
//! - 目录内含 `.git`（目录**或文件**——worktree 是文件）或 `.svn` 目录即为
//!   仓库根；找到后**不下钻**其子目录（嵌套子仓库不单独上报，只报父仓库）。
//! - 构建产物与依赖目录（`loc::SKIP_DIRS`）不进入，避免超长扫描。
//! - 子目录读取失败（权限/竞态）记入 `errors` 并继续；根目录不可读则整体
//!   返回 `Err`。
//! - 不跟随符号链接（防目录环）。

use std::path::{Path, PathBuf};

use crate::loc::SKIP_DIRS;

/// 一个扫描发现的仓库：路径 + VCS 类型（"git" | "svn"）。
#[derive(Debug, PartialEq, Eq)]
pub struct FoundRepo {
    pub path: PathBuf,
    pub vcs: &'static str,
}

/// 一次扫描的完整结果。
#[derive(Debug, Default)]
pub struct ScanOutcome {
    pub found: Vec<FoundRepo>,
    /// (目录路径, 错误信息)，按发现顺序。
    pub errors: Vec<(PathBuf, String)>,
}

/// 从 `root` 起递归扫描仓库根。根目录不可读时返回 `Err`。
pub fn scan_for_repos(root: &Path) -> std::io::Result<ScanOutcome> {
    let mut outcome = ScanOutcome::default();
    walk(root, &mut outcome)?;
    // 稳定输出顺序：按路径排序，便于测试与日志可读性。
    outcome.found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(outcome)
}

/// 目录含 `.git`（目录或文件）或 `.svn` 目录时即为仓库根。
fn is_repo_root(dir: &Path) -> bool {
    dir.join(".git").exists() || dir.join(".svn").is_dir()
}

fn vcs_of(dir: &Path) -> &'static str {
    if dir.join(".svn").is_dir() {
        "svn"
    } else {
        "git"
    }
}

fn walk(dir: &Path, outcome: &mut ScanOutcome) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else { continue };
        // 不跟随符号链接/连接点：避免目录环与重复统计。
        if file_type.is_symlink() {
            continue;
        }
        if !file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SKIP_DIRS
            .iter()
            .any(|skip| skip.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        if is_repo_root(&path) {
            outcome.found.push(FoundRepo {
                path: path.clone(),
                vcs: vcs_of(&path),
            });
            continue; // 找到仓库即停：嵌套子仓库不上报
        }
        // 子目录自身 read_dir 失败记入 errors；更深的错误已在下层入账。
        if let Err(e) = walk(&path, outcome) {
            outcome.errors.push((path, e.to_string()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "git-pm-scan-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    #[test]
    fn scan_finds_git_and_svn_repos() {
        let root = temp_root("basic");
        fs::create_dir_all(root.join("alpha/.git")).unwrap();
        fs::create_dir_all(root.join("beta/.svn")).unwrap();

        let outcome = scan_for_repos(&root).expect("scan");
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(
            outcome.found,
            vec![
                FoundRepo { path: root.join("alpha"), vcs: "git" },
                FoundRepo { path: root.join("beta"), vcs: "svn" },
            ]
        );
    }

    #[test]
    fn scan_git_worktree_marker_is_a_file() {
        let root = temp_root("worktree");
        fs::create_dir_all(root.join("wt")).unwrap();
        fs::write(root.join("wt/.git"), "gitdir: ../real").unwrap();

        let outcome = scan_for_repos(&root).expect("scan");
        assert_eq!(
            outcome.found,
            vec![FoundRepo { path: root.join("wt"), vcs: "git" }]
        );
    }

    #[test]
    fn scan_does_not_descend_into_found_repos() {
        // 父仓库命中后不再下钻：嵌套的子仓库不上报（需求 4：仅添加父项目）。
        let root = temp_root("nested");
        fs::create_dir_all(root.join("outer/.git")).unwrap();
        fs::create_dir_all(root.join("outer/vendor/inner/.svn")).unwrap();

        let outcome = scan_for_repos(&root).expect("scan");
        assert_eq!(outcome.found.len(), 1, "{:?}", outcome.found);
        assert_eq!(outcome.found[0].path, root.join("outer"));
        assert_eq!(outcome.found[0].vcs, "git");
    }

    #[test]
    fn scan_skips_build_dependency_and_vcs_dirs() {
        let root = temp_root("skipped");
        fs::create_dir_all(root.join("node_modules/pkg/.git")).unwrap();
        fs::create_dir_all(root.join("target/debug/x/.svn")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap(); // 根目录自身的元数据
        fs::create_dir_all(root.join("real/.git")).unwrap();

        let outcome = scan_for_repos(&root).expect("scan");
        assert_eq!(
            outcome.found,
            vec![FoundRepo { path: root.join("real"), vcs: "git" }]
        );
    }

    #[test]
    fn scan_missing_root_is_err() {
        let missing = std::env::temp_dir().join("git-pm-scan-missing-root");
        let _ = fs::remove_dir_all(&missing);
        assert!(scan_for_repos(&missing).is_err());
    }

    #[test]
    fn scan_empty_root_finds_nothing() {
        let root = temp_root("empty");
        let outcome = scan_for_repos(&root).expect("scan");
        assert!(outcome.found.is_empty());
        assert!(outcome.errors.is_empty());
    }
}
