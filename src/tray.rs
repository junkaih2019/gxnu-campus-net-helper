//! 系统托盘图标管理：
//! - 使用柔和圆角矩形 Wi-Fi 状态图标，随监控状态实时刷新
//! - 左键单击弹出竖版快捷浮窗（`tray_flyout.rs`），左键双击唤起完整主窗口，右键弹出快捷菜单（`tray_menu.rs`）

use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::models::{MonitorSnapshot, MonitorState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    ToggleFlyout,
    OpenMenu,
    Show,
    OpenAccountSettings,
    OpenUpdate,
    SwitchAccount(String),
    KickDevice { ip: String, is_local: bool },
    Start,
    Pause,
    Check,
    OpenLog,
    Exit,
}

pub struct TrayUi {
    icon: TrayIcon,
    last_key: Option<(MonitorState, u32, String)>,
}

impl TrayUi {
    pub fn new() -> Result<Self, String> {
        let size = native_tray_icon_size();
        let icon = TrayIconBuilder::new()
            .with_icon(state_icon(MonitorState::Paused, size)?)
            .with_tooltip(format!("{}：启动中", crate::APP_NAME))
            .build()
            .map_err(|err| format!("创建托盘图标失败：{err}"))?;

        Ok(Self {
            icon,
            last_key: None,
        })
    }

    /// 状态、系统托盘物理像素尺寸或文案变化时才刷新托盘图标与悬停提示，避免无谓重绘。
    pub fn apply(&mut self, snapshot: &MonitorSnapshot) {
        let size = native_tray_icon_size();
        let key = (snapshot.state, size, snapshot.message.clone());
        if self.last_key.as_ref() == Some(&key) {
            return;
        }
        self.last_key = Some(key);
        let title = state_title(snapshot.state);
        if let Ok(icon) = state_icon(snapshot.state, size) {
            let _ = self.icon.set_icon(Some(icon));
        }
        let tooltip = format!("{}：{title}", crate::APP_NAME);
        let _ = self.icon.set_tooltip(Some(tooltip.as_str()));
    }

    /// 后台零轮询监听托盘图标点击：
    /// - 左键单击：弹出/收起竖版快捷浮窗
    /// - 左键双击：直接唤起完整主窗口
    /// - 右键松开：弹出自定义 Windows 11 Fluent 现代菜单
    pub fn start_event_listener<F>(&self, on_action: F)
    where
        F: Fn(TrayAction) + Send + Sync + 'static,
    {
        let _ = std::thread::Builder::new()
            .name("tray-icon-listener".to_string())
            .spawn(move || {
                let rx = TrayIconEvent::receiver();
                while let Ok(event) = rx.recv() {
                    match event {
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } => {
                            on_action(TrayAction::ToggleFlyout);
                        }
                        TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        } => {
                            on_action(TrayAction::Show);
                        }
                        TrayIconEvent::Click {
                            button: MouseButton::Right,
                            button_state: MouseButtonState::Up,
                            ..
                        } => {
                            on_action(TrayAction::OpenMenu);
                        }
                        _ => {}
                    }
                }
            });
    }
}

fn state_title(state: MonitorState) -> &'static str {
    match state {
        MonitorState::Online => "网络正常",
        MonitorState::Offline => "连接异常",
        MonitorState::Suspect => "正在复核",
        MonitorState::Authenticating => "正在登录",
        MonitorState::Verifying => "正在验证",
        MonitorState::Checking => "检测中",
        MonitorState::Paused => "已暂停",
        MonitorState::Stopped => "已停止",
    }
}

fn native_tray_icon_size() -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    let raw = unsafe { GetSystemMetrics(SM_CXSMICON) };
    if (16..=128).contains(&raw) {
        raw as u32
    } else {
        32
    }
}

/// 按当前系统 DPI 物理像素尺寸直出满幅圆角方框 5 态托盘图标（避免系统二次缩放发虚或偏小）。
fn state_icon(state: MonitorState, size: u32) -> Result<Icon, String> {
    let rgba = crate::app_icon::render_icon_rgba(Some(state), size);
    Icon::from_rgba(rgba, size, size).map_err(|err| format!("生成托盘图标失败：{err}"))
}
