//! 运行日志按 2 MB 轮转，保留 5 份备份。

use chrono::Local;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const BACKUPS: u32 = 5;

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

/// 读取日志文件最后 `max_lines` 行（用于窗口启动时的历史回填）。
pub fn read_tail(path: &Path, max_lines: usize) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<String> = text.lines().map(|line| line.to_string()).collect();
    if lines.len() <= max_lines {
        lines
    } else {
        lines[lines.len() - max_lines..].to_vec()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
