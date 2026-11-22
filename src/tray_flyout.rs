//! 现代 Windows 11 Fluent 风格托盘左键快捷浮窗（纯 Win32 GDI + DWM 原生圆角与阴影）。
//!
//! 特点：
//! - 零 OpenGL、零额外字库内存占用，保持轻守护进程 ~5 MB 内存与 < 5ms 瞬时弹出
//! - 竖长方形一屏融合三大核心信息：精简「账号流量」（运营商/校园网可切换）、精简「在线设备」、微缩「连接稳定性」（7h/24h 可切换）
//! - 底部提供「立即检测」「暂停/开始守护」「打开主界面 →」一键操作
//! - 点击浮窗外、按 Esc 或再次单击托盘图标自动收起，自动跟随浅色/深色主题与 Per-Monitor DPI

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, Ellipse, EndPaint, FillRect,
    GetMonitorInfoW, InvalidateRect, LineTo, MonitorFromPoint, MoveToEx, RoundRect, SelectObject,
    SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE,
    DT_VCENTER, FW_BOLD, FW_NORMAL, MONITORINFO, MONITOR_DEFAULTTONEAREST, NULL_PEN,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetWindowLongPtrW,
    IsWindowVisible, LoadCursorW, RegisterClassW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, CS_DROPSHADOW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HWND_TOPMOST,
    IDC_ARROW, IDC_HAND, SWP_NOACTIVATE, SW_HIDE, SW_SHOW, WA_INACTIVE, WM_ACTIVATE, WM_ERASEBKGND,
    WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::metrics_store::{
    build_health_track, compute_stats, filter_range, outages_in_range, MetricsStore,
    TelemetryPoint, TimeRange, TrackState,
};
use crate::models::{
    DisconnectReason, MonitorSnapshot, MonitorState, OnlineDevice, SelfServiceState,
    SelfServiceStatus, TrafficSummary,
};
use crate::tray::TrayAction;

const FLYOUT_CLASS_NAME: &str = "GXNUGuardFluentTrayFlyout";
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;
const DWMWCP_ROUND: u32 = 2;

const WM_MOUSELEAVE_MSG: u32 = 0x02A3;
const TME_LEAVE_FLAG: u32 = 0x0000_0002;
const VK_ESCAPE: u16 = 0x1B;

#[repr(C)]
struct TrackMouseEventStruct {
    cb_size: u32,
    dw_flags: u32,
    hwnd_track: HWND,
    dw_hover_time: u32,
}

#[link(name = "user32")]
extern "system" {
    fn GetDpiForWindow(hwnd: HWND) -> u32;
    fn TrackMouseEvent(lpeventtrack: *mut TrackMouseEventStruct) -> i32;
}

/// 浮窗共享只读数据源（直接复用守护进程内存状态，零额外拷贝或轮询）。
pub struct FlyoutDataSources {
    pub app_dir: PathBuf,
    pub latest_snapshot: Arc<Mutex<Option<MonitorSnapshot>>>,
    pub online_since_unix: Arc<Mutex<Option<i64>>>,
    pub latest_traffic: Arc<Mutex<TrafficSummary>>,
    pub latest_self_service: Arc<Mutex<SelfServiceState>>,
    pub active_account_label: Arc<Mutex<String>>,
    pub active_carrier: Arc<Mutex<String>>,
    pub metrics_store: Arc<MetricsStore>,
}

#[derive(Clone, Copy)]
struct FlyoutPalette {
    dark: bool,
    bg: COLORREF,
    subtle: COLORREF,
    border: COLORREF,
    divider: COLORREF,
    text: COLORREF,
    text_sec: COLORREF,
    text_muted: COLORREF,
    accent: COLORREF,
    accent_hover: COLORREF,
    accent_soft: COLORREF,
    accent_border: COLORREF,
    ok: COLORREF,
    ok_soft: COLORREF,
    ok_border: COLORREF,
    warn: COLORREF,
    warn_soft: COLORREF,
    warn_border: COLORREF,
    danger: COLORREF,
    danger_soft: COLORREF,
    danger_border: COLORREF,
    neutral: COLORREF,
    neutral_soft: COLORREF,
    neutral_border: COLORREF,
    pill_bg: COLORREF,
    pill_border: COLORREF,
    btn_bg: COLORREF,
    btn_hover: COLORREF,
    btn_border: COLORREF,
    idle_pill: COLORREF,
}

impl FlyoutPalette {
    fn load(app_dir: &Path) -> Self {
        Self::of(detect_dark_mode(app_dir))
    }

    fn of(dark: bool) -> Self {
        if dark {
            Self {
                dark: true,
                bg: rgb(0x1B, 0x1F, 0x27),
                subtle: rgb(0x15, 0x18, 0x20),
                border: rgb(0x2F, 0x36, 0x44),
                divider: rgb(0x28, 0x2E, 0x3A),
                text: rgb(0xF1, 0xF5, 0xF9),
                text_sec: rgb(0xB4, 0xBE, 0xCE),
                text_muted: rgb(0x8B, 0x95, 0xA7),
                accent: rgb(0x3B, 0x82, 0xF6),
                accent_hover: rgb(0x60, 0xA5, 0xFA),
                accent_soft: rgb(0x1C, 0x2A, 0x44),
                accent_border: rgb(0x2A, 0x45, 0x75),
                ok: rgb(0x10, 0xB9, 0x81),
                ok_soft: rgb(0x11, 0x2D, 0x26),
                ok_border: rgb(0x19, 0x52, 0x42),
                warn: rgb(0xF5, 0x9E, 0x0B),
                warn_soft: rgb(0x31, 0x25, 0x13),
                warn_border: rgb(0x5C, 0x41, 0x18),
                danger: rgb(0xF8, 0x71, 0x71),
                danger_soft: rgb(0x33, 0x1A, 0x1D),
                danger_border: rgb(0x5E, 0x28, 0x2E),
                neutral: rgb(0x94, 0xA3, 0xB8),
                neutral_soft: rgb(0x25, 0x29, 0x32),
                neutral_border: rgb(0x38, 0x3E, 0x4B),
                pill_bg: rgb(0x23, 0x28, 0x33),
                pill_border: rgb(0x34, 0x3B, 0x49),
                btn_bg: rgb(0x23, 0x28, 0x33),
                btn_hover: rgb(0x2B, 0x32, 0x40),
                btn_border: rgb(0x34, 0x3B, 0x49),
                idle_pill: rgb(0x2D, 0x34, 0x42),
            }
        } else {
            Self {
                dark: false,
                bg: rgb(0xFF, 0xFF, 0xFF),
                subtle: rgb(0xF5, 0xF7, 0xFA),
                border: rgb(0xDC, 0xE2, 0xEC),
                divider: rgb(0xE6, 0xEB, 0xF2),
                text: rgb(0x11, 0x18, 0x27),
                text_sec: rgb(0x4B, 0x55, 0x63),
                text_muted: rgb(0x6B, 0x76, 0x88),
                accent: rgb(0x25, 0x63, 0xEB),
                accent_hover: rgb(0x1D, 0x4E, 0xD8),
                accent_soft: rgb(0xEB, 0xF3, 0xFF),
                accent_border: rgb(0xBF, 0xDB, 0xFE),
                ok: rgb(0x05, 0x96, 0x69),
                ok_soft: rgb(0xEC, 0xFD, 0xF5),
                ok_border: rgb(0xA7, 0xF3, 0xD0),
                warn: rgb(0xD9, 0x77, 0x06),
                warn_soft: rgb(0xFF, 0xFB, 0xEB),
                warn_border: rgb(0xFD, 0xE6, 0x8A),
                danger: rgb(0xDC, 0x26, 0x26),
                danger_soft: rgb(0xFE, 0xF2, 0xF2),
                danger_border: rgb(0xFE, 0xCA, 0xCA),
                neutral: rgb(0x64, 0x74, 0x8B),
                neutral_soft: rgb(0xF1, 0xF5, 0xF9),
                neutral_border: rgb(0xCB, 0xD5, 0xE1),
                pill_bg: rgb(0xFF, 0xFF, 0xFF),
                pill_border: rgb(0xDC, 0xE2, 0xEC),
                btn_bg: rgb(0xF8, 0xFA, 0xFC),
                btn_hover: rgb(0xEE, 0xF2, 0xF7),
                btn_border: rgb(0xDC, 0xE2, 0xEC),
                idle_pill: rgb(0xDC, 0xE2, 0xEB),
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FlyoutTrafficMode {
    Carrier,
    Campus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HitTarget {
    TrafficCarrier,
    TrafficCampus,
    Range7h,
    Range24h,
    CheckNow,
    ToggleMonitor,
    OpenMain,
}

#[derive(Clone, Copy)]
struct HitBox {
    target: HitTarget,
    rect: RECT,
}

#[derive(Clone)]
struct FlyoutDeviceView {
    terminal: String,
    ip: String,
    is_local: bool,
    is_conflict: bool,
    duration: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    Online,
    Outage,
    Offline,
    Idle,
}

struct FlyoutModel {
    // Header
    status_title: &'static str,
    status_dot: COLORREF,
    status_soft: COLORREF,
    status_border: COLORREF,
    online_badge: String,
    header_meta: String,
    monitoring_enabled: bool,

    // Traffic
    has_portal_traffic: bool,
    traffic_hero: String,
    traffic_sub: String,
    traffic_sub_color: COLORREF,
    traffic_sub_bold: bool,
    traffic_dual_bar: bool,
    traffic_bar_ratio: f32,
    traffic_bar_color: COLORREF,
    traffic_bar_has_left: bool,
    traffic_bar_has_right: bool,
    traffic_foot_left: String,
    traffic_foot_left_dot: Option<COLORREF>,
    traffic_foot_right: String,
    traffic_foot_right_dot: Option<COLORREF>,

    // Online devices
    devices_badge: String,
    devices_badge_fg: COLORREF,
    devices_badge_bg: COLORREF,
    devices_badge_bd: COLORREF,
    devices: Vec<FlyoutDeviceView>,
    devices_overflow_extra: usize,
    devices_empty_hint: String,

    // Stability
    uptime_text: String,
    uptime_color: COLORREF,
    outage_summary: String,
    slots: Vec<SlotKind>,
    ruler_left: String,
}

struct FlyoutState {
    sources: FlyoutDataSources,
    on_action: Arc<dyn Fn(TrayAction) + Send + Sync>,
    palette: FlyoutPalette,
    traffic_mode: FlyoutTrafficMode,
    traffic_user_override: bool,
    last_carrier: String,
    range: TimeRange,
    dpi: u32,
    width_px: i32,
    height_px: i32,
    hovered: Option<HitTarget>,
    tracking_mouse: bool,
    hits: Vec<HitBox>,
    model: FlyoutModel,
    telemetry_cache: Vec<TelemetryPoint>,
    last_deactivated: Option<Instant>,
}

pub struct FluentTrayFlyout {
    hwnd: HWND,
    state_ptr: *mut FlyoutState,
}

impl FluentTrayFlyout {
    pub fn new<F>(sources: FlyoutDataSources, on_action: F) -> Result<Self, String>
    where
        F: Fn(TrayAction) + Send + Sync + 'static,
    {
        unsafe {
            let hinstance = GetModuleHandleW(null());
            let class_w = to_wide(FLYOUT_CLASS_NAME);
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW | CS_DROPSHADOW,
                lpfnWndProc: Some(flyout_wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: null_mut(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: class_w.as_ptr(),
            };
            RegisterClassW(&wc);

            let palette = FlyoutPalette::load(&sources.app_dir);
            let init_carrier = sources
                .active_carrier
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| "2".to_string());
            let init_mode = if init_carrier == "0" {
                FlyoutTrafficMode::Campus
            } else {
                FlyoutTrafficMode::Carrier
            };
            let placeholder_model = build_empty_model(&palette);

            let boxed = Box::new(FlyoutState {
                sources,
                on_action: Arc::new(on_action),
                palette,
                traffic_mode: init_mode,
                traffic_user_override: false,
                last_carrier: init_carrier,
                range: TimeRange::H7,
                dpi: 96,
                width_px: 326,
                height_px: 390,
                hovered: None,
                tracking_mouse: false,
                hits: Vec::new(),
                model: placeholder_model,
                telemetry_cache: Vec::new(),
                last_deactivated: None,
            });
            let state_ptr = Box::into_raw(boxed);

            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class_w.as_ptr(),
                class_w.as_ptr(),
                WS_POPUP,
                0,
                0,
                326,
                390,
                null_mut(),
                null_mut(),
                hinstance,
                null_mut(),
            );
            if hwnd.is_null() {
                drop(Box::from_raw(state_ptr));
                return Err("创建托盘快捷浮窗失败".to_string());
            }

            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);

            let corner_pref = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner_pref as *const u32 as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );

            Ok(Self { hwnd, state_ptr })
        }
    }

    /// 左键单击托盘图标：若浮窗已展示或刚因点击图标而失焦收起（< 280ms），则收起不重复弹出；否则弹出刷新。
    pub fn toggle_at_cursor(&mut self) -> bool {
        unsafe {
            let state = &mut *self.state_ptr;
            if IsWindowVisible(self.hwnd) != 0 {
                ShowWindow(self.hwnd, SW_HIDE);
                return false;
            }
            if state
                .last_deactivated
                .is_some_and(|t| t.elapsed() < Duration::from_millis(280))
            {
                state.last_deactivated = None;
                return false;
            }
            self.show_at_cursor_inner();
            true
        }
    }

    /// 若浮窗当前处于可见状态，则即时同步最新状态并重绘（例如点击「立即检测」或自助服务返回后）。
    pub fn refresh_if_visible(&mut self) {
        unsafe {
            if IsWindowVisible(self.hwnd) == 0 {
                return;
            }
            let state = &mut *self.state_ptr;
            refresh_flyout_state(state, false);
            let old_h = state.height_px;
            let new_h = compute_height_px(state);
            if new_h != old_h {
                state.height_px = new_h;
                position_flyout_window(self.hwnd, state, false);
            }
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    pub fn hide(&mut self) {
        unsafe {
            if IsWindowVisible(self.hwnd) != 0 {
                ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    unsafe fn show_at_cursor_inner(&mut self) {
        let state = &mut *self.state_ptr;
        state.palette = FlyoutPalette::load(&state.sources.app_dir);
        state.hovered = None;
        state.tracking_mouse = false;
        refresh_flyout_state(state, true);

        let dark_val: u32 = if state.palette.dark { 1 } else { 0 };
        let _ = DwmSetWindowAttribute(
            self.hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_val as *const u32 as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );
        let border_color = state.palette.border;
        let _ = DwmSetWindowAttribute(
            self.hwnd,
            DWMWA_BORDER_COLOR,
            &border_color as *const COLORREF as *const c_void,
            std::mem::size_of::<COLORREF>() as u32,
        );

        let dpi = GetDpiForWindow(self.hwnd).max(96);
        state.dpi = dpi;
        state.width_px = scale(326, dpi);
        state.height_px = compute_height_px(state);

        position_flyout_window(self.hwnd, state, true);
        ShowWindow(self.hwnd, SW_SHOW);
        SetForegroundWindow(self.hwnd);
        InvalidateRect(self.hwnd, null(), 0);
    }
}

impl Drop for FluentTrayFlyout {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.is_null() {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                DestroyWindow(self.hwnd);
                self.hwnd = null_mut();
            }
            if !self.state_ptr.is_null() {
                drop(Box::from_raw(self.state_ptr));
                self.state_ptr = null_mut();
            }
        }
    }
}

unsafe fn position_flyout_window(hwnd: HWND, state: &FlyoutState, use_cursor: bool) {
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);

    let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let (work_left, work_top, work_right, work_bottom) = if GetMonitorInfoW(monitor, &mut mi) != 0 {
        (
            mi.rcWork.left,
            mi.rcWork.top,
            mi.rcWork.right,
            mi.rcWork.bottom,
        )
    } else {
        (0, 0, 1920, 1040)
    };

    let win_w = state.width_px;
    let win_h = state.height_px;
    let margin = scale(10, state.dpi);

    let (mut x, mut y) = if use_cursor {
        (pt.x - win_w / 2, work_bottom - win_h - margin)
    } else {
        (work_right - win_w - margin, work_bottom - win_h - margin)
    };
    if x + win_w > work_right - margin {
        x = work_right - win_w - margin;
    }
    if x < work_left + margin {
        x = work_left + margin;
    }
    if y < work_top + margin {
        y = work_top + margin;
    }
    if y + win_h > work_bottom - margin {
        y = work_bottom - win_h - margin;
    }

    SetWindowPos(hwnd, HWND_TOPMOST, x, y, win_w, win_h, SWP_NOACTIVATE);
}

fn compute_height_px(state: &FlyoutState) -> i32 {
    let d = state.dpi;
    let dev_rows = state.model.devices.len().clamp(1, 3) as i32;
    let dev_list_h = dev_rows * 26 + (dev_rows - 1) * 5;
    let extra_h = if state.model.devices_overflow_extra > 0 {
        16
    } else {
        0
    };
    // top(13) + hdr(38) + sep(17) + traffic(84) + sep(17) + dev_hdr(22) + gap(6) + dev_list + extra
    // + sep(17) + stab(56) + sep(17) + foot(28) + bot(12)
    let logical_h = 13 + 38 + 17 + 84 + 17 + 22 + 6 + dev_list_h + extra_h + 17 + 56 + 17 + 28 + 12;
    scale(logical_h, d)
}

fn refresh_flyout_state(state: &mut FlyoutState, reload_telemetry: bool) {
    let snap = state
        .sources
        .latest_snapshot
        .lock()
        .ok()
        .and_then(|g| g.clone());
    let online_since = state.sources.online_since_unix.lock().ok().and_then(|g| *g);
    let local_traffic = state
        .sources
        .latest_traffic
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let ss = state
        .sources
        .latest_self_service
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let acct_label = state
        .sources
        .active_account_label
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let carrier = state
        .sources
        .active_carrier
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "2".to_string());

    if carrier != state.last_carrier {
        state.last_carrier = carrier.clone();
        state.traffic_user_override = false;
    }
    if !state.traffic_user_override {
        state.traffic_mode = if carrier == "0" {
            FlyoutTrafficMode::Campus
        } else {
            FlyoutTrafficMode::Carrier
        };
    }

    if reload_telemetry || state.telemetry_cache.is_empty() {
        state.telemetry_cache = MetricsStore::history(&state.sources.app_dir);
    }

    state.model = build_flyout_model(
        &state.palette,
        snap.as_ref(),
        online_since,
        &local_traffic,
        &ss,
        &acct_label,
        &carrier,
        state.traffic_mode,
        &state.telemetry_cache,
        state.range,
    );
}

#[allow(clippy::too_many_arguments)]
fn build_flyout_model(
    p: &FlyoutPalette,
    snap: Option<&MonitorSnapshot>,
    online_since_unix: Option<i64>,
    local_traffic: &TrafficSummary,
    ss: &SelfServiceState,
    acct_label: &str,
    carrier: &str,
    traffic_mode: FlyoutTrafficMode,
    telemetry: &[TelemetryPoint],
    range: TimeRange,
) -> FlyoutModel {
    let now_unix = Local::now().timestamp();
    let mon_state = snap.map(|s| s.state).unwrap_or(MonitorState::Paused);
    let monitoring_enabled = snap.is_some_and(|s| s.monitoring_enabled);
    let login_rejected = snap.and_then(|s| s.reason) == Some(DisconnectReason::LoginRejected);

    let (status_title, status_dot, status_soft, status_border) = match mon_state {
        MonitorState::Online => ("校园网已连接", p.ok, p.ok_soft, p.ok_border),
        MonitorState::Checking => ("正在检测网络", p.accent, p.accent_soft, p.accent_border),
        MonitorState::Authenticating | MonitorState::Verifying | MonitorState::Suspect => {
            ("正在自动恢复", p.warn, p.warn_soft, p.warn_border)
        }
        MonitorState::Offline => ("网络连接异常", p.danger, p.danger_soft, p.danger_border),
        MonitorState::Paused | MonitorState::Stopped => {
            ("守护已暂停", p.neutral, p.neutral_soft, p.neutral_border)
        }
    };

    let campus_ip = snap
        .map(|s| s.metrics.effective_campus_ip().to_string())
        .filter(|ip| !ip.is_empty())
        .or_else(|| {
            if mon_state == MonitorState::Online {
                ss.data.as_ref().and_then(|d| {
                    (d.online_devices.len() == 1).then(|| d.online_devices[0].ip.clone())
                })
            } else {
                None
            }
        })
        .unwrap_or_default();

    let online_badge = if mon_state == MonitorState::Online {
        let portal_since = ss
            .data
            .as_ref()
            .and_then(|d| d.matched_local_since_unix(&campus_ip));
        if let Some(ts) = portal_since.or(online_since_unix) {
            format!(
                "在线 {}",
                format_uptime_compact((now_unix - ts).max(0) as u64)
            )
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let mut meta_parts: Vec<String> = Vec::new();
    let acct = acct_label.trim();
    if !acct.is_empty() && acct != "未配置账号" {
        meta_parts.push(truncate_chars(acct, 10));
    }
    meta_parts.push(carrier_name(carrier).to_string());
    if !campus_ip.is_empty() {
        meta_parts.push(campus_ip.clone());
    }
    if let Some(s) = snap {
        if mon_state == MonitorState::Online && s.metrics.last_check_ms > 0 {
            meta_parts.push(format_ms(s.metrics.last_check_ms));
        }
    }
    let header_meta = if mon_state == MonitorState::Online || meta_parts.len() >= 2 {
        meta_parts.join(" · ")
    } else {
        snap.map(|s| s.message.clone())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "后台轻量守护中".to_string())
    };

    // ── Section 1: 账号流量 ──────────────────────────────────────────
    let has_portal_traffic = ss.data.is_some();
    let (
        traffic_hero,
        traffic_sub,
        traffic_sub_color,
        traffic_sub_bold,
        traffic_dual_bar,
        traffic_bar_ratio,
        traffic_bar_color,
        traffic_bar_has_left,
        traffic_bar_has_right,
        traffic_foot_left,
        traffic_foot_left_dot,
        traffic_foot_right,
        traffic_foot_right_dot,
    ) = if let Some(data) = &ss.data {
        let t = &data.traffic;
        match traffic_mode {
            FlyoutTrafficMode::Carrier => {
                let down = t.month_down_mb.max(0.0);
                let up = t.month_up_mb.max(0.0);
                let total = down + up;
                let dl_frac = if total > 0.001 {
                    (down / total) as f32
                } else {
                    0.0
                };
                let dl_pct = (dl_frac * 100.0).round() as u32;
                let ul_pct = 100u32.saturating_sub(dl_pct);
                (
                    format_mb(total),
                    "本月实际 · 运营商不限量".to_string(),
                    p.text_muted,
                    false,
                    true,
                    dl_frac,
                    p.accent,
                    down > 0.0,
                    up > 0.0,
                    format!("下行 {} ({dl_pct}%)", format_mb(down)),
                    Some(p.accent),
                    format!("上行 {} ({ul_pct}%)", format_mb(up)),
                    Some(p.ok),
                )
            }
            FlyoutTrafficMode::Campus => {
                let rem = t.campus_remaining_mb.max(0.0);
                let used = t.campus_used_mb.max(0.0);
                let total = t.campus_total_mb().max(0.001);
                let rem_ratio = (rem / total).clamp(0.0, 1.0) as f32;
                let depleted = rem <= 0.1;
                let col = if depleted || rem_ratio <= 0.05 {
                    p.danger
                } else if rem_ratio <= 0.20 {
                    p.warn
                } else {
                    p.ok
                };
                let sub = if depleted {
                    "额度已用尽".to_string()
                } else if used <= 0.01 {
                    "剩余 100%".to_string()
                } else {
                    format!("剩余 {:.1}%", (rem_ratio * 100.0).min(99.9))
                };
                let right_note = match (
                    !t.user_group_name.is_empty(),
                    !t.plan_description.is_empty(),
                ) {
                    (true, true) => format!("{} · {}", t.user_group_name, t.plan_description),
                    (true, false) => t.user_group_name.clone(),
                    (false, true) => t.plan_description.clone(),
                    (false, false) => "仅校园网计费".to_string(),
                };
                (
                    if depleted {
                        "已用尽".to_string()
                    } else {
                        format_mb(rem)
                    },
                    sub,
                    col,
                    true,
                    false,
                    if depleted { 0.0 } else { rem_ratio },
                    col,
                    !depleted && rem_ratio > 0.0,
                    false,
                    format!("本月已用 {}", format_mb(used)),
                    None,
                    right_note,
                    None,
                )
            }
        }
    } else {
        let rx = local_traffic.today_rx_bytes;
        let tx = local_traffic.today_tx_bytes;
        let total = rx.saturating_add(tx);
        let dl_frac = if total > 0 {
            rx as f32 / total as f32
        } else {
            0.0
        };
        let sub = match ss.status {
            SelfServiceStatus::Refreshing => "正在同步自助服务…".to_string(),
            SelfServiceStatus::CaptchaCooldown => "门户需验证码 · 本地参考".to_string(),
            _ => "本机今日（网卡参考）".to_string(),
        };
        (
            format_bytes(total),
            sub,
            p.text_muted,
            false,
            true,
            dl_frac,
            p.accent,
            rx > 0,
            tx > 0,
            format!("下载 {}", format_bytes(rx)),
            Some(p.accent),
            format!("上传 {}", format_bytes(tx)),
            Some(p.ok),
        )
    };

    // ── Section 2: 在线设备 ──────────────────────────────────────────
    let (
        devices_badge,
        devices_badge_fg,
        devices_badge_bg,
        devices_badge_bd,
        devices,
        overflow,
        empty_hint,
    ) = if let Some(data) = &ss.data {
        let mut devs: Vec<(&OnlineDevice, bool)> = data
            .online_devices
            .iter()
            .map(|dev| {
                let is_local = (!campus_ip.is_empty() && dev.ip == campus_ip)
                    || (campus_ip.is_empty()
                        && mon_state == MonitorState::Online
                        && data.online_devices.len() == 1);
                (dev, is_local)
            })
            .collect();
        devs.sort_by_key(|(_, is_local)| !*is_local);

        let count = devs.len();
        let at_limit = data.max_online_devices.is_some_and(|m| count >= m as usize);
        let badge_txt = match data.max_online_devices {
            Some(max) => format!("{count} / {max} 台在线"),
            None => format!("{count} 台在线"),
        };
        let (fg, bg, bd) = if login_rejected && count > 0 {
            (p.danger, p.danger_soft, p.danger_border)
        } else if at_limit {
            (p.warn, p.warn_soft, p.warn_border)
        } else if count > 0 {
            (p.accent, p.accent_soft, p.accent_border)
        } else {
            (p.neutral, p.neutral_soft, p.neutral_border)
        };

        let overflow = count.saturating_sub(3);
        let list = devs
            .into_iter()
            .take(3)
            .map(|(dev, is_local)| {
                let secs = if let Some(u) = dev.use_time_secs {
                    u.saturating_add((now_unix - data.fetched_at_unix).max(0) as u64)
                } else if let Some(ts) = dev.login_unix {
                    (now_unix - ts).max(0) as u64
                } else {
                    0
                };
                FlyoutDeviceView {
                    terminal: if dev.terminal_type.is_empty() {
                        "终端".to_string()
                    } else {
                        dev.terminal_type.clone()
                    },
                    ip: dev.ip.clone(),
                    is_local,
                    is_conflict: !is_local && (at_limit || login_rejected),
                    duration: if secs > 0 {
                        format_duration_short(secs)
                    } else {
                        "在线".to_string()
                    },
                }
            })
            .collect();

        (
            badge_txt,
            fg,
            bg,
            bd,
            list,
            overflow,
            "当前账号暂无在线终端会话".to_string(),
        )
    } else {
        let hint = if !campus_ip.is_empty() {
            format!("本机 {campus_ip} · 等待自助服务同步")
        } else if !ss.message.is_empty() {
            truncate_chars(&ss.message, 24)
        } else {
            "连入校园网后自动同步在线终端".to_string()
        };
        (
            "未同步".to_string(),
            p.neutral,
            p.neutral_soft,
            p.neutral_border,
            Vec::new(),
            0,
            hint,
        )
    };

    // ── Section 3: 连接稳定性（36 枚竖向细胶囊） ─────────────────────
    let points = filter_range(telemetry, now_unix, range);
    let stats = compute_stats(&points);
    let outages = outages_in_range(telemetry, now_unix, range);
    let track = build_health_track(telemetry, now_unix, range);
    let has_outages = !outages.events.is_empty();

    let (uptime_text, uptime_color) = if let Some(pct) = stats.uptime_pct {
        let text = if !has_outages && pct >= 99.95 {
            "100%".to_string()
        } else {
            format!("{:.1}%", if has_outages { pct.min(99.9) } else { pct })
        };
        let col = if pct >= 98.0 {
            p.text
        } else if pct >= 90.0 {
            p.warn
        } else {
            p.danger
        };
        (text, col)
    } else {
        ("—".to_string(), p.text_muted)
    };

    let outage_summary = if has_outages {
        format!("{} 次断联", outages.events.len())
    } else if stats.check_count > 0 {
        "全程稳定".to_string()
    } else {
        "等待采样".to_string()
    };

    const SLOTS: usize = 36;
    let window_secs = range.secs().max(60);
    let window_start = now_unix - window_secs;
    let slot_secs = window_secs as f64 / SLOTS as f64;
    let mut slots = Vec::with_capacity(SLOTS);
    for idx in 0..SLOTS {
        let s_start = window_start + (idx as f64 * slot_secs).round() as i64;
        let s_end = window_start + ((idx + 1) as f64 * slot_secs).round() as i64;
        let mut has_outage = false;
        let mut has_unrecovered = false;
        for ev in &outages.events {
            let ev_start = ev.start.ts;
            let ev_end = ev.end_ts.unwrap_or(now_unix).max(ev_start + 1);
            if ev_start < s_end && ev_end > s_start {
                has_outage = true;
                if !ev.recovered {
                    has_unrecovered = true;
                }
            }
        }
        if has_outage {
            slots.push(if has_unrecovered {
                SlotKind::Offline
            } else {
                SlotKind::Outage
            });
            continue;
        }
        let online_track = track.iter().any(|seg| {
            seg.state == TrackState::Online && seg.start_ts < s_end && seg.end_ts > s_start
        });
        let online_pt = points
            .iter()
            .any(|pt| pt.event == "check" && pt.online && pt.ts >= s_start && pt.ts < s_end);
        slots.push(if online_track || online_pt {
            SlotKind::Online
        } else {
            SlotKind::Idle
        });
    }

    FlyoutModel {
        status_title,
        status_dot,
        status_soft,
        status_border,
        online_badge,
        header_meta,
        monitoring_enabled,
        has_portal_traffic,
        traffic_hero,
        traffic_sub,
        traffic_sub_color,
        traffic_sub_bold,
        traffic_dual_bar,
        traffic_bar_ratio,
        traffic_bar_color,
        traffic_bar_has_left,
        traffic_bar_has_right,
        traffic_foot_left,
        traffic_foot_left_dot,
        traffic_foot_right,
        traffic_foot_right_dot,
        devices_badge,
        devices_badge_fg,
        devices_badge_bg,
        devices_badge_bd,
        devices,
        devices_overflow_extra: overflow,
        devices_empty_hint: empty_hint,
        uptime_text,
        uptime_color,
        outage_summary,
        slots,
        ruler_left: format_hm(window_start),
    }
}

fn build_empty_model(p: &FlyoutPalette) -> FlyoutModel {
    FlyoutModel {
        status_title: "正在初始化",
        status_dot: p.neutral,
        status_soft: p.neutral_soft,
        status_border: p.neutral_border,
        online_badge: String::new(),
        header_meta: "后台轻量守护中".to_string(),
        monitoring_enabled: true,
        has_portal_traffic: false,
        traffic_hero: "0 MB".to_string(),
        traffic_sub: "等待同步".to_string(),
        traffic_sub_color: p.text_muted,
        traffic_sub_bold: false,
        traffic_dual_bar: false,
        traffic_bar_ratio: 0.0,
        traffic_bar_color: p.accent,
        traffic_bar_has_left: false,
        traffic_bar_has_right: false,
        traffic_foot_left: String::new(),
        traffic_foot_left_dot: None,
        traffic_foot_right: String::new(),
        traffic_foot_right_dot: None,
        devices_badge: "未同步".to_string(),
        devices_badge_fg: p.neutral,
        devices_badge_bg: p.neutral_soft,
        devices_badge_bd: p.neutral_border,
        devices: Vec::new(),
        devices_overflow_extra: 0,
        devices_empty_hint: "连入校园网后自动同步".to_string(),
        uptime_text: "—".to_string(),
        uptime_color: p.text_muted,
        outage_summary: "等待采样".to_string(),
        slots: vec![SlotKind::Idle; 36],
        ruler_left: "--:--".to_string(),
    }
}

unsafe extern "system" fn flyout_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut FlyoutState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = &mut *state_ptr;

    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint_flyout(hwnd, state);
            0
        }
        WM_MOUSEMOVE => {
            if !state.tracking_mouse {
                let mut tme = TrackMouseEventStruct {
                    cb_size: std::mem::size_of::<TrackMouseEventStruct>() as u32,
                    dw_flags: TME_LEAVE_FLAG,
                    hwnd_track: hwnd,
                    dw_hover_time: 0,
                };
                TrackMouseEvent(&mut tme);
                state.tracking_mouse = true;
            }
            let x = (lparam as i32 & 0xFFFF) as i16 as i32;
            let y = ((lparam as i32 >> 16) & 0xFFFF) as i16 as i32;
            let next = hit_test(&state.hits, x, y);
            if next != state.hovered {
                state.hovered = next;
                InvalidateRect(hwnd, null(), 0);
            }
            let cursor = if next.is_some() { IDC_HAND } else { IDC_ARROW };
            windows_sys::Win32::UI::WindowsAndMessaging::SetCursor(LoadCursorW(null_mut(), cursor));
            0
        }
        WM_MOUSELEAVE_MSG => {
            state.tracking_mouse = false;
            if state.hovered.is_some() {
                state.hovered = None;
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_LBUTTONUP => {
            let x = (lparam as i32 & 0xFFFF) as i16 as i32;
            let y = ((lparam as i32 >> 16) & 0xFFFF) as i16 as i32;
            if let Some(target) = hit_test(&state.hits, x, y) {
                match target {
                    HitTarget::TrafficCarrier => {
                        state.traffic_mode = FlyoutTrafficMode::Carrier;
                        state.traffic_user_override = true;
                        refresh_flyout_state(state, false);
                        InvalidateRect(hwnd, null(), 0);
                    }
                    HitTarget::TrafficCampus => {
                        state.traffic_mode = FlyoutTrafficMode::Campus;
                        state.traffic_user_override = true;
                        refresh_flyout_state(state, false);
                        InvalidateRect(hwnd, null(), 0);
                    }
                    HitTarget::Range7h => {
                        state.range = TimeRange::H7;
                        refresh_flyout_state(state, false);
                        InvalidateRect(hwnd, null(), 0);
                    }
                    HitTarget::Range24h => {
                        state.range = TimeRange::H24;
                        refresh_flyout_state(state, false);
                        InvalidateRect(hwnd, null(), 0);
                    }
                    HitTarget::CheckNow => {
                        let cb = Arc::clone(&state.on_action);
                        cb(TrayAction::Check);
                    }
                    HitTarget::ToggleMonitor => {
                        let action = if state.model.monitoring_enabled {
                            TrayAction::Pause
                        } else {
                            TrayAction::Start
                        };
                        let cb = Arc::clone(&state.on_action);
                        cb(action);
                    }
                    HitTarget::OpenMain => {
                        ShowWindow(hwnd, SW_HIDE);
                        let cb = Arc::clone(&state.on_action);
                        cb(TrayAction::Show);
                    }
                }
            }
            0
        }
        WM_RBUTTONUP => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_KEYDOWN => {
            if (wparam as u16) == VK_ESCAPE {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_ACTIVATE => {
            if (wparam & 0xFFFF) as u32 == WA_INACTIVE {
                state.last_deactivated = Some(Instant::now());
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_KILLFOCUS => {
            state.last_deactivated = Some(Instant::now());
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn hit_test(hits: &[HitBox], x: i32, y: i32) -> Option<HitTarget> {
    hits.iter()
        .find(|h| x >= h.rect.left && x < h.rect.right && y >= h.rect.top && y < h.rect.bottom)
        .map(|h| h.target)
}

unsafe fn paint_flyout(hwnd: HWND, state: &mut FlyoutState) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    if hdc.is_null() {
        return;
    }

    let w = state.width_px.max(1);
    let h = state.height_px.max(1);

    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mem_dc, mem_bmp);

    draw_flyout_contents(mem_dc, state, w, h);

    BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
    SelectObject(mem_dc, old_bmp);
    DeleteObject(mem_bmp);
    DeleteDC(mem_dc);
    EndPaint(hwnd, &ps);
}

unsafe fn draw_flyout_contents(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    state: &mut FlyoutState,
    w: i32,
    h: i32,
) {
    let d = state.dpi;
    let p = state.palette;
    state.hits.clear();

    // 背景
    let bg_brush = CreateSolidBrush(p.bg);
    let full_rect = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    FillRect(hdc, &full_rect, bg_brush);
    DeleteObject(bg_brush);
    SetBkMode(hdc, TRANSPARENT as i32);

    let f_title = make_font(14, FW_BOLD, d);
    let f_sec = make_font(12, FW_BOLD, d);
    let f_hero = make_font(19, FW_BOLD, d);
    let f_ip = make_font(11, FW_BOLD, d);
    let f_body = make_font(11, FW_NORMAL, d);
    let f_bold_sm = make_font(10, FW_BOLD, d);
    let f_sm = make_font(10, FW_NORMAL, d);
    let f_xs = make_font(9, FW_NORMAL, d);

    let pad_x = scale(14, d);
    let left = pad_x;
    let right = w - pad_x;
    let content_w = right - left;
    let mut y = scale(13, d);

    // ── 1. 顶部状态头（38px） ─────────────────────────────────────────
    let top_row_h = scale(20, d);
    let dot_cy = y + top_row_h / 2;
    let dot_cx = left + scale(6, d);
    fill_circle(
        hdc,
        dot_cx,
        dot_cy,
        scale(7, d),
        p.bg,
        state.model.status_soft,
    );
    fill_circle(
        hdc,
        dot_cx,
        dot_cy,
        scale(4, d),
        state.model.status_dot,
        state.model.status_dot,
    );

    let title_x = left + scale(17, d);
    let title_w = measure_or_draw_text(
        hdc,
        f_title,
        state.model.status_title,
        RECT {
            left: title_x,
            top: y,
            right: right - scale(80, d),
            bottom: y + top_row_h,
        },
        p.text,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    if !state.model.online_badge.is_empty() {
        let bx = title_x + title_w + scale(8, d);
        draw_pill_badge(
            hdc,
            f_bold_sm,
            &state.model.online_badge,
            bx,
            dot_cy,
            scale(18, d),
            scale(12, d),
            state.model.status_dot,
            state.model.status_soft,
            state.model.status_border,
            d,
        );
    }

    let meta_y = y + scale(21, d);
    draw_text_line(
        hdc,
        f_sm,
        &state.model.header_meta,
        RECT {
            left,
            top: meta_y,
            right,
            bottom: meta_y + scale(16, d),
        },
        p.text_muted,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    y += scale(38, d);

    y = draw_divider(hdc, left, right, y, p.divider, d);

    // ── 2. 区块一：账号流量（84px） ───────────────────────────────────
    let sec_h = scale(22, d);
    draw_text_line(
        hdc,
        f_sec,
        "账号流量",
        RECT {
            left,
            top: y,
            right: left + scale(90, d),
            bottom: y + sec_h,
        },
        p.text,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    if state.model.has_portal_traffic {
        draw_segmented_pair(
            hdc,
            f_sm,
            f_bold_sm,
            right,
            y + sec_h / 2,
            ("运营商", HitTarget::TrafficCarrier),
            ("校园网", HitTarget::TrafficCampus),
            state.traffic_mode == FlyoutTrafficMode::Carrier,
            state.hovered,
            &mut state.hits,
            &p,
            d,
        );
    }
    y += scale(24, d);

    // Hero 数字 + 右侧简短说明
    let hero_h = scale(24, d);
    draw_text_line(
        hdc,
        f_hero,
        &state.model.traffic_hero,
        RECT {
            left,
            top: y,
            right: left + content_w * 55 / 100,
            bottom: y + hero_h,
        },
        if state.model.traffic_hero == "已用尽" {
            p.danger
        } else {
            p.text
        },
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    draw_text_line(
        hdc,
        if state.model.traffic_sub_bold {
            f_bold_sm
        } else {
            f_sm
        },
        &state.model.traffic_sub,
        RECT {
            left: left + content_w * 42 / 100,
            top: y + scale(4, d),
            right,
            bottom: y + hero_h,
        },
        state.model.traffic_sub_color,
        DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    y += scale(29, d);

    // 6px 细比例条
    let bar_h = scale(6, d);
    let bar_r = bar_h;
    fill_round_rect(
        hdc,
        RECT {
            left,
            top: y,
            right,
            bottom: y + bar_h,
        },
        bar_r,
        p.subtle,
        p.subtle,
    );
    if state.model.traffic_dual_bar {
        let has_l = state.model.traffic_bar_has_left;
        let has_r = state.model.traffic_bar_has_right;
        if has_l || has_r {
            let gap = if has_l && has_r { scale(2, d) } else { 0 };
            let usable = (content_w - gap).max(4);
            let lw = if has_l && has_r {
                ((usable as f32) * state.model.traffic_bar_ratio.clamp(0.08, 0.92)).round() as i32
            } else if has_l {
                usable
            } else {
                0
            };
            let rw = if has_r { (usable - lw).max(0) } else { 0 };
            if lw > 1 {
                fill_round_rect(
                    hdc,
                    RECT {
                        left,
                        top: y,
                        right: left + lw,
                        bottom: y + bar_h,
                    },
                    bar_r,
                    p.accent,
                    p.accent,
                );
            }
            if rw > 1 {
                fill_round_rect(
                    hdc,
                    RECT {
                        left: left + lw + gap,
                        top: y,
                        right,
                        bottom: y + bar_h,
                    },
                    bar_r,
                    p.ok,
                    p.ok,
                );
            }
        }
    } else if state.model.traffic_bar_has_left {
        let fw = ((content_w as f32) * state.model.traffic_bar_ratio.clamp(0.0, 1.0))
            .round()
            .max(scale(8, d) as f32) as i32;
        fill_round_rect(
            hdc,
            RECT {
                left,
                top: y,
                right: (left + fw).min(right),
                bottom: y + bar_h,
            },
            bar_r,
            state.model.traffic_bar_color,
            state.model.traffic_bar_color,
        );
    }
    y += bar_h + scale(8, d);

    // 底部 1 行指标对照
    let foot_h = scale(16, d);
    let mut lx = left;
    if let Some(dot_c) = state.model.traffic_foot_left_dot {
        fill_circle(
            hdc,
            lx + scale(3, d),
            y + foot_h / 2,
            scale(3, d),
            dot_c,
            dot_c,
        );
        lx += scale(10, d);
    }
    draw_text_line(
        hdc,
        f_sm,
        &state.model.traffic_foot_left,
        RECT {
            left: lx,
            top: y,
            right: left + content_w * 54 / 100,
            bottom: y + foot_h,
        },
        p.text_sec,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );

    if !state.model.traffic_foot_right.is_empty() {
        let right_txt_w = measure_text_width(hdc, f_sm, &state.model.traffic_foot_right);
        let max_rw = content_w * 48 / 100;
        let clamped_rw = right_txt_w.min(max_rw);
        if let Some(dot_c) = state.model.traffic_foot_right_dot {
            let dot_x = right - clamped_rw - scale(7, d);
            fill_circle(hdc, dot_x, y + foot_h / 2, scale(3, d), dot_c, dot_c);
        }
        draw_text_line(
            hdc,
            f_sm,
            &state.model.traffic_foot_right,
            RECT {
                left: right - max_rw,
                top: y,
                right,
                bottom: y + foot_h,
            },
            if state.model.traffic_foot_right_dot.is_some() {
                p.text_sec
            } else {
                p.text_muted
            },
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
    }
    y += scale(17, d);

    y = draw_divider(hdc, left, right, y, p.divider, d);

    // ── 3. 区块二：在线设备 ──────────────────────────────────────────
    draw_text_line(
        hdc,
        f_sec,
        "在线设备",
        RECT {
            left,
            top: y,
            right: left + scale(100, d),
            bottom: y + sec_h,
        },
        p.text,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    let badge_w = measure_text_width(hdc, f_bold_sm, &state.model.devices_badge) + scale(14, d);
    draw_pill_badge(
        hdc,
        f_bold_sm,
        &state.model.devices_badge,
        right - badge_w,
        y + sec_h / 2,
        scale(18, d),
        scale(14, d),
        state.model.devices_badge_fg,
        state.model.devices_badge_bg,
        state.model.devices_badge_bd,
        d,
    );
    y += sec_h + scale(6, d);

    if state.model.devices.is_empty() {
        let row_h = scale(26, d);
        let r = RECT {
            left,
            top: y,
            right,
            bottom: y + row_h,
        };
        fill_round_rect(hdc, r, scale(6, d), p.subtle, p.divider);
        draw_text_line(
            hdc,
            f_sm,
            &state.model.devices_empty_hint,
            RECT {
                left: left + scale(10, d),
                top: y,
                right: right - scale(10, d),
                bottom: y + row_h,
            },
            p.text_muted,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        y += row_h;
    } else {
        let row_h = scale(26, d);
        let row_gap = scale(5, d);
        for (idx, dev) in state.model.devices.iter().enumerate() {
            if idx > 0 {
                y += row_gap;
            }
            let r = RECT {
                left,
                top: y,
                right,
                bottom: y + row_h,
            };
            let (row_bg, row_bd) = if dev.is_conflict {
                (p.warn_soft, p.warn_border)
            } else {
                (p.subtle, p.divider)
            };
            fill_round_rect(hdc, r, scale(6, d), row_bg, row_bd);

            let cy = y + row_h / 2;
            let dot_c = if dev.is_local {
                p.ok
            } else if dev.is_conflict {
                p.warn
            } else {
                p.accent
            };
            fill_circle(hdc, left + scale(11, d), cy, scale(3, d), dot_c, dot_c);

            // 固定 32px 终端类型胶囊
            let term_x = left + scale(20, d);
            let term_w = scale(32, d);
            let term_r = RECT {
                left: term_x,
                top: cy - scale(8, d),
                right: term_x + term_w,
                bottom: cy + scale(8, d),
            };
            fill_round_rect(hdc, term_r, scale(4, d), p.pill_bg, p.pill_border);
            draw_text_line(
                hdc,
                f_bold_sm,
                &dev.terminal,
                term_r,
                p.text_sec,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );

            // 固定 90px IP 列
            let ip_x = term_x + term_w + scale(7, d);
            let ip_w = scale(90, d);
            draw_text_line(
                hdc,
                f_ip,
                &dev.ip,
                RECT {
                    left: ip_x,
                    top: y,
                    right: ip_x + ip_w,
                    bottom: y + row_h,
                },
                p.text,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );

            // 固定 34px 身份胶囊
            let id_x = ip_x + ip_w + scale(4, d);
            let id_w = scale(34, d);
            let (id_lbl, id_fg, id_bg, id_bd) = if dev.is_local {
                ("本机", p.ok, p.ok_soft, p.ok_border)
            } else if dev.is_conflict {
                ("占线", p.warn, p.warn_soft, p.warn_border)
            } else {
                ("其他", p.text_muted, p.pill_bg, p.pill_border)
            };
            let id_r = RECT {
                left: id_x,
                top: cy - scale(8, d),
                right: id_x + id_w,
                bottom: cy + scale(8, d),
            };
            fill_round_rect(hdc, id_r, scale(4, d), id_bg, id_bd);
            draw_text_line(
                hdc,
                f_bold_sm,
                id_lbl,
                id_r,
                id_fg,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );

            // 右侧时长
            draw_text_line(
                hdc,
                f_sm,
                &dev.duration,
                RECT {
                    left: id_x + id_w + scale(4, d),
                    top: y,
                    right: right - scale(8, d),
                    bottom: y + row_h,
                },
                p.text_muted,
                DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );

            y += row_h;
        }
        if state.model.devices_overflow_extra > 0 {
            y += scale(3, d);
            draw_text_line(
                hdc,
                f_xs,
                &format!(
                    "另有 {} 台在线终端，点击下方打开主界面查看",
                    state.model.devices_overflow_extra
                ),
                RECT {
                    left: left + scale(2, d),
                    top: y,
                    right,
                    bottom: y + scale(13, d),
                },
                p.text_muted,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            y += scale(13, d);
        }
    }

    y = draw_divider(hdc, left, right, y, p.divider, d);

    // ── 4. 区块三：连接稳定性（微缩 36 枚竖向细胶囊） ────────────────
    let stab_title_w = measure_or_draw_text(
        hdc,
        f_sec,
        "连接稳定性",
        RECT {
            left,
            top: y,
            right: left + scale(80, d),
            bottom: y + sec_h,
        },
        p.text,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    draw_text_line(
        hdc,
        f_title,
        &state.model.uptime_text,
        RECT {
            left: left + stab_title_w + scale(6, d),
            top: y,
            right: left + stab_title_w + scale(62, d),
            bottom: y + sec_h,
        },
        state.model.uptime_color,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    let seg_left = draw_segmented_pair(
        hdc,
        f_sm,
        f_bold_sm,
        right,
        y + sec_h / 2,
        ("7h", HitTarget::Range7h),
        ("24h", HitTarget::Range24h),
        state.range == TimeRange::H7,
        state.hovered,
        &mut state.hits,
        &p,
        d,
    );
    draw_text_line(
        hdc,
        f_sm,
        &state.model.outage_summary,
        RECT {
            left: left + stab_title_w + scale(54, d),
            top: y,
            right: seg_left - scale(6, d),
            bottom: y + sec_h,
        },
        p.text_muted,
        DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    y += sec_h + scale(6, d);

    // 36 枚竖向细胶囊（14px 高）
    let pill_area_h = scale(14, d);
    let slot_n = state.model.slots.len().max(1) as i32;
    let gap = scale(2, d).max(2);
    let total_gap = gap * (slot_n - 1);
    let slot_w = ((content_w - total_gap) / slot_n).max(3);
    let used_w = slot_w * slot_n + total_gap;
    let start_x = left + (content_w - used_w) / 2;
    let pill_r = scale(3, d).min(slot_w);

    for (idx, kind) in state.model.slots.iter().enumerate() {
        let x0 = start_x + idx as i32 * (slot_w + gap);
        let (top_y, color) = match kind {
            SlotKind::Online => (y + scale(1, d), p.ok),
            SlotKind::Outage => (y, p.warn),
            SlotKind::Offline => (y, p.danger),
            SlotKind::Idle => (y + pill_area_h - scale(5, d), p.idle_pill),
        };
        fill_round_rect(
            hdc,
            RECT {
                left: x0,
                top: top_y,
                right: x0 + slot_w,
                bottom: y + pill_area_h,
            },
            pill_r,
            color,
            color,
        );
    }
    y += pill_area_h + scale(3, d);

    // 首尾极简时刻标尺
    let ruler_h = scale(11, d);
    draw_text_line(
        hdc,
        f_xs,
        &state.model.ruler_left,
        RECT {
            left,
            top: y,
            right: left + scale(60, d),
            bottom: y + ruler_h,
        },
        p.text_muted,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    draw_text_line(
        hdc,
        f_xs,
        "现在",
        RECT {
            left: right - scale(50, d),
            top: y,
            right,
            bottom: y + ruler_h,
        },
        p.text_sec,
        DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    y += ruler_h;

    y = draw_divider(hdc, left, right, y, p.divider, d);

    // ── 5. 底部快捷操作栏（28px） ────────────────────────────────────
    let btn_h = scale(28, d);
    let btn_gap = scale(6, d);
    let avail_btn_w = content_w - btn_gap * 2;
    let w1 = avail_btn_w * 29 / 100;
    let w2 = avail_btn_w * 30 / 100;
    let w3 = avail_btn_w - w1 - w2;

    let r1 = RECT {
        left,
        top: y,
        right: left + w1,
        bottom: y + btn_h,
    };
    let r2 = RECT {
        left: r1.right + btn_gap,
        top: y,
        right: r1.right + btn_gap + w2,
        bottom: y + btn_h,
    };
    let r3 = RECT {
        left: r2.right + btn_gap,
        top: y,
        right: r2.right + btn_gap + w3,
        bottom: y + btn_h,
    };

    draw_action_button(
        hdc,
        f_body,
        r1,
        "立即检测",
        false,
        state.hovered == Some(HitTarget::CheckNow),
        &p,
        d,
    );
    state.hits.push(HitBox {
        target: HitTarget::CheckNow,
        rect: r1,
    });

    let toggle_lbl = if state.model.monitoring_enabled {
        "暂停守护"
    } else {
        "开始守护"
    };
    draw_action_button(
        hdc,
        f_body,
        r2,
        toggle_lbl,
        false,
        state.hovered == Some(HitTarget::ToggleMonitor),
        &p,
        d,
    );
    state.hits.push(HitBox {
        target: HitTarget::ToggleMonitor,
        rect: r2,
    });

    draw_action_button(
        hdc,
        f_ip,
        r3,
        "打开主界面 →",
        true,
        state.hovered == Some(HitTarget::OpenMain),
        &p,
        d,
    );
    state.hits.push(HitBox {
        target: HitTarget::OpenMain,
        rect: r3,
    });

    DeleteObject(f_title);
    DeleteObject(f_sec);
    DeleteObject(f_hero);
    DeleteObject(f_ip);
    DeleteObject(f_body);
    DeleteObject(f_bold_sm);
    DeleteObject(f_sm);
    DeleteObject(f_xs);
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_segmented_pair(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font_normal: windows_sys::Win32::Graphics::Gdi::HFONT,
    font_bold: windows_sys::Win32::Graphics::Gdi::HFONT,
    right_edge: i32,
    cy: i32,
    left_opt: (&str, HitTarget),
    right_opt: (&str, HitTarget),
    left_active: bool,
    hovered: Option<HitTarget>,
    hits: &mut Vec<HitBox>,
    p: &FlyoutPalette,
    dpi: u32,
) -> i32 {
    let pad = scale(2, dpi);
    let gap = scale(2, dpi);
    let total_h = scale(22, dpi);
    let item_h = total_h - pad * 2;
    let w_l = (measure_text_width(hdc, font_bold, left_opt.0) + scale(14, dpi)).max(scale(30, dpi));
    let w_r =
        (measure_text_width(hdc, font_bold, right_opt.0) + scale(14, dpi)).max(scale(30, dpi));
    let total_w = w_l + w_r + gap + pad * 2;

    let outer = RECT {
        left: right_edge - total_w,
        top: cy - total_h / 2,
        right: right_edge,
        bottom: cy + total_h / 2,
    };
    fill_round_rect(hdc, outer, scale(6, dpi), p.subtle, p.divider);

    let rl = RECT {
        left: outer.left + pad,
        top: outer.top + pad,
        right: outer.left + pad + w_l,
        bottom: outer.top + pad + item_h,
    };
    let rr = RECT {
        left: rl.right + gap,
        top: outer.top + pad,
        right: rl.right + gap + w_r,
        bottom: outer.top + pad + item_h,
    };

    for (rect, (lbl, target), active) in
        [(rl, left_opt, left_active), (rr, right_opt, !left_active)]
    {
        let is_hov = hovered == Some(target);
        if active {
            fill_round_rect(hdc, rect, scale(4, dpi), p.pill_bg, p.pill_border);
        } else if is_hov {
            fill_round_rect(hdc, rect, scale(4, dpi), p.btn_hover, p.btn_hover);
        }
        draw_text_line(
            hdc,
            if active { font_bold } else { font_normal },
            lbl,
            rect,
            if active { p.text } else { p.text_muted },
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        hits.push(HitBox { target, rect });
    }

    outer.left
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_action_button(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: windows_sys::Win32::Graphics::Gdi::HFONT,
    rect: RECT,
    label: &str,
    primary: bool,
    hovered: bool,
    p: &FlyoutPalette,
    dpi: u32,
) {
    let (bg, bd, fg) = if primary {
        let fill = if hovered { p.accent_hover } else { p.accent };
        (fill, fill, rgb(0xFF, 0xFF, 0xFF))
    } else {
        let fill = if hovered { p.btn_hover } else { p.btn_bg };
        (fill, p.btn_border, p.text)
    };
    fill_round_rect(hdc, rect, scale(6, dpi), bg, bd);
    draw_text_line(
        hdc,
        font,
        label,
        rect,
        fg,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_pill_badge(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: windows_sys::Win32::Graphics::Gdi::HFONT,
    text: &str,
    left: i32,
    cy: i32,
    height: i32,
    pad_h: i32,
    fg: COLORREF,
    bg: COLORREF,
    border: COLORREF,
    dpi: u32,
) {
    let tw = measure_text_width(hdc, font, text);
    let w = tw + pad_h;
    let r = RECT {
        left,
        top: cy - height / 2,
        right: left + w,
        bottom: cy + height / 2,
    };
    fill_round_rect(hdc, r, scale(5, dpi), bg, border);
    draw_text_line(
        hdc,
        font,
        text,
        r,
        fg,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
}

unsafe fn draw_divider(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    left: i32,
    right: i32,
    y: i32,
    color: COLORREF,
    dpi: u32,
) -> i32 {
    let line_y = y + scale(8, dpi);
    let pen = CreatePen(PS_SOLID, 1, color);
    let old = SelectObject(hdc, pen);
    MoveToEx(hdc, left, line_y, null_mut());
    LineTo(hdc, right, line_y);
    SelectObject(hdc, old);
    DeleteObject(pen);
    y + scale(17, dpi)
}

unsafe fn fill_round_rect(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    r: RECT,
    radius: i32,
    fill: COLORREF,
    border: COLORREF,
) {
    let brush = CreateSolidBrush(fill);
    let pen = if border == fill {
        windows_sys::Win32::Graphics::Gdi::GetStockObject(NULL_PEN)
    } else {
        CreatePen(PS_SOLID, 1, border)
    };
    let old_b = SelectObject(hdc, brush);
    let old_p = SelectObject(hdc, pen);
    RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    DeleteObject(brush);
    if border != fill {
        DeleteObject(pen);
    }
}

unsafe fn fill_circle(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    cx: i32,
    cy: i32,
    r: i32,
    border: COLORREF,
    fill: COLORREF,
) {
    let brush = CreateSolidBrush(fill);
    let pen = if border == fill {
        windows_sys::Win32::Graphics::Gdi::GetStockObject(NULL_PEN)
    } else {
        CreatePen(PS_SOLID, 1, border)
    };
    let old_b = SelectObject(hdc, brush);
    let old_p = SelectObject(hdc, pen);
    Ellipse(hdc, cx - r, cy - r, cx + r, cy + r);
    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    DeleteObject(brush);
    if border != fill {
        DeleteObject(pen);
    }
}

unsafe fn make_font(pt_px: i32, weight: u32, dpi: u32) -> windows_sys::Win32::Graphics::Gdi::HFONT {
    let face = to_wide("Microsoft YaHei UI");
    let mut face_buf = [0u16; 32];
    let n = face.len().min(31);
    face_buf[..n].copy_from_slice(&face[..n]);
    CreateFontW(
        -scale(pt_px, dpi),
        0,
        0,
        0,
        weight as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        face_buf.as_ptr(),
    )
}

unsafe fn draw_text_line(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: windows_sys::Win32::Graphics::Gdi::HFONT,
    text: &str,
    mut rect: RECT,
    color: COLORREF,
    flags: u32,
) {
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, color);
    let wide: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(hdc, wide.as_ptr(), wide.len() as i32, &mut rect, flags);
    SelectObject(hdc, old);
}

unsafe fn measure_or_draw_text(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: windows_sys::Win32::Graphics::Gdi::HFONT,
    text: &str,
    rect: RECT,
    color: COLORREF,
    flags: u32,
) -> i32 {
    draw_text_line(hdc, font, text, rect, color, flags);
    measure_text_width(hdc, font, text)
}

unsafe fn measure_text_width(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: windows_sys::Win32::Graphics::Gdi::HFONT,
    text: &str,
) -> i32 {
    const DT_CALCRECT: u32 = 0x0000_0400;
    let old = SelectObject(hdc, font);
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let wide: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(
        hdc,
        wide.as_ptr(),
        wide.len() as i32,
        &mut r,
        DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
    );
    SelectObject(hdc, old);
    r.right - r.left
}

fn scale(px: i32, dpi: u32) -> i32 {
    ((px as i64 * dpi as i64 + 48) / 96) as i32
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn detect_dark_mode(app_dir: &Path) -> bool {
    let prefs_path = app_dir.join("ui_prefs.json");
    if let Ok(text) = std::fs::read_to_string(&prefs_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(theme) = v.get("theme").and_then(|t| t.as_str()) {
                return theme.eq_ignore_ascii_case("dark");
            }
        }
    }
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    if let Ok(subkey) = key {
        if let Ok(val) = subkey.get_value::<u32, _>("AppsUseLightTheme") {
            return val == 0;
        }
    }
    false
}

fn carrier_name(carrier: &str) -> &'static str {
    match carrier {
        "0" => "校园用户",
        "1" => "中国电信",
        "2" => "中国联通",
        "3" => "中国移动",
        _ => "中国联通",
    }
}

fn format_ms(ms: u64) -> String {
    if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms} ms")
    }
}

fn format_uptime_compact(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn format_duration_short(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn format_mb(mb: f64) -> String {
    let mb = if mb.is_finite() && mb > 0.0 { mb } else { 0.0 };
    if mb >= 1024.0 * 1024.0 {
        format!("{:.2} TB", mb / (1024.0 * 1024.0))
    } else if mb >= 1024.0 {
        let gb = mb / 1024.0;
        if gb >= 100.0 {
            format!("{gb:.1} GB")
        } else {
            format!("{gb:.2} GB")
        }
    } else if mb >= 100.0 {
        format!("{mb:.0} MB")
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else if mb > 0.0 {
        format!("{:.0} KB", mb * 1024.0)
    } else {
        "0 MB".to_string()
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        let m = b / MB;
        if m >= 100.0 {
            format!("{m:.0} MB")
        } else {
            format!("{m:.1} MB")
        }
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else if bytes == 0 {
        "0 MB".to_string()
    } else {
        format!("{bytes} B")
    }
}

fn format_hm(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".to_string())
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut iter = text.chars();
    let collected: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{MonitorMetrics, SelfServiceData, SelfServiceTraffic};

    fn sample_snapshot(now: i64) -> MonitorSnapshot {
        MonitorSnapshot {
            monitoring_enabled: true,
            retry_after_secs: None,
            state: MonitorState::Online,
            reason: Some(DisconnectReason::Online),
            message: "公网连接正常".into(),
            metrics: MonitorMetrics {
                portal_ip: "172.16.24.195".into(),
                last_check_ms: 51,
                ..Default::default()
            },
        }
        .tap_ts(now)
    }

    trait TapTs: Sized {
        fn tap_ts(self, _now: i64) -> Self {
            self
        }
    }
    impl TapTs for MonitorSnapshot {}

    fn sample_self_service(now: i64, two_devices: bool) -> SelfServiceState {
        let mut devices = vec![OnlineDevice {
            ip: "172.16.24.195".into(),
            login_time: "2026-11-22 05:52:00".into(),
            login_unix: Some(now - 6960),
            terminal_type: "PC".into(),
            use_time_secs: Some(6960),
        }];
        if two_devices {
            devices.push(OnlineDevice {
                ip: "172.16.25.102".into(),
                login_time: "2026-11-22 06:20:00".into(),
                login_unix: Some(now - 5400),
                terminal_type: "手机".into(),
                use_time_secs: Some(5400),
            });
        }
        SelfServiceState {
            status: SelfServiceStatus::Ready,
            message: String::new(),
            cooldown_until_unix: None,
            data: Some(SelfServiceData {
                account_key: "demo".into(),
                traffic: SelfServiceTraffic {
                    campus_used_mb: 535.2,
                    campus_remaining_mb: 553_035.5,
                    month_down_mb: 285_804.0,
                    month_up_mb: 80_081.6,
                    user_group_name: "硕士-计科".into(),
                    plan_description: "每月免费30G".into(),
                    month_session_count: Some(236),
                },
                max_online_devices: Some(3),
                online_devices: devices,
                recent_sessions: Vec::new(),
                fetched_at_unix: now - 30,
            }),
        }
    }

    fn sample_telemetry(now: i64) -> Vec<TelemetryPoint> {
        let mut pts = Vec::new();
        let start = now - 7 * 3600;
        let outages = [now - 5 * 3600, now - 3 * 3600, now - 2400];
        let mut t = start;
        while t <= now {
            let in_out = outages.iter().any(|&o| t >= o && t <= o + 15);
            pts.push(TelemetryPoint {
                ts: t,
                ms: if in_out { 0 } else { 51 },
                online: !in_out,
                reason: if in_out {
                    "AUTH_EXPIRED".into()
                } else {
                    "ONLINE".into()
                },
                event: "check".into(),
                confirmed_offline: Some(in_out),
                sample_id: None,
            });
            t += 300;
        }
        for o in outages {
            pts.push(TelemetryPoint {
                ts: o,
                ms: 0,
                online: false,
                reason: "AUTH_EXPIRED".into(),
                event: "check".into(),
                confirmed_offline: Some(true),
                sample_id: None,
            });
            pts.push(TelemetryPoint {
                ts: o + 3,
                ms: 48,
                online: true,
                reason: "ONLINE".into(),
                event: "auth_ok".into(),
                confirmed_offline: Some(false),
                sample_id: None,
            });
        }
        pts.sort_by_key(|p| p.ts);
        pts
    }

    #[test]
    fn flyout_model_builds_carrier_campus_and_degraded_views() {
        let now = Local::now().timestamp();
        let snap = sample_snapshot(now);
        let ss = sample_self_service(now, true);
        let pts = sample_telemetry(now);
        let pal = FlyoutPalette::of(true);

        let carrier_m = build_flyout_model(
            &pal,
            Some(&snap),
            Some(now - 6960),
            &TrafficSummary::default(),
            &ss,
            "HJK",
            "2",
            FlyoutTrafficMode::Carrier,
            &pts,
            TimeRange::H7,
        );
        assert_eq!(carrier_m.status_title, "校园网已连接");
        assert!(carrier_m.traffic_hero.contains("GB"));
        assert_eq!(carrier_m.devices.len(), 2);
        assert_eq!(carrier_m.slots.len(), 36);

        let campus_m = build_flyout_model(
            &pal,
            Some(&snap),
            Some(now - 6960),
            &TrafficSummary::default(),
            &ss,
            "HJK",
            "0",
            FlyoutTrafficMode::Campus,
            &pts,
            TimeRange::H7,
        );
        assert_eq!(campus_m.traffic_hero, "540.1 GB");
        assert!(campus_m.traffic_sub.contains("99.9%"));
        assert!(campus_m.traffic_foot_right.contains("硕士-计科"));

        // Optional GDI visual QA dump when GXNU_FLYOUT_QA_DIR is set
        if let Ok(dir) = std::env::var("GXNU_FLYOUT_QA_DIR") {
            let out_dir = PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&out_dir);
            render_gdi_snapshot(
                &out_dir.join("flyout-dark.bgra"),
                true,
                FlyoutTrafficMode::Carrier,
                false,
            );
            render_gdi_snapshot(
                &out_dir.join("flyout-light.bgra"),
                false,
                FlyoutTrafficMode::Campus,
                true,
            );
        }
    }

    fn render_gdi_snapshot(
        out_path: &Path,
        dark: bool,
        mode: FlyoutTrafficMode,
        two_devices: bool,
    ) {
        use windows_sys::Win32::Graphics::Gdi::{
            CreateDIBSection, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        };
        let now = Local::now().timestamp();
        let snap = sample_snapshot(now);
        let ss = sample_self_service(now, two_devices);
        let pts = sample_telemetry(now);
        let pal = FlyoutPalette::of(dark);
        let model = build_flyout_model(
            &pal,
            Some(&snap),
            Some(now - 6960),
            &TrafficSummary::default(),
            &ss,
            "HJK",
            if mode == FlyoutTrafficMode::Campus {
                "0"
            } else {
                "2"
            },
            mode,
            &pts,
            TimeRange::H7,
        );
        let dpi: u32 = 144;
        let tmp_dir = std::env::temp_dir().join("gxnu_flyout_qa_dummy");
        let mut state = FlyoutState {
            sources: FlyoutDataSources {
                app_dir: tmp_dir.clone(),
                latest_snapshot: Arc::new(Mutex::new(Some(snap))),
                online_since_unix: Arc::new(Mutex::new(Some(now - 6960))),
                latest_traffic: Arc::new(Mutex::new(TrafficSummary::default())),
                latest_self_service: Arc::new(Mutex::new(ss)),
                active_account_label: Arc::new(Mutex::new("HJK".into())),
                active_carrier: Arc::new(Mutex::new("2".into())),
                metrics_store: Arc::new(MetricsStore::new(&tmp_dir)),
            },
            on_action: Arc::new(|_| {}),
            palette: pal,
            traffic_mode: mode,
            traffic_user_override: true,
            last_carrier: "2".into(),
            range: TimeRange::H7,
            dpi,
            width_px: scale(326, dpi),
            height_px: 0,
            hovered: None,
            tracking_mouse: false,
            hits: Vec::new(),
            model,
            telemetry_cache: pts,
            last_deactivated: None,
        };
        state.height_px = compute_height_px(&state);
        let w = state.width_px;
        let h = state.height_px;

        unsafe {
            let mem_dc = CreateCompatibleDC(null_mut());
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h; // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;

            let mut bits: *mut c_void = null_mut();
            let dib = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if !dib.is_null() && !bits.is_null() {
                let old = SelectObject(mem_dc, dib);
                draw_flyout_contents(mem_dc, &mut state, w, h);
                let byte_len = (w * h * 4) as usize;
                let slice = std::slice::from_raw_parts(bits as *const u8, byte_len);
                let mut buf = Vec::with_capacity(8 + byte_len);
                buf.extend_from_slice(&(w as u32).to_le_bytes());
                buf.extend_from_slice(&(h as u32).to_le_bytes());
                buf.extend_from_slice(slice);
                let _ = std::fs::write(out_path, buf);
                SelectObject(mem_dc, old);
                DeleteObject(dib);
            }
            DeleteDC(mem_dc);
        }
    }
}
