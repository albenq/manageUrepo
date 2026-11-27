//! SVN 命令行封装（只调 `svn` CLI，不引入 SVN 库）。
//!
//! 与 `git.rs` 平级：`run` 系列返回 `git::GitOutput`（退出码 + stdout/stderr），
//! 解析函数均为纯函数、可独立单测。凭据仅通过子进程参数传递
//! （`--username/--password --non-interactive`），明文不进日志。

use std::path::Path;
use std::process::Command;

use crate::git::{self, CommitInfo, GitOutput};

/// Run `svn <args>` with the process cwd set to `dir`.
///
/// Mirrors `git::run`: stdout/stderr are captured as lossy UTF-8 into a
/// `GitOutput`; spawn failures and a missing `dir` surface as `io::Error`.
pub fn run(dir: &Path, args: &[&str]) -> std::io::Result<GitOutput> {
    let output = Command::new("svn").args(args).current_dir(dir).output()?;
    Ok(git::capture(output))
}

/// Run `svn <args>` with credentials appended.
///
/// The password travels only as a child-process argument; callers must
/// redact the returned output before logging it (see `app::redact_output`).
pub fn run_with_credentials(
    dir: &Path,
    args: &[&str],
    user: &str,
    password: &str,
) -> std::io::Result<GitOutput> {
    let mut full: Vec<&str> = args.to_vec();
    full.extend_from_slice(&[
        "--username",
        user,
        "--password",
        password,
        "--non-interactive",
    ]);
    run(dir, &full)
}

/// Run `svn status` in `dir` (one line per changed/unversioned item).
pub fn status(dir: &Path) -> std::io::Result<GitOutput> {
    run(dir, &["status"])
}

/// Run `svn update` in `dir`.
pub fn update(dir: &Path) -> std::io::Result<GitOutput> {
    run(dir, &["update", "--non-interactive"])
}

/// Run `svn update` with injected credentials.
pub fn update_with_credentials(
    dir: &Path,
    user: &str,
    password: &str,
) -> std::io::Result<GitOutput> {
    run_with_credentials(dir, &["update", "--non-interactive"], user, password)
}

/// Stage unversioned items (`svn add --force . -q`) and commit
/// (`svn commit -m <msg> --non-interactive`).
///
/// Returns the `add` output when staging fails; otherwise the `commit`
/// output. Mirrors `git::commit_all`'s "commit everything" semantics.
pub fn commit_all(dir: &Path, message: &str) -> std::io::Result<GitOutput> {
    let add = run(dir, &["add", "--force", ".", "-q"])?;
    if !add.ok() {
        return Ok(add);
    }
    run(dir, &["commit", "-m", message, "--non-interactive"])
}

/// Stage + commit with injected credentials (redaction is the caller's job).
pub fn commit_all_with_credentials(
    dir: &Path,
    message: &str,
    user: &str,
    password: &str,
) -> std::io::Result<GitOutput> {
    let add = run_with_credentials(dir, &["add", "--force", ".", "-q"], user, password)?;
    if !add.ok() {
        return Ok(add);
    }
    run_with_credentials(
        dir,
        &["commit", "-m", message, "--non-interactive"],
        user,
        password,
    )
}

/// The newest `limit` revisions, newest first; `skip` pages past them.
///
/// `svn log` has no skip flag, so this fetches `skip + limit` entries and
/// drops the first `skip`. Fewer than `skip` entries in the history yields
/// an empty vector (the caller's `exhausted` signal).
pub fn log_recent(dir: &Path, skip: usize, limit: usize) -> Result<Vec<CommitInfo>, String> {
    let total = skip + limit;
    let total_arg = format!("--limit={total}");
    let out = run(dir, &["log", "-r", "HEAD:1", &total_arg]).map_err(|e| e.to_string())?;
    if !out.ok() {
        // Not a working copy (E155007) or other svn failure: report the
        // stderr, except "not a working copy" which is an empty log.
        if out.stderr.contains("E155007") {
            return Ok(Vec::new());
        }
        return Err(format!("svn log failed: {}", out.stderr));
    }
    let mut commits = parse_log(&out.stdout);
    commits.drain(..skip.min(commits.len()));
    Ok(commits)
}

/// The repository URL of the working copy (`svn info` → `URL:` line).
/// 预留：当前 UI 无消费者；避免 dead_code 告警，待 SVN 克隆功能实现时启用。
#[allow(dead_code)]
pub fn info_url(dir: &Path) -> Result<String, String> {
    let out = run(dir, &["info"]).map_err(|e| e.to_string())?;
    if !out.ok() {
        return Err(format!("svn info failed: {}", out.stderr));
    }
    parse_info_url(&out.stdout).ok_or_else(|| "svn info: URL line not found".to_string())
}

/// Parse `svn status` output into `(code, path)` pairs. See
/// `format_svn_status` (app.rs) for the display-oriented consumer.
#[cfg(test)]
pub(crate) fn parse_status_lines(stdout: &str) -> Vec<(char, String)> {
    let mut entries = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut chars = line.chars();
        let code = chars.next().unwrap_or(' ');
        let path = chars.as_str().trim().to_string();
        if path.is_empty() {
            continue;
        }
        entries.push((code, path));
    }
    entries
}

/// Separator line that delimits `svn log` records (72 dashes).
const LOG_SEP: &str = "------------------------------------------------------------------------";

/// Parse `svn log -r HEAD:1` output into `CommitInfo` records.
///
/// A record header looks like
/// `r123 | author | 2026-01-02 12:34:56 +0800 (Fri, 02 Jan 2026) | 1 line`
/// followed by a blank line and the (possibly multi-line) message. Message
/// lines are joined with spaces — the same convention as the git log
/// parser. `hash` keeps the `rNNN` revision string.
fn parse_log(stdout: &str) -> Vec<CommitInfo> {
    let mut commits = Vec::new();
    let mut in_header = false;
    for line in stdout.lines() {
        if line == LOG_SEP {
            in_header = true;
            continue;
        }
        if in_header {
            // Header: `rN | author | date | n line(s)`
            let fields: Vec<&str> = line.split('|').collect();
            if fields.len() >= 3 {
                let hash = fields[0].trim().to_string();
                let author = fields[1].trim().to_string();
                // Date: "2026-01-02 12:34:56 +0800 (...)" → keep "YYYY-MM-DD HH:MM".
                let date = fields[2].trim().get(..16).unwrap_or("").to_string();
                if hash.starts_with('r') {
                    commits.push(CommitInfo {
                        hash,
                        author,
                        date,
                        subject: String::new(),
                    });
                    in_header = false;
                }
            }
        } else if let Some(last) = commits.last_mut() {
            // Message lines: skip the blank line right after the header,
            // then accumulate until the next separator.
            if line.is_empty() && last.subject.is_empty() {
                continue;
            }
            if !last.subject.is_empty() {
                last.subject.push(' ');
            }
            last.subject.push_str(line);
        }
    }
    // Message lines are joined with spaces; the blank line before the next
    // separator adds a trailing space, so trim the finished subject.
    for commit in &mut commits {
        commit.subject = commit.subject.trim().to_string();
    }
    commits
}

/// Extract the value of the `URL: ` line from `svn info` output.
fn parse_info_url(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("URL: "))
        .map(str::trim)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------- parse_status_lines

    #[test]
    fn parse_status_lines_basic() {
        let raw = "M       src/a.rs\n?       b.txt\n";
        assert_eq!(
            parse_status_lines(raw),
            vec![('M', "src/a.rs".to_string()), ('?', "b.txt".to_string())]
        );
    }

    #[test]
    fn parse_status_lines_empty_and_blank() {
        assert!(parse_status_lines("").is_empty());
        assert!(parse_status_lines("\n\n").is_empty());
    }

    #[test]
    fn parse_status_lines_space_only_code_row_keeps_path() {
        // SVN 状态码列允许空白（例如属性修改行首为空格）：
        // 首字符为空格时按 `(code=' ', path)` 处理而不是跳过。
        let raw = "        prop.txt\n";
        assert_eq!(parse_status_lines(raw), vec![(' ', "prop.txt".to_string())]);
    }

    // ------------------------------------------------------- parse_log

    const SEP: &str = "------------------------------------------------------------------------";

    #[test]
    fn parse_log_single_and_multi_line_messages() {
        let raw = format!(
            "{sep}\nr2 | alice | 2026-01-02 12:34:56 +0800 (Fri, 02 Jan 2026) | 1 line\n\ntwo commits\n\n{sep}\nr1 | bob | 2026-01-01 09:00:00 +0800 (Thu, 01 Jan 2026) | 3 lines\n\nsubject line\nwrapped body here\nlast line\n\n{sep}\n",
            sep = SEP
        );
        let commits = parse_log(&raw);
        assert_eq!(commits.len(), 2, "{raw}");
        // 新的修订号在前（svn log -r HEAD:1 的顺序）。
        assert_eq!(commits[0].hash, "r2");
        assert_eq!(commits[0].author, "alice");
        assert_eq!(commits[0].date, "2026-01-02 12:34");
        assert_eq!(commits[0].subject, "two commits");
        assert_eq!(commits[1].hash, "r1");
        assert_eq!(commits[1].author, "bob");
        assert_eq!(commits[1].date, "2026-01-01 09:00");
        // 多行消息以空格连接（与 git 解析器行为对齐）。
        assert_eq!(commits[1].subject, "subject line wrapped body here last line");
    }

    #[test]
    fn parse_log_empty_output() {
        assert!(parse_log("").is_empty());
    }

    // --------------------------------------------------- parse_info_url

    #[test]
    fn parse_info_url_found() {
        let raw = "Path: workdir\nWorking Copy Root Path: workdir\nURL: https://svn.example.com/repo/trunk\nRelative URL: ^/trunk\n";
        assert_eq!(parse_info_url(raw).as_deref(), Some("https://svn.example.com/repo/trunk"));
    }

    #[test]
    fn parse_info_url_missing() {
        assert_eq!(parse_info_url("Path: x\n"), None);
        assert_eq!(parse_info_url(""), None);
    }

    // ------------------------------------------------ integration tests
    //
    // 这些测试需要本机安装 `svn` / `svnadmin` CLI；缺失时打印提示并跳过
    //（程序功能本身在无 svn 环境下是「调用失败记日志」，不依赖这些测试）。

    /// True when the svn CLI family is available on PATH.
    fn svn_available() -> bool {
        Command::new("svn")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    struct SvnTrio {
        _guard: tempfile::TempDir,
        /// file:/// URL of the freshly created repository (kept for clarity;
        /// checkout already consumed it).
        _url: String,
        /// First working copy.
        wc1: std::path::PathBuf,
        /// Second working copy (used by the update test).
        wc2: std::path::PathBuf,
    }

    /// Create a local repository with `svnadmin` and check out two working
    /// copies from it, with one initial commit in wc1.
    fn svn_trio() -> SvnTrio {
        let guard = tempfile::tempdir().expect("tempdir");
        let repo = guard.path().join("repo");
        let create = Command::new("svnadmin")
            .arg("create")
            .arg(&repo)
            .output()
            .expect("svnadmin create");
        assert!(create.status.success(), "svnadmin create failed");
        let url = format!("file:///{}", repo.to_str().unwrap().replace('\\', "/"));

        let wc1 = guard.path().join("wc1");
        let co1 = Command::new("svn")
            .args(["checkout", &url, wc1.to_str().unwrap()])
            .output()
            .expect("svn checkout wc1");
        assert!(co1.status.success(), "checkout wc1 failed");

        let wc2 = guard.path().join("wc2");
        let co2 = Command::new("svn")
            .args(["checkout", &url, wc2.to_str().unwrap()])
            .output()
            .expect("svn checkout wc2");
        assert!(co2.status.success(), "checkout wc2 failed");

        SvnTrio { _guard: guard, _url: url, wc1, wc2 }
    }

    #[test]
    fn status_reports_unversioned_file() {
        if !svn_available() {
            eprintln!("skipping: svn CLI not available");
            return;
        }
        let trio = svn_trio();
        std::fs::write(trio.wc1.join("new.txt"), "hello").expect("create file");

        let out = status(&trio.wc1).expect("svn status");
        assert!(out.ok(), "svn status failed: {}", out.stderr);
        assert!(
            out.stdout.contains("new.txt"),
            "unversioned file missing from status: {:?}",
            out.stdout
        );
    }

    #[test]
    fn log_recent_after_two_commits() {
        if !svn_available() {
            eprintln!("skipping: svn CLI not available");
            return;
        }
        let trio = svn_trio();
        std::fs::write(trio.wc1.join("a.txt"), "one").unwrap();
        let c1 = commit_all(&trio.wc1, "first commit").expect("commit 1");
        assert!(c1.ok(), "commit 1 failed: {}", c1.stderr);
        std::fs::write(trio.wc1.join("b.txt"), "two").unwrap();
        let c2 = commit_all(&trio.wc1, "second commit").expect("commit 2");
        assert!(c2.ok(), "commit 2 failed: {}", c2.stderr);

        let commits = log_recent(&trio.wc1, 0, 10).expect("log_recent");
        assert_eq!(commits.len(), 2, "two commits expected: {commits:?}");
        // Newest first.
        assert_eq!(commits[0].subject, "second commit");
        assert_eq!(commits[1].subject, "first commit");
        assert!(commits[0].hash.starts_with('r'), "hash must be a revision: {commits:?}");
        assert_eq!(commits[0].date.len(), 16, "date must be YYYY-MM-DD HH:MM");

        // skip=1 limit=1 returns the older commit only.
        let page = log_recent(&trio.wc1, 1, 1).expect("log_recent page");
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].subject, "first commit");
    }

    #[test]
    fn update_pulls_other_working_copy_commit() {
        if !svn_available() {
            eprintln!("skipping: svn CLI not available");
            return;
        }
        let trio = svn_trio();
        std::fs::write(trio.wc1.join("from-wc1.txt"), "hi").unwrap();
        let c = commit_all(&trio.wc1, "add from wc1").expect("commit in wc1");
        assert!(c.ok(), "commit in wc1 failed: {}", c.stderr);

        assert!(!trio.wc2.join("from-wc1.txt").exists(), "wc2 is stale before update");
        let up = update(&trio.wc2).expect("svn update");
        assert!(up.ok(), "svn update failed: {}", up.stderr);
        assert!(trio.wc2.join("from-wc1.txt").exists(), "update must bring the new file");
    }
}
