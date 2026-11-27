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
pub fn info_url(dir: &Path) -> Result<String, String> {
    let out = run(dir, &["info"]).map_err(|e| e.to_string())?;
    if !out.ok() {
        return Err(format!("svn info failed: {}", out.stderr));
    }
    parse_info_url(&out.stdout).ok_or_else(|| "svn info: URL line not found".to_string())
}

/// Parse `svn status` output into `(code, path)` pairs.
///
/// Each line starts with one status column character followed by spaces and
/// the path. Blank lines are skipped; a leading space (a blank code column,
/// e.g. property-only rows) is kept as `(' ', path)`.
fn parse_status_lines(stdout: &str) -> Vec<(char, String)> {
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
}
