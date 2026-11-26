//! 纯文件系统列表模块（不依赖 egui），供中央文件浏览器使用。

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// 目录列表中的一个条目。
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// 本地时间 "YYYY-MM-DD HH:MM"；获取失败为 None。Task 4 中央浏览器展示。
    #[allow(dead_code)]
    pub modified: Option<String>,
    pub is_hidden: bool,
}

/// 列出目录内容：目录在前、组内按名称字母序。
///
/// path 不存在或非目录返回 Err；单条目元数据读失败则跳过该条目；
/// 文件名非 UTF-8 用 to_string_lossy；以 `.` 开头的名字（含 `.git`）标记 is_hidden。
pub fn list_dir(path: &Path) -> Result<Vec<Entry>, String> {
    let meta = fs::metadata(path).map_err(|_| format!("path does not exist: {}", path.display()))?;
    if !meta.is_dir() {
        return Err(format!("not a directory: {}", path.display()));
    }
    let rd =
        fs::read_dir(path).map_err(|e| format!("failed to read directory {}: {}", path.display(), e))?;

    let mut entries = Vec::new();
    for item in rd {
        // 单个条目读失败（权限/竞态删除等）→ 跳过该条目，不中断整个列表
        let Ok(item) = item else { continue };
        let Ok(meta) = item.metadata() else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        let is_hidden = name.starts_with('.');
        entries.push(Entry {
            name,
            is_dir,
            size,
            modified: fmt_modified(&meta),
            is_hidden,
        });
    }
    // 目录在前，组内按名称字母序
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

/// 人类可读文件大小：<1024 → "N B"，否则 1024 进制 KB/MB/GB 保留一位小数。
pub fn fmt_size(bytes: u64) -> String {
    const UNIT: f64 = 1024.0;
    if bytes < 1024 {
        return format!("{} B", bytes);
    }
    let mut v = bytes as f64;
    for unit in ["KB", "MB"] {
        v /= UNIT;
        if v < UNIT {
            return format!("{:.1} {}", v, unit);
        }
    }
    format!("{:.1} GB", v / UNIT)
}

/// 把修改时间格式化为本地时间 "YYYY-MM-DD HH:MM"；获取或换算失败返回 None。
fn fmt_modified(meta: &fs::Metadata) -> Option<String> {
    let sys: SystemTime = meta.modified().ok()?;
    let secs = sys.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let local = secs + local_utc_offset_secs();
    let days = local.div_euclid(86_400);
    let secs_of_day = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        y,
        m,
        d,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60
    ))
}

/// 本地时区相对 UTC 的偏移秒数。
#[cfg(windows)]
fn local_utc_offset_secs() -> i64 {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct WinSystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    extern "system" {
        fn GetSystemTime(lp: *mut WinSystemTime);
        fn GetLocalTime(lp: *mut WinSystemTime);
    }
    // 同一系统时钟的 UTC/本地读数之差即时区偏移；对齐到分钟以消除跨秒误差
    unsafe {
        let mut utc = WinSystemTime::default();
        let mut local = WinSystemTime::default();
        GetSystemTime(&mut utc);
        GetLocalTime(&mut local);
        if utc.year < 1970 || local.year < 1970 {
            return 0; // 字段异常时退回 UTC
        }
        let utc_secs = days_from_civil(utc.year as i64, utc.month as u32, utc.day as u32) * 86_400
            + utc.hour as i64 * 3600
            + utc.minute as i64 * 60
            + utc.second as i64;
        let local_secs =
            days_from_civil(local.year as i64, local.month as u32, local.day as u32) * 86_400
                + local.hour as i64 * 3600
                + local.minute as i64 * 60
                + local.second as i64;
        ((local_secs - utc_secs + 30).div_euclid(60)) * 60
    }
}

#[cfg(not(windows))]
fn local_utc_offset_secs() -> i64 {
    0 // 非 Windows 平台退回 UTC
}

/// 天数（自 1970-01-01 起，可为负）→ (年, 月, 日)。
/// Howard Hinnant 的 civil_from_days 算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (年, 月, 日) → 天数（自 1970-01-01 起）。
/// Howard Hinnant 的 days_from_civil 算法；仅 Windows 时区探测使用。
#[cfg(windows)]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn list_dir_sorts_dirs_first_and_marks_hidden() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir(dir.path().join("b_dir")).expect("mkdir");
        fs::write(dir.path().join("a.txt"), b"hello").expect("write");
        fs::create_dir(dir.path().join(".git")).expect("mkdir");
        fs::write(dir.path().join(".hidden"), b"x").expect("write");

        let entries = list_dir(dir.path()).expect("list_dir");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

        // 目录在前（.git 与 b_dir 均为目录），组内按名称字母序
        assert_eq!(names, vec![".git", "b_dir", ".hidden", "a.txt"]);
        assert!(entries.iter().take(2).all(|e| e.is_dir));
        assert!(entries.iter().skip(2).all(|e| !e.is_dir));

        // .git 与 .hidden 标记为隐藏；a.txt 与 b_dir 不隐藏
        let git = entries.iter().find(|e| e.name == ".git").expect(".git entry");
        let hidden = entries
            .iter()
            .find(|e| e.name == ".hidden")
            .expect(".hidden entry");
        let a = entries.iter().find(|e| e.name == "a.txt").expect("a.txt entry");
        let b = entries.iter().find(|e| e.name == "b_dir").expect("b_dir entry");
        assert!(git.is_hidden);
        assert!(hidden.is_hidden);
        assert!(!a.is_hidden);
        assert!(!b.is_hidden);
    }

    #[test]
    fn list_dir_reports_sizes_and_dir_flag() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("data.bin"), b"hello world").expect("write"); // 11 字节
        fs::create_dir(dir.path().join("sub")).expect("mkdir");

        let entries = list_dir(dir.path()).expect("list_dir");

        let file = entries
            .iter()
            .find(|e| e.name == "data.bin")
            .expect("data.bin entry");
        assert_eq!(file.size, 11);
        assert!(!file.is_dir);

        let sub = entries.iter().find(|e| e.name == "sub").expect("sub entry");
        assert!(sub.is_dir);
        assert_eq!(sub.size, 0);

        // modified 为本地时间 "YYYY-MM-DD HH:MM"（新建文件必有值）
        let m = file.modified.as_deref().expect("modified present");
        let b = m.as_bytes();
        assert_eq!(b.len(), 16);
        assert_eq!(b[4], b'-');
        assert_eq!(b[7], b'-');
        assert_eq!(b[10], b' ');
        assert_eq!(b[13], b':');
        assert!(b
            .iter()
            .enumerate()
            .all(|(i, &c)| matches!(i, 4 | 7 | 10 | 13) || c.is_ascii_digit()));
    }

    #[test]
    fn list_dir_missing_path_is_err() {
        let dir = tempdir().expect("tempdir");

        // 不存在的路径 → Err
        let missing = dir.path().join("definitely_missing");
        assert!(list_dir(&missing).is_err());

        // 非目录（普通文件）→ Err
        let file = dir.path().join("plain.txt");
        fs::write(&file, b"x").expect("write");
        assert!(list_dir(&file).is_err());
    }

    #[test]
    fn fmt_size_boundaries() {
        assert_eq!(fmt_size(0), "0 B");
        assert_eq!(fmt_size(1023), "1023 B");
        assert_eq!(fmt_size(1024), "1.0 KB");
        assert_eq!(fmt_size(1048576), "1.0 MB");
    }
}
