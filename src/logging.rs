//! 运行日志按 2 MB 轮转，保留 5 份备份。

use chrono::Local;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const BACKUPS: u32 = 5;
const TAIL_BLOCK_BYTES: usize = 32 * 1024;

struct Logger {
    path: PathBuf,
    file: File,
    size: u64,
    console: bool,
}

static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();
/// 窗口日志面板的订阅者（每行实时推送）。
static UI_SINKS: OnceLock<Mutex<Vec<Sender<String>>>> = OnceLock::new();

/// 订阅日志行；窗口每帧从返回的通道里取。
pub fn subscribe() -> Receiver<String> {
    let (tx, rx) = channel();
    let sinks = UI_SINKS.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut list) = sinks.lock() {
        list.push(tx);
    }
    rx
}

/// 读取最近 `max_lines` 条常规日志，跳过探测明细，并在需要时向轮转备份回溯。
pub fn read_tail(path: &Path, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for index in 0..=BACKUPS {
        if lines.len() == max_lines {
            break;
        }
        let path = if index == 0 {
            path.to_path_buf()
        } else {
            path_with_suffix(path, index)
        };
        if let Ok(mut file) = File::open(path) {
            let _ = append_tail(&mut file, max_lines, &mut lines);
        }
    }
    lines.reverse();
    lines
}

fn append_tail(file: &mut File, max_lines: usize, lines: &mut Vec<String>) -> io::Result<()> {
    let mut end = file.seek(SeekFrom::End(0))?;
    while end > 0 && lines.len() < max_lines {
        let remaining = max_lines - lines.len();
        let start = tail_start(file, end, remaining.max(256))?;
        file.seek(SeekFrom::Start(start))?;
        // 分块向前补足可见行，不解码整个日志文件。
        let batch: Vec<_> = BufReader::new((&mut *file).take(end - start))
            .lines()
            .map_while(Result::ok)
            .filter(|line| !is_probe_detail(line))
            .collect();
        lines.extend(batch.into_iter().rev().take(remaining));
        end = start;
    }
    Ok(())
}

pub(crate) fn is_probe_detail(line: &str) -> bool {
    line.get(20..)
        .and_then(|rest| rest.split_once(' '))
        .is_some_and(|(_, message)| message.starts_with("[PUBLIC_PROBE_FAILED] "))
}

fn tail_start(file: &mut File, end: u64, mut lines_left: usize) -> io::Result<u64> {
    let mut block = [0; TAIL_BLOCK_BYTES];
    let mut right = end;
    while right > 0 {
        let left = right.saturating_sub(TAIL_BLOCK_BYTES as u64);
        let chunk = &mut block[..(right - left) as usize];
        file.seek(SeekFrom::Start(left))?;
        file.read_exact(chunk)?;
        // 末尾的 LF 只终止最后一行；再往前的 LF 才界定要保留的行首。
        let scan = if right == end {
            chunk.strip_suffix(b"\n").unwrap_or(chunk)
        } else {
            chunk
        };
        for (i, &byte) in scan.iter().enumerate().rev() {
            if byte == b'\n' {
                lines_left -= 1;
                if lines_left == 0 {
                    return Ok(left + i as u64 + 1);
                }
            }
        }
        right = left;
    }
    Ok(0)
}

/// 初始化日志，返回 `app.log` 的绝对路径。
pub fn init(log_dir: &Path, console: bool) -> std::io::Result<PathBuf> {
    fs::create_dir_all(log_dir)?;
    let path = log_dir.join("app.log");
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let _ = LOGGER.set(Mutex::new(Logger {
        path: path.clone(),
        file,
        size,
        console,
    }));
    Ok(path)
}

pub fn log(level: &str, message: &str) {
    let line = format!(
        "{} {} {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        level,
        message
    );
    if let Some(lock) = LOGGER.get() {
        if let Ok(mut logger) = lock.lock() {
            if logger.console {
                print!("{line}");
            }
            if logger.file.write_all(line.as_bytes()).is_ok() {
                logger.size += line.len() as u64;
                if logger.size >= MAX_BYTES {
                    rotate(&mut logger);
                }
            }
        }
    }
    if let Some(sinks) = UI_SINKS.get() {
        if let Ok(list) = sinks.lock() {
            let trimmed = line.trim_end().to_string();
            for sink in list.iter() {
                let _ = sink.send(trimmed.clone());
            }
        }
    }
}

fn rotate(logger: &mut Logger) {
    let _ = logger.file.flush();
    for index in (1..=BACKUPS).rev() {
        let from = if index == 1 {
            logger.path.clone()
        } else {
            path_with_suffix(&logger.path, index - 1)
        };
        let to = path_with_suffix(&logger.path, index);
        if index == BACKUPS {
            let _ = fs::remove_file(&to);
        }
        if from.exists() {
            let _ = fs::rename(&from, &to);
        }
    }
    if let Ok(file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&logger.path)
    {
        logger.file = file;
        logger.size = 0;
    }
}

fn path_with_suffix(path: &Path, index: u32) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.display(), index))
}

pub fn info(message: &str) {
    log("INFO", message);
}

pub fn warn(message: &str) {
    log("WARNING", message);
}

pub fn error(message: &str) {
    log("ERROR", message);
}

/// 将路径规范化为适合 `explorer.exe` 解析的标准 Win32 路径（剥离 `\\?\` 前缀并统一反斜杠）。
pub fn normalize_win32_path(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('/', "\\");
    if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(stripped) = raw.strip_prefix(r"\\?\") {
        stripped.to_string()
    } else {
        raw
    }
}

/// 构造传给 `explorer.exe` 的原生命令行参数 `/select,"<path>"`（避免被 MSVCRT 整段二次套引号导致跳去「文档」库）。
pub fn explorer_select_raw_arg(path: &Path) -> String {
    let clean = normalize_win32_path(path).replace('"', "");
    format!("/select,\"{clean}\"")
}

/// 在 Windows 文件资源管理器中打开目录并将光标停留在目标文件（如 `app.log`）上；
/// 若目标文件尚不存在，则回退打开其所在目录。
pub fn reveal_in_explorer(path: &Path) {
    use std::os::windows::process::CommandExt;

    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // 若为尚未创建的日志文件，先行创建空文件以便资源管理器精确高亮选中
        if path.extension().is_some_and(|ext| ext == "log") {
            let _ = OpenOptions::new().create(true).append(true).open(path);
        }
    }

    if path.exists() {
        let raw_arg = explorer_select_raw_arg(path);
        let _ = std::process::Command::new("explorer.exe")
            .raw_arg(raw_arg)
            .spawn();
    } else if let Some(parent) = path.parent().filter(|p| p.exists()) {
        let _ = std::process::Command::new("explorer.exe")
            .arg(normalize_win32_path(parent))
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_tail_matches_full_read(path: &Path, text: &str, limits: &[usize]) {
        fs::write(path, text).unwrap();
        let full = fs::read_to_string(path).unwrap();
        let lines: Vec<_> = full.lines().collect();
        for &limit in limits {
            assert_eq!(
                read_tail(path, limit),
                lines[lines.len().saturating_sub(limit)..],
                "limit={limit}, file size={}",
                full.len()
            );
        }
    }

    #[test]
    fn tail_matches_lines_for_empty_files_and_mixed_terminators() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        for text in [
            "",
            "\n",
            "\r\n",
            "\n\n",
            "一行",
            "一行\r",
            "\r\n甲\n\n乙\r\n",
            "\n甲\r\n\r乙",
        ] {
            assert_tail_matches_full_read(&path, text, &[0, 1, 2, 3, 4, usize::MAX]);
        }
    }

    #[test]
    fn tail_handles_utf8_lines_and_crlf_split_across_reverse_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let history = "以往的守护记录\n".repeat(8000);
        let long_line = "界".repeat(TAIL_BLOCK_BYTES);
        for ending in ["", "\n", "\r\n"] {
            let text = format!("{history}{long_line}\r\n末行{ending}");
            assert!((1..=3).any(|n| !text.is_char_boundary(text.len() - n * TAIL_BLOCK_BYTES)));
            assert_tail_matches_full_read(&path, &text, &[0, 1, 2, 800, usize::MAX]);
        }
        // 从 EOF 回退一块时恰好从 LF 开始，CR 在上一块。
        let text = format!("首行\r\n{}", "x".repeat(TAIL_BLOCK_BYTES - 1));
        assert_eq!(text.as_bytes()[text.len() - TAIL_BLOCK_BYTES], b'\n');
        assert_tail_matches_full_read(&path, &text, &[1, 2, 3]);
    }

    #[test]
    fn tail_backfills_normal_logs_past_hidden_probe_details_and_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let detail = "2026-11-24 01:00:00 INFO [PUBLIC_PROBE_FAILED] target=fixture\n";
        fs::write(path_with_suffix(&path, 1), format!("older\n{detail}old\n")).unwrap();
        fs::write(&path, format!("recent\n{}", detail.repeat(1000))).unwrap();
        assert_eq!(read_tail(&path, 2), ["old", "recent"]);
        assert_eq!(read_tail(&path, 10), ["older", "old", "recent"]);
    }

    #[test]
    fn rotation_paths_use_numbered_backups() {
        let base = PathBuf::from(r"C:\logs\app.log");
        assert_eq!(
            path_with_suffix(&base, 1).display().to_string(),
            r"C:\logs\app.log.1"
        );
        assert_eq!(
            path_with_suffix(&base, 5).display().to_string(),
            r"C:\logs\app.log.5"
        );
    }

    #[test]
    fn explorer_select_arg_preserves_spaces_and_strips_verbatim_prefix() {
        let p = PathBuf::from(r"\\?\C:\Users\Test User\AppData\Local\GXNUCampusLogin/logs/app.log");
        assert_eq!(
            explorer_select_raw_arg(&p),
            r#"/select,"C:\Users\Test User\AppData\Local\GXNUCampusLogin\logs\app.log""#
        );
    }
}
