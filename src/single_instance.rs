//! 单实例互斥。
//!
//! 守护进程使用命名互斥体 `GXNUCampusLoginRS`。

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

pub struct InstanceGuard {
    handle: HANDLE,
}

impl InstanceGuard {
    /// 返回 `(guard, already_running)`；guard 必须在程序存活期间持有。
    pub fn acquire(name: &str) -> Result<(Self, bool), String> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: 传入合法的 NUL 结尾宽字符串；句柄由 Drop 负责关闭。
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
        if handle.is_null() {
            let err = unsafe { GetLastError() };
            return Err(format!("创建单实例互斥体失败（错误码 {err}）"));
        }
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        Ok((Self { handle }, already_running))
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
        }
    }
}
