//! 开机自启 + 通知 AppUserModelID 注册。
//!
//! 自启写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，
//! 自启项值名为 `GXNUCampusLoginRS`。

use crate::logging;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "GXNUCampusLoginRS";

pub fn is_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_READ)
        .and_then(|key| key.get_value::<String, _>(VALUE_NAME))
        .is_ok()
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

/// 注册通知用的 AppUserModelID，让 Windows 弹窗显示“GXNU 校园网守护”。
pub fn ensure_app_id(app_id: &str) {
    let path = format!(r"Software\Classes\AppUserModelId\{app_id}");
    match RegKey::predef(HKEY_CURRENT_USER).create_subkey(&path) {
        Ok((key, _)) => {
            let _ = key.set_value("DisplayName", &"GXNU 校园网守护");
        }
        Err(err) => logging::warn(&format!("[STARTUP] 注册通知 AppID 失败：{err}")),
    }
}
