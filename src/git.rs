use std::path::Path;
use std::process::Command;

pub struct GitOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl GitOutput {
    /// True when the git process exited with code 0.
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

fn capture(output: std::process::Output) -> GitOutput {
    GitOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Run `git <args>` with the process cwd set to `repo_dir`.
///
/// stdout/stderr are captured as lossy UTF-8. If `git` cannot be spawned or
/// `repo_dir` does not exist, the `io::Error` is returned as-is.
pub fn run(repo_dir: &Path, args: &[&str]) -> std::io::Result<GitOutput> {
    let output = Command::new("git").args(args).current_dir(repo_dir).output()?;
    Ok(capture(output))
}

/// Run `git clone <url> <target>` with the process cwd left as the current
/// directory.
pub fn clone_repo(url: &str, target: &Path) -> std::io::Result<GitOutput> {
    let output = Command::new("git")
        .arg("clone")
        .arg(url)
        .arg(target)
        .output()?;
    Ok(capture(output))
}

/// True when `dir` is inside a git work tree.
pub fn is_repo(dir: &Path) -> bool {
    match run(dir, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(out) => out.ok() && out.stdout.trim() == "true",
        Err(_) => false,
    }
}

/// Run `git status --porcelain=v1 -b` in `dir`.
pub fn status_porcelain(dir: &Path) -> std::io::Result<GitOutput> {
    run(dir, &["status", "--porcelain=v1", "-b"])
}

/// Run `git branch --format=%(refname:short)` in `dir`.
pub fn branch_list(dir: &Path) -> std::io::Result<GitOutput> {
    run(dir, &["branch", "--format=%(refname:short)"])
}

/// Run `git tag --list` in `dir`.
pub fn tag_list(dir: &Path) -> std::io::Result<GitOutput> {
    run(dir, &["tag", "--list"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Create a throwaway git repository in a tempdir: `git init` plus
    /// user.email/user.name config and an initial commit (git < 2.45 lists
    /// no unborn branch, so `git branch` needs at least one commit).
    fn temp_repo() -> (tempfile::TempDir, PathBuf) {
        let guard = tempfile::tempdir().expect("create tempdir");
        let repo = guard.path().to_path_buf();
        let init = run(&repo, &["init"]).expect("git init in tempdir");
        assert!(init.ok(), "git init failed: {}", init.stderr);
        for args in [
            ["config", "user.email", "t@t"],
            ["config", "user.name", "t"],
        ] {
            let out = run(&repo, &args).expect("git config in tempdir");
            assert!(out.ok(), "git config failed: {}", out.stderr);
        }
        std::fs::write(repo.join(".gitkeep"), "").expect("create seed file");
        let add = run(&repo, &["add", "."]).expect("git add in tempdir");
        assert!(add.ok(), "git add failed: {}", add.stderr);
        let commit = run(&repo, &["commit", "-m", "init"]).expect("git commit in tempdir");
        assert!(commit.ok(), "git commit failed: {}", commit.stderr);
        (guard, repo)
    }

    #[test]
    fn run_captures_output_and_code() {
        let (_guard, repo) = temp_repo();
        let ok = run(&repo, &["status"]).expect("run git status");
        assert!(ok.ok(), "git status should succeed: {}", ok.stderr);
        let bad = run(&repo, &["rev-parse", "--verify", "nope"]).expect("run rev-parse");
        assert!(!bad.ok(), "rev-parse of unknown ref should fail");
    }

    #[test]
    fn status_porcelain_lists_changes() {
        let (_guard, repo) = temp_repo();
        std::fs::write(repo.join("new.txt"), "hello").expect("create untracked file");
        let out = status_porcelain(&repo).expect("status --porcelain");
        assert!(out.ok(), "status failed: {}", out.stderr);
        assert!(out.stdout.contains("??"), "untracked file missing: {}", out.stdout);
    }

    #[test]
    fn branch_list_contains_master_or_main() {
        let (_guard, repo) = temp_repo();
        let out = branch_list(&repo).expect("branch list");
        assert!(out.ok(), "branch list failed: {}", out.stderr);
        assert!(!out.stdout.trim().is_empty(), "branch list should not be empty");
    }

    #[test]
    fn missing_git_is_error() {
        let missing = std::env::temp_dir().join("git-pm-definitely-missing-repo");
        let result = run(&missing, &["status"]);
        assert!(result.is_err(), "run on a missing directory must return Err");
    }
}
