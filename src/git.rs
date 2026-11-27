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

pub(crate) fn capture(output: std::process::Output) -> GitOutput {
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

/// One commit as shown in the history view.
///
/// `hash` is the full 40-character SHA-1; `date` is formatted
/// "YYYY-MM-DD HH:MM".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub hash: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

/// List configured remotes (`git remote`), one name per entry.
///
/// A repository with no remotes yields an empty vector, not an error.
pub fn remote_list(repo: &Path) -> Result<Vec<String>, String> {
    let out = run(repo, &["remote"]).map_err(|e| e.to_string())?;
    if !out.ok() {
        return Err(format!("git remote failed: {}", out.stderr));
    }
    Ok(out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Field separator used by the `log_recent` `--pretty` format (ASCII unit
/// separator, emitted by git's `%x1f` placeholder).
const LOG_FIELD_SEP: char = '\u{1f}';

/// The newest commits of the current branch, newest first.
///
/// `skip` pages past the newest commits and `limit` caps the page size. An
/// empty history (no commits yet) yields an empty vector, not an error.
///
/// Note: `--max-count` is used instead of the equivalent short `-n` because
/// git parses `-n=<limit>` as the count `0` (strtol over the leading `=`),
/// silently returning nothing.
pub fn log_recent(repo: &Path, skip: usize, limit: usize) -> Result<Vec<CommitInfo>, String> {
    let skip_arg = format!("--skip={skip}");
    let limit_arg = format!("--max-count={limit}");
    let out = run(
        repo,
        &[
            "log",
            skip_arg.as_str(),
            limit_arg.as_str(),
            "--date=format:%Y-%m-%d %H:%M",
            "--pretty=%H%x1f%an%x1f%ad%x1f%s",
        ],
    )
    .map_err(|e| e.to_string())?;
    if !out.ok() {
        // `git log` fails on an unborn HEAD (no commits yet); report that as
        // an empty log. `rev-parse --verify --quiet HEAD` exits 1 exactly
        // when HEAD does not resolve, while other failures (not a
        // repository: 128, spawn errors) surface as Err here.
        let probe = run(repo, &["rev-parse", "--verify", "--quiet", "HEAD"])
            .map_err(|e| e.to_string())?;
        if probe.code == 1 {
            return Ok(Vec::new());
        }
        return Err(format!("git log failed: {}", out.stderr));
    }
    Ok(parse_commit_lines(&out.stdout))
}

/// Parse `git log --pretty=%H%x1f%an%x1f%ad%x1f%s` output.
///
/// A record starts when a line yields four `\x1f`-separated fields with a
/// 40-character hex hash first; any other line is treated as a continuation
/// of the previous subject, keeping the parser safe against line breaks.
fn parse_commit_lines(stdout: &str) -> Vec<CommitInfo> {
    let mut commits = Vec::new();
    for line in stdout.lines() {
        let mut fields = line.splitn(4, LOG_FIELD_SEP);
        match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some(hash), Some(author), Some(date), Some(subject))
                if hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                commits.push(CommitInfo {
                    hash: hash.to_string(),
                    author: author.to_string(),
                    date: date.to_string(),
                    subject: subject.to_string(),
                });
            }
            _ => {
                if let Some(last) = commits.last_mut() {
                    if !last.subject.is_empty() {
                        last.subject.push(' ');
                    }
                    last.subject.push_str(line);
                }
            }
        }
    }
    commits
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

    /// Create a git repository in a tempdir with no commits (unborn HEAD).
    fn temp_repo_unborn() -> (tempfile::TempDir, PathBuf) {
        let guard = tempfile::tempdir().expect("create tempdir");
        let repo = guard.path().to_path_buf();
        let init = run(&repo, &["init"]).expect("git init in tempdir");
        assert!(init.ok(), "git init failed: {}", init.stderr);
        (guard, repo)
    }

    /// temp_repo plus a second, empty commit by a different author.
    fn temp_repo_two_commits() -> (tempfile::TempDir, PathBuf) {
        let (guard, repo) = temp_repo();
        let second = run(
            &repo,
            &[
                "commit",
                "--allow-empty",
                "--author=Alice <alice@example.com>",
                "-m",
                "second commit",
            ],
        )
        .expect("second commit in tempdir");
        assert!(second.ok(), "second commit failed: {}", second.stderr);
        (guard, repo)
    }

    /// Assert that `date` looks like "YYYY-MM-DD HH:MM".
    fn assert_date_format(date: &str) {
        assert_eq!(date.len(), 16, "date must be 16 chars: {date:?}");
        let b = date.as_bytes();
        for idx in [0usize, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15] {
            assert!(b[idx].is_ascii_digit(), "non-digit at {idx} in {date:?}");
        }
        assert_eq!(b[4], b'-', "expected dash at index 4 in {date:?}");
        assert_eq!(b[7], b'-', "expected dash at index 7 in {date:?}");
        assert_eq!(b[10], b' ', "expected space at index 10 in {date:?}");
        assert_eq!(b[13], b':', "expected colon at index 13 in {date:?}");
    }

    #[test]
    fn remote_list_empty_repo_is_ok() {
        let (_guard, repo) = temp_repo_unborn();
        let remotes = remote_list(&repo).expect("remote_list on a repo without remotes");
        assert!(remotes.is_empty(), "expected no remotes: {remotes:?}");
    }

    #[test]
    fn log_recent_parses_fields_in_order() {
        let (_guard, repo) = temp_repo_two_commits();
        let commits = log_recent(&repo, 0, 10).expect("log_recent");
        assert_eq!(commits.len(), 2, "expected two commits: {commits:?}");
        let newest = &commits[0];
        let oldest = &commits[1];

        // Newest first: HEAD's commit must come first, verbatim.
        let head = run(&repo, &["rev-parse", "HEAD"]).expect("rev-parse HEAD");
        assert!(head.ok(), "rev-parse HEAD failed: {}", head.stderr);
        assert_eq!(newest.hash, head.stdout.trim(), "newest commit should be HEAD");
        assert_eq!(newest.hash.len(), 40, "hash must be the full 40-char SHA-1");
        assert!(
            newest.hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "hash must be hex: {}",
            newest.hash
        );
        assert_eq!(newest.author, "Alice");
        assert_date_format(&newest.date);
        assert_eq!(newest.subject, "second commit");

        assert_eq!(oldest.author, "t");
        assert_eq!(oldest.hash.len(), 40, "hash must be the full 40-char SHA-1");
        assert_date_format(&oldest.date);
        assert_eq!(oldest.subject, "init");
        assert_ne!(newest.hash, oldest.hash, "the two commits must differ");
    }

    #[test]
    fn log_recent_skip_pages() {
        let (_guard, repo) = temp_repo_two_commits();
        let page = log_recent(&repo, 1, 1).expect("log_recent with skip");
        assert_eq!(page.len(), 1, "skip=1 limit=1 should return exactly one commit");
        assert_eq!(page[0].subject, "init", "skip=1 should return the older commit");
        assert_eq!(page[0].author, "t");
    }

    #[test]
    fn log_recent_empty_history_is_ok() {
        let (_guard, repo) = temp_repo_unborn();
        let commits = log_recent(&repo, 0, 10).expect("log_recent on empty history");
        assert!(commits.is_empty(), "expected no commits: {commits:?}");
    }
}
