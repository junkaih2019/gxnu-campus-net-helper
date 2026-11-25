//! 开机自启、路径迁移校验与可执行文件版本读取。
//!
//! 自启写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，
//! 自启项值名为 `GXNUCampusLoginRS`。

use std::cmp::Ordering;
use std::ffi::c_void;
use std::fs;
use std::path::{Component, Path, PathBuf};

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

use crate::{logging, updater};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "GXNUCampusLoginRS";
const MAX_VERSION_INFO_BYTES: u32 = 1024 * 1024;
const VS_FFI_SIGNATURE: u32 = 0xFEEF_04BD;

#[repr(C)]
#[derive(Clone, Copy)]
struct VsFixedFileInfo {
    dw_signature: u32,
    dw_struc_version: u32,
    dw_file_version_ms: u32,
    dw_file_version_ls: u32,
    dw_product_version_ms: u32,
    dw_product_version_ls: u32,
    dw_file_flags_mask: u32,
    dw_file_flags: u32,
    dw_file_os: u32,
    dw_file_type: u32,
    dw_file_subtype: u32,
    dw_file_date_ms: u32,
    dw_file_date_ls: u32,
}

#[link(name = "version")]
extern "system" {
    fn GetFileVersionInfoSizeW(lptstrfilename: *const u16, lpdwhandle: *mut u32) -> u32;
    fn GetFileVersionInfoW(
        lptstrfilename: *const u16,
        dwhandle: u32,
        dwlen: u32,
        lpdata: *mut c_void,
    ) -> i32;
    fn VerQueryValueW(
        pblock: *const c_void,
        lpsubblock: *const u16,
        lplpbuffer: *mut *mut c_void,
        pulen: *mut u32,
    ) -> i32;
}

fn registered_command() -> Option<String> {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_READ)
        .and_then(|key| key.get_value::<String, _>(VALUE_NAME))
        .ok()
}

/// 从注册表命令行（如 `"C:\App\gxnu.exe" --minimized`）中提取可执行文件路径。
pub fn parse_command_exe_path(command: &str) -> Option<PathBuf> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"')?;
        let inner = rest[..end].trim();
        return (!inner.is_empty()).then(|| PathBuf::from(inner));
    }
    let lower = trimmed.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find(".exe") {
        let end = search_from + rel + 4;
        if end == trimmed.len() || trimmed[end..].starts_with(char::is_whitespace) {
            return Some(PathBuf::from(&trimmed[..end]));
        }
        search_from = end;
    }
    trimmed
        .split_whitespace()
        .next()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

pub fn is_enabled() -> bool {
    registered_command()
        .and_then(|cmd| parse_command_exe_path(&cmd))
        .is_some_and(|path| path.is_file())
}

pub fn is_registered_to_current_exe() -> bool {
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    registered_command()
        .and_then(|cmd| parse_command_exe_path(&cmd))
        .is_some_and(|registered| registered.is_file() && same_exe_path(&registered, &current))
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE)
        .map_err(|err| format!("打开 Run 注册表项失败：{err}"))?;
    if enabled {
        let exe = std::env::current_exe().map_err(|err| format!("取程序路径失败：{err}"))?;
        let command = format!("\"{}\" --minimized", exe.display());
        key.set_value(VALUE_NAME, &command)
            .map_err(|err| format!("写入自启项失败：{err}"))
    } else {
        match key.delete_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("删除自启项失败：{err}")),
        }
    }
}

/// 当用户已开启开机自启，但注册表仍指向已删除文件或不高于当前版本的旧路径时，自动迁移到当前程序。
pub fn sync_autostart_if_needed(current_version: &str) {
    let Ok(current_exe) = std::env::current_exe() else {
        return;
    };
    if auto_manage_disabled_for(&current_exe) {
        return;
    }
    let Some(raw_cmd) = registered_command() else {
        return;
    };
    let Some(old_exe) = parse_command_exe_path(&raw_cmd) else {
        return;
    };
    if same_exe_path(&old_exe, &current_exe) {
        return;
    }

    let old_exists = old_exe.is_file();
    let old_version = old_exists.then(|| read_exe_version(&old_exe)).flatten();
    if !should_migrate_autostart(current_version, old_exists, old_version.as_deref()) {
        return;
    }

    match set_enabled(true) {
        Ok(()) => {
            logging::info(&format!(
                "[STARTUP] 开机自启路径已从「{}」自动同步至当前程序「{}」（v{current_version}）",
                old_exe.display(),
                current_exe.display()
            ));
        }
        Err(err) => {
            logging::warn(&format!("[STARTUP] 自动同步开机自启路径失败：{err}"));
        }
    }
}

/// 只在当前程序版本严格更高、或注册表指向的文件已不存在时迁移开机自启。
/// 同版本的另一份副本（QQ 接收目录、U 盘、桌面备份等）不会抢走自启路径。
pub(crate) fn should_migrate_autostart(
    current_version: &str,
    old_exists: bool,
    old_version: Option<&str>,
) -> bool {
    if !old_exists {
        return true;
    }
    let Some(old_ver) = old_version else {
        return false;
    };
    updater::compare_versions(current_version, old_ver) == Some(Ordering::Greater)
}

/// 已开启开机自启时，保存设置是否需要把注册表改指向当前程序。
/// 与启动时的自动同步使用同一套版本保护：注册表指向更高版本的现存文件时绝不改写。
pub fn should_repoint_to_current(current_version: &str) -> bool {
    let Ok(current_exe) = std::env::current_exe() else {
        return false;
    };
    if auto_manage_disabled_for(&current_exe) {
        return false;
    }
    let Some(registered) = registered_command().and_then(|cmd| parse_command_exe_path(&cmd)) else {
        return true;
    };
    if same_exe_path(&registered, &current_exe) {
        return false;
    }
    let exists = registered.is_file();
    let version = exists.then(|| read_exe_version(&registered)).flatten();
    should_migrate_autostart(current_version, exists, version.as_deref())
}

/// 临时目录、`cargo` 构建目录或设置了 `GXNU_NO_TAKEOVER` 时，不自动接管守护进程，也不自动改写开机自启。
pub fn auto_manage_disabled_for(exe: &Path) -> bool {
    std::env::var_os("GXNU_NO_TAKEOVER").is_some_and(|v| !v.is_empty())
        || is_in_temp_dir(exe)
        || is_dev_build_path(exe)
}

/// 识别 `target\debug`、`target\release` 以及 `target\<triple>\release` 等 cargo 产物路径。
pub fn is_dev_build_path(exe: &Path) -> bool {
    let parts: Vec<String> = exe
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    let is_profile = |s: &str| s == "debug" || s == "release";
    parts.iter().enumerate().any(|(i, part)| {
        part == "target"
            && (parts.get(i + 1).is_some_and(|p| is_profile(p))
                || parts.get(i + 2).is_some_and(|p| is_profile(p)))
    })
}

pub fn is_in_temp_dir(exe: &Path) -> bool {
    let exe_norm = normalize_path_key(exe);
    let temp_norm = ensure_trailing_sep(&normalize_path_key(&std::env::temp_dir()));
    !temp_norm.is_empty() && exe_norm.starts_with(&temp_norm)
}

pub fn same_exe_path(a: &Path, b: &Path) -> bool {
    normalize_path_key(a) == normalize_path_key(b)
}

fn normalize_path_key(path: &Path) -> String {
    let resolved = resolve_best_effort(path);
    let raw = resolved.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    stripped.replace('/', r"\").to_lowercase()
}

/// 优先对完整路径做 `canonicalize`；若目标文件尚不存在（例如临时目录探测路径或已删除文件），
/// 向上回溯最近存在的祖先目录展开 8.3 短路径（如 `RUNNER~1` → `runneradmin`）与符号链接后再拼接剩余文件名。
fn resolve_best_effort(path: &Path) -> PathBuf {
    if let Ok(canon) = fs::canonicalize(path) {
        return canon;
    }
    let mut tail = Vec::new();
    let mut cur = path;
    while let Some(parent) = cur.parent() {
        if parent == cur {
            break;
        }
        if let Some(name) = cur.file_name() {
            tail.push(name.to_os_string());
        }
        if let Ok(mut canon_parent) = fs::canonicalize(parent) {
            for seg in tail.iter().rev() {
                canon_parent.push(seg);
            }
            return canon_parent;
        }
        cur = parent;
    }
    path.to_path_buf()
}

fn ensure_trailing_sep(path: &str) -> String {
    if path.is_empty() || path.ends_with('\\') {
        path.to_string()
    } else {
        format!(r"{path}\")
    }
}

/// 查询指定运行中 PID 的可执行文件完整路径。
pub fn process_exe_path(pid: u32) -> Option<PathBuf> {
    if pid == 0 {
        return None;
    }
    // SAFETY: 只读查询指定 PID 的镜像路径，句柄在返回前立即关闭。
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if ok == 0 || size == 0 || (size as usize) > buf.len() {
            return None;
        }
        Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..size as usize],
        )))
    }
}

/// 读取可执行文件的版本号（优先读取 PE `VERSIONINFO` 资源，回退文件名语义版本）。
pub fn read_exe_version(path: &Path) -> Option<String> {
    read_pe_version(path).or_else(|| version_from_filename(path))
}

fn read_pe_version(path: &Path) -> Option<String> {
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: 调用 Windows version.dll 标准只读接口，返回指针均校验落在 `data` 缓冲区边界内。
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle);
        if size == 0 || size > MAX_VERSION_INFO_BYTES {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
            return None;
        }

        for subkey in [
            r"\StringFileInfo\080404b0\ProductVersion",
            r"\StringFileInfo\040904b0\ProductVersion",
        ] {
            if let Some(ver) = query_version_string(&data, subkey) {
                if updater::compare_versions(&ver, &ver).is_some() {
                    return Some(updater::normalize_version_str(&ver));
                }
            }
        }

        let root: Vec<u16> = "\\\0".encode_utf16().collect();
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if VerQueryValueW(
            data.as_ptr() as *const c_void,
            root.as_ptr(),
            &mut ptr,
            &mut len,
        ) == 0
            || ptr.is_null()
            || (len as usize) < std::mem::size_of::<VsFixedFileInfo>()
        {
            return None;
        }
        let offset = (ptr as usize).checked_sub(data.as_ptr() as usize)?;
        if data.len().checked_sub(offset)? < std::mem::size_of::<VsFixedFileInfo>() {
            return None;
        }
        let info = std::ptr::read_unaligned(ptr as *const VsFixedFileInfo);
        if info.dw_signature != VS_FFI_SIGNATURE {
            return None;
        }
        let (ms, ls) = if info.dw_product_version_ms != 0 || info.dw_product_version_ls != 0 {
            (info.dw_product_version_ms, info.dw_product_version_ls)
        } else {
            (info.dw_file_version_ms, info.dw_file_version_ls)
        };
        let (v0, v1, v2, v3) = (ms >> 16, ms & 0xFFFF, ls >> 16, ls & 0xFFFF);
        if (v0, v1, v2, v3) == (0, 0, 0, 0) {
            return None;
        }
        Some(if v3 == 0 {
            format!("{v0}.{v1}.{v2}")
        } else {
            format!("{v0}.{v1}.{v2}.{v3}")
        })
    }
}

unsafe fn query_version_string(data: &[u8], subkey: &str) -> Option<String> {
    let query: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let mut ptr: *mut c_void = std::ptr::null_mut();
    let mut len = 0u32;
    if VerQueryValueW(
        data.as_ptr() as *const c_void,
        query.as_ptr(),
        &mut ptr,
        &mut len,
    ) == 0
        || ptr.is_null()
        || len == 0
    {
        return None;
    }
    let offset = (ptr as usize).checked_sub(data.as_ptr() as usize)?;
    let max_units = data.len().checked_sub(offset)? / 2;
    let limit = (len as usize).min(max_units);
    let mut units = Vec::with_capacity(limit);
    let base = ptr as *const u16;
    for idx in 0..limit {
        let ch = std::ptr::read_unaligned(base.add(idx));
        if ch == 0 {
            break;
        }
        units.push(ch);
    }
    let text = String::from_utf16_lossy(&units).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn version_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    for part in stem.rsplit(['-', '_', ' ']) {
        let cand = part.trim_start_matches(['v', 'V']);
        if cand.contains('.')
            && cand.as_bytes().first().is_some_and(u8::is_ascii_digit)
            && updater::compare_versions(cand, cand).is_some()
        {
            return Some(updater::normalize_version_str(cand));
        }
    }
    None
}

/// 注册通知用的 AppUserModelID，为 Windows 弹窗设置应用名称。
pub fn ensure_app_id(app_id: &str) {
    let path = format!(r"Software\Classes\AppUserModelId\{app_id}");
    match RegKey::predef(HKEY_CURRENT_USER).create_subkey(&path) {
        Ok((key, _)) => {
            let _ = key.set_value("DisplayName", &crate::APP_NAME);
        }
        Err(err) => logging::warn(&format!("[STARTUP] 注册通知 AppID 失败：{err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_and_unquoted_run_commands() {
        assert_eq!(
            parse_command_exe_path(r#""F:\Tools\GXNU Helper\gxnu-v0.2.7.exe" --minimized"#),
            Some(PathBuf::from(r"F:\Tools\GXNU Helper\gxnu-v0.2.7.exe"))
        );
        assert_eq!(
            parse_command_exe_path(r"C:\Program Files\GXNU\gxnu-guard.exe --minimized"),
            Some(PathBuf::from(r"C:\Program Files\GXNU\gxnu-guard.exe"))
        );
        assert_eq!(parse_command_exe_path("   "), None);
        assert_eq!(parse_command_exe_path(r#""""#), None);
    }

    #[test]
    fn extracts_fallback_version_from_release_filenames() {
        assert_eq!(
            version_from_filename(Path::new("gxnu-campus-net-helper-v0.2.6.exe")).as_deref(),
            Some("0.2.6")
        );
        assert_eq!(
            version_from_filename(Path::new("gxnu-campus-net-helper-0.2.0-windows-x64.exe"))
                .as_deref(),
            Some("0.2.0")
        );
        assert_eq!(version_from_filename(Path::new("gxnu-guard.exe")), None);
    }

    #[test]
    fn autostart_migration_upgrades_or_repairs_without_downgrading() {
        // 原文件已删除：立即修复迁移
        assert!(should_migrate_autostart("0.2.7", false, None));
        // 新版本高于注册表中的旧版：自动升级路径
        assert!(should_migrate_autostart("0.2.7", true, Some("0.2.6")));
        assert!(should_migrate_autostart("0.2.7.9", true, Some("0.2.7.8")));
        // 同版本副本（QQ 接收目录 / U 盘 / 桌面备份）且原文件仍在：不抢自启
        assert!(!should_migrate_autostart("0.2.7", true, Some("0.2.7")));
        assert!(!should_migrate_autostart("0.2.7", true, Some("v0.2.7.0")));
        // 同版本但原文件已被剪切挪走：照常迁移到新位置
        assert!(should_migrate_autostart("0.2.7", false, None));
        // 临时运行旧版：绝不反向覆盖高版本自启
        assert!(!should_migrate_autostart("0.2.5", true, Some("0.2.7")));
        // 原文件存在但无法识别版本：保守不覆盖
        assert!(!should_migrate_autostart("0.2.7", true, None));
    }

    #[test]
    fn reads_current_process_path_and_detects_temp_directory() {
        let me = process_exe_path(std::process::id()).expect("query current test binary path");
        assert!(me.is_file());
        let in_tmp = std::env::temp_dir().join("gxnu-temp-probe.exe");
        assert!(is_in_temp_dir(&in_tmp));
        let nested_tmp = std::env::temp_dir()
            .join("gxnu-nonexistent-subdir")
            .join("gxnu-temp-probe.exe");
        assert!(is_in_temp_dir(&nested_tmp));
    }

    #[test]
    fn dev_build_and_temp_paths_are_never_auto_managed() {
        assert!(is_dev_build_path(Path::new(
            r"F:\repo\target\release\gxnu-guard.exe"
        )));
        assert!(is_dev_build_path(Path::new(
            r"F:\repo\target\x86_64-pc-windows-gnu\debug\gxnu-guard.exe"
        )));
        assert!(!is_dev_build_path(Path::new(
            r"F:\repo\builds\v0.2.7\gxnu-campus-net-helper-v0.2.7.8.exe"
        )));
        assert!(!is_dev_build_path(Path::new(
            r"D:\target practice\gxnu.exe"
        )));
        assert!(!is_dev_build_path(Path::new(r"D:\Games\target\gxnu.exe")));
        // 单元测试二进制本身位于 target\debug 下，必须被排除在自动接管与自启改写之外。
        assert!(auto_manage_disabled_for(&std::env::current_exe().unwrap()));
        assert!(!should_repoint_to_current("999.0.0"));
    }
}
