//! OS callbacks only post to the tray thread. The monitor owns debounce and rechecks.
use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, NotifyIpInterfaceChange, MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE,
};
use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;
use windows_sys::Win32::System::Power::{
    PowerRegisterSuspendResumeNotification, PowerUnregisterSuspendResumeNotification,
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_CALLBACK, MSG, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND,
};

const CHANGED: u32 = 0x8000 + 4;

unsafe extern "system" fn interface_changed(
    context: *const c_void,
    _: *const MIB_IPINTERFACE_ROW,
    _: MIB_NOTIFICATION_TYPE,
) {
    crate::post_thread_msg(context as usize as u32, CHANGED);
}

unsafe extern "system" fn power_event(context: *const c_void, event: u32, _: *const c_void) -> u32 {
    if matches!(event, PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND) {
        crate::post_thread_msg(context as usize as u32, CHANGED);
    }
    0
}

pub struct NetworkEvents {
    interface: HANDLE,
    power: HANDLE,
    power_params: Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>,
}

impl NetworkEvents {
    pub fn register(tid: u32) -> Self {
        let context = tid as usize as *mut c_void;
        let mut events = Self {
            interface: null_mut(),
            power: null_mut(),
            power_params: Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
                Callback: Some(power_event),
                Context: context,
            }),
        };
        // The context is a thread ID, never a pointer to Rust monitor state.
        unsafe {
            let err = NotifyIpInterfaceChange(
                AF_UNSPEC,
                Some(interface_changed),
                context,
                0,
                &mut events.interface,
            );
            if err != 0 {
                crate::logging::warn(&format!("[NETWORK_EVENT_ERROR] 监听接口变化失败：{err}"));
            }
            let err = PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                (&raw mut *events.power_params).cast(),
                &mut events.power,
            );
            if err != 0 {
                crate::logging::warn(&format!("[NETWORK_EVENT_ERROR] 监听系统恢复失败：{err}"));
            }
        }
        events
    }

    pub fn handle(&self, msg: &MSG, control: &crate::monitor::MonitorControl) -> bool {
        if msg.hwnd.is_null() && msg.message == CHANGED {
            control.network_changed();
            return true;
        }
        false
    }
}

impl Drop for NetworkEvents {
    fn drop(&mut self) {
        // Unregister on the tray thread, outside both OS callbacks.
        unsafe {
            if !self.interface.is_null() {
                CancelMibChangeNotify2(self.interface);
            }
            if !self.power.is_null() {
                PowerUnregisterSuspendResumeNotification(self.power as isize);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_callbacks_wake_the_tray_for_resume_and_interface_changes() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            PeekMessageW, PBT_APMSUSPEND, PM_REMOVE,
        };
        let tid = crate::current_thread_id() as usize as *const c_void;
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            // Peek establishes this test thread's Windows message queue.
            PeekMessageW(&mut msg, null_mut(), CHANGED, CHANGED, PM_REMOVE);
            power_event(tid, PBT_APMSUSPEND, std::ptr::null());
            assert_eq!(
                PeekMessageW(&mut msg, null_mut(), CHANGED, CHANGED, PM_REMOVE),
                0
            );
            power_event(tid, PBT_APMRESUMEAUTOMATIC, std::ptr::null());
            interface_changed(tid, std::ptr::null(), 0);
            for _ in 0..2 {
                assert_ne!(
                    PeekMessageW(&mut msg, null_mut(), CHANGED, CHANGED, PM_REMOVE),
                    0
                );
                assert!(msg.hwnd.is_null());
            }
        }
    }
}
