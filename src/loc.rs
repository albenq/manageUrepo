//! 代码行数统计（纯 std，不依赖 egui / git / svn）。
//!
//! 递归扫描项目工作区，跳过 VCS 元数据与常见构建/依赖目录，跳过二进制
//! 文件（含 NUL 字节）与超大文件，统计文本文件数、总行数、总字节数，
//! 并按扩展名聚合（行数降序）。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// 扫描时跳过的目录名（不区分大小写）。
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".gradle",
    ".next",
    ".cache",
    "coverage",
];

/// 超过该大小的文件不统计（多为生成物或二进制，读取代价高）。
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// 一次统计的汇总结果。
#[derive(Debug, Default)]
pub struct LocStats {
    /// 参与统计的文本文件数。
    pub files: usize,
    /// 总行数（最后一行没有换行符也计入）。
    pub lines: u64,
    /// 这些文件的总字节数。
    pub bytes: u64,
    /// 按扩展名聚合，行数降序（同分行数按文件数、扩展名排序）。
    pub by_ext: Vec<ExtStat>,
}

/// 单个扩展名的统计。
#[derive(Debug)]
pub struct ExtStat {
    /// 小写扩展名；无扩展名的文本文件为 `(none)`。
    pub ext: String,
    pub files: usize,
    pub lines: u64,
}

/// 统计 `root` 下的代码行数。根目录不可读时返回错误；子目录/文件读取
/// 失败则跳过，不影响整体结果。
pub fn count_lines(root: &Path) -> std::io::Result<LocStats> {
    let mut stats = LocStats::default();
    let mut by_ext: HashMap<String, (usize, u64)> = HashMap::new();
    walk(root, &mut stats, &mut by_ext)?;
    stats.by_ext = by_ext
        .into_iter()
        .map(|(ext, (files, lines))| ExtStat { ext, files, lines })
        .collect();
    stats.by_ext.sort_by(|a, b| {
        b.lines
            .cmp(&a.lines)
            .then_with(|| b.files.cmp(&a.files))
            .then_with(|| a.ext.cmp(&b.ext))
    });
    Ok(stats)
}

/// 递归遍历目录；子目录错误被吞掉（权限/竞态），根目录错误向上传播。
fn walk(
    dir: &Path,
    stats: &mut LocStats,
    by_ext: &mut HashMap<String, (usize, u64)>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else { continue };
        // 不跟随符号链接：避免目录环与重复统计。
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if SKIP_DIRS
                .iter()
                .any(|skip| skip.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            let _ = walk(&path, stats, by_ext);
        } else if file_type.is_file() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() > MAX_FILE_BYTES {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            let Some(lines) = count_text_lines(&bytes) else {
                continue;
            };
            stats.files += 1;
            stats.lines += lines;
            stats.bytes += meta.len();
            let ext = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.to_lowercase())
                .unwrap_or_else(|| "(none)".to_string());
            let agg = by_ext.entry(ext).or_insert((0, 0));
            agg.0 += 1;
            agg.1 += lines;
        }
    }
    Ok(())
}

/// 文本文件行数：二进制（含 NUL）返回 None；最后一行无换行符也计 1 行。
fn count_text_lines(bytes: &[u8]) -> Option<u64> {
    if bytes.contains(&0) {
        return None;
    }
    let mut lines = bytes.iter().filter(|&&byte| byte == b'\n').count() as u64;
    if matches!(bytes.last(), Some(&last) if last != b'\n') {
        lines += 1;
    }
    Some(lines)
}

/// 千分位格式化：`1234567` → `1,234,567`。
pub fn format_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "git-pm-loc-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    fn write(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write test file");
    }

    #[test]
    fn counts_text_lines_and_skips_binary_files() {
        let root = temp_root("basic");
        write(&root.join("a.rs"), b"fn main() {\n}\n");
        write(&root.join("sub/b.txt"), b"one\ntwo");
        write(&root.join("bin.dat"), b"abc\0def\n");

        let stats = count_lines(&root).expect("count");
        assert_eq!(stats.files, 2);
        assert_eq!(stats.lines, 4);
        assert_eq!(stats.bytes, 14 + 7);
        let ext: Vec<(&str, usize, u64)> = stats
            .by_ext
            .iter()
            .map(|e| (e.ext.as_str(), e.files, e.lines))
            .collect();
        assert_eq!(ext, vec![("rs", 1, 2), ("txt", 1, 2)]);
    }

    #[test]
    fn skips_ignored_directories() {
        let root = temp_root("ignored");
        write(&root.join("main.rs"), b"a\n");
        write(&root.join("target/gen.rs"), b"x\ny\nz\n");
        write(&root.join("node_modules/pkg/index.js"), b"1\n2\n3\n4\n");
        write(&root.join(".git/config"), b"x\n");

        let stats = count_lines(&root).expect("count");
        assert_eq!(stats.files, 1);
        assert_eq!(stats.lines, 1);
    }

    #[test]
    fn empty_file_has_zero_lines() {
        let root = temp_root("empty");
        write(&root.join("empty.txt"), b"");
        let stats = count_lines(&root).expect("count");
        assert_eq!(stats.files, 1);
        assert_eq!(stats.lines, 0);
    }

    #[test]
    fn missing_root_returns_error() {
        let missing = std::env::temp_dir().join("git-pm-loc-missing-root");
        let _ = fs::remove_dir_all(&missing);
        assert!(count_lines(&missing).is_err());
    }

    #[test]
    fn formats_thousands_separators() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(1000), "1,000");
        assert_eq!(format_thousands(1_234_567), "1,234,567");
    }
}
