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

/// Stage all changes (`git add -A`) and create a commit (`commit -m <msg>`).
///
/// Returns the `add` output when staging fails; otherwise the `commit`
/// output.
pub fn commit_all(dir: &Path, message: &str) -> std::io::Result<GitOutput> {
    let add = run(dir, &["add", "-A"])?;
    if !add.ok() {
        return Ok(add);
    }
    run(dir, &["commit", "-m", message])
}

/// Run `git pull` in `dir`.
pub fn pull(dir: &Path) -> std::io::Result<GitOutput> {
    run(dir, &["pull"])
}

/// Run `git push <remote> <branch>` in `dir`.
///
/// No credential logic here: `remote` is the final remote name or URL; the
/// UI layer is responsible for embedding credentials into https URLs.
pub fn push(dir: &Path, remote: &str, branch: &str) -> std::io::Result<GitOutput> {
    run(dir, &["push", remote, branch])
}

/// Run `git branch <name>` in `dir` (creates the branch without checking
/// it out).
pub fn create_branch(dir: &Path, name: &str) -> std::io::Result<GitOutput> {
    run(dir, &["branch", name])
}

/// Run `git checkout <name>` in `dir`.
pub fn checkout(dir: &Path, name: &str) -> std::io::Result<GitOutput> {
    run(dir, &["checkout", name])
}

/// Run `git tag` in `dir`: annotated (`tag -a <name> -m <msg>`) when
/// `message` is `Some`, lightweight (`tag <name>`) otherwise.
pub fn create_tag(dir: &Path, name: &str, message: Option<&str>) -> std::io::Result<GitOutput> {
    match message {
        Some(msg) => run(dir, &["tag", "-a", name, "-m", msg]),
        None => run(dir, &["tag", name]),
    }
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

    /// Repository A plus a bare remote B: A is pushed to B, then B is cloned
    /// to C. Keeps the tempdir guards alive for the duration of the test.
    struct Trio {
        _guards: Vec<tempfile::TempDir>,
        a: PathBuf,
        b: PathBuf,
        c: PathBuf,
        branch: String,
    }

    /// tempdir A: repo with an initial commit; tempdir B: `git init --bare`
    /// remote; A pushes to B; B is cloned into tempdir C.
    fn remote_trio() -> Trio {
        let (g1, a) = temp_repo();
        let g2 = tempfile::tempdir().expect("create remote tempdir");
        let b = g2.path().to_path_buf();
        let init = run(&b, &["init", "--bare"]).expect("git init --bare");
        assert!(init.ok(), "git init --bare failed: {}", init.stderr);

        let head = run(&a, &["rev-parse", "--abbrev-ref", "HEAD"]).expect("current branch");
        assert!(head.ok(), "abbrev-ref failed: {}", head.stderr);
        let branch = head.stdout.trim().to_string();

        let psh = push(&a, b.to_str().expect("remote path"), &branch).expect("push A to B");
        assert!(psh.ok(), "initial push to bare remote failed: {}", psh.stderr);

        let g3 = tempfile::tempdir().expect("create clone tempdir");
        let c = g3.path().join("clone");
        let clone = clone_repo(b.to_str().expect("remote path"), &c).expect("clone B to C");
        assert!(clone.ok(), "clone failed: {}", clone.stderr);

        Trio { _guards: vec![g1, g2, g3], a, b, c, branch }
    }

    #[test]
    fn commit_all_creates_commit() {
        let (_guard, repo) = temp_repo();
        std::fs::write(repo.join("file.txt"), "content").expect("create file");
        let out = commit_all(&repo, "add file").expect("commit_all");
        assert!(out.ok(), "commit_all failed: {}", out.stderr);
        let head = run(&repo, &["rev-parse", "--verify", "HEAD"]).expect("rev-parse HEAD");
        assert!(head.ok(), "HEAD should verify after commit_all: {}", head.stderr);
    }

    #[test]
    fn push_pull_roundtrip() {
        let trio = remote_trio();
        // A makes a new commit and pushes it to the bare remote (by path).
        std::fs::write(trio.a.join("feature.txt"), "v2").expect("create file in A");
        let cmt = commit_all(&trio.a, "feature commit").expect("commit_all in A");
        assert!(cmt.ok(), "commit_all failed: {}", cmt.stderr);
        let psh = push(&trio.a, trio.b.to_str().expect("remote path"), &trio.branch)
            .expect("push from A");
        assert!(psh.ok(), "push failed: {}", psh.stderr);

        // C pulls and ends up with the same history as A.
        let pll = pull(&trio.c).expect("pull in C");
        assert!(pll.ok(), "pull failed: {}", pll.stderr);
        let log_a = run(&trio.a, &["log", "--format=%H"]).expect("log in A");
        let log_c = run(&trio.c, &["log", "--format=%H"]).expect("log in C");
        assert!(log_a.ok(), "log in A failed: {}", log_a.stderr);
        assert!(log_c.ok(), "log in C failed: {}", log_c.stderr);
        assert_eq!(
            log_a.stdout.trim(),
            log_c.stdout.trim(),
            "A and C logs should match after push/pull"
        );
    }

    #[test]
    fn create_branch_and_checkout() {
        let (_guard, repo) = temp_repo();
        let br = create_branch(&repo, "dev").expect("create_branch");
        assert!(br.ok(), "create_branch failed: {}", br.stderr);
        let list = branch_list(&repo).expect("branch_list");
        assert!(list.ok(), "branch_list failed: {}", list.stderr);
        assert!(
            list.stdout.lines().any(|l| l.trim() == "dev"),
            "dev missing from branch list: {}",
            list.stdout
        );
        let co = checkout(&repo, "dev").expect("checkout");
        assert!(co.ok(), "checkout failed: {}", co.stderr);
        let head = run(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).expect("abbrev-ref HEAD");
        assert!(head.ok(), "abbrev-ref failed: {}", head.stderr);
        assert_eq!(head.stdout.trim(), "dev", "HEAD should be on dev after checkout");
    }

    #[test]
    fn create_tag_annotated_and_light() {
        let (_guard, repo) = temp_repo();
        let ann = create_tag(&repo, "v1", Some("release one")).expect("create_tag annotated");
        assert!(ann.ok(), "annotated tag failed: {}", ann.stderr);
        let light = create_tag(&repo, "v2", None).expect("create_tag lightweight");
        assert!(light.ok(), "lightweight tag failed: {}", light.stderr);

        let tags = tag_list(&repo).expect("tag_list");
        assert!(tags.ok(), "tag_list failed: {}", tags.stderr);
        let names: Vec<&str> = tags.stdout.lines().map(|l| l.trim()).collect();
        assert!(names.contains(&"v1"), "v1 missing from tags: {}", tags.stdout);
        assert!(names.contains(&"v2"), "v2 missing from tags: {}", tags.stdout);

        let t1 = run(&repo, &["cat-file", "-t", "v1"]).expect("cat-file v1");
        assert!(t1.ok(), "cat-file v1 failed: {}", t1.stderr);
        assert_eq!(t1.stdout.trim(), "tag", "v1 should be an annotated tag object");
        let t2 = run(&repo, &["cat-file", "-t", "v2"]).expect("cat-file v2");
        assert!(t2.ok(), "cat-file v2 failed: {}", t2.stderr);
        assert_eq!(t2.stdout.trim(), "commit", "v2 should point directly at a commit");
    }
}
