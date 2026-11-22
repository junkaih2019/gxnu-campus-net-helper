//! 现代 Windows 11 Fluent 风格托盘右键菜单（纯 Win32 GDI + DWM 原生圆角与阴影）。
//!
//! 特点：
//! - 零 OpenGL、零额外字库内存占用，保持轻守护进程 ~3 MB 私有内存与 0% 后台 CPU
//! - 自动跟随应用内浅色/深色主题（读取 `ui_prefs.json` 或系统 `AppsUseLightTheme`）
//! - 完整 Per-Monitor DPI 适配 + 双缓冲绘制，100%–200% 缩放下文字与圆角同样锐利
//! - 顶部实时状态卡片（状态指示灯 + 状态文案 + 延迟/账号摘要）+ 圆角悬停胶囊 + 红色退出警示态

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, Ellipse, EndPaint, FillRect,
    GetMonitorInfoW, InvalidateRect, LineTo, MonitorFromPoint, MoveToEx, RoundRect, SelectObject,
    SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_NORMAL,
    FW_SEMIBOLD, MONITORINFO, MONITOR_DEFAULTTONEAREST, NULL_PEN, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetWindowLongPtrW, LoadCursorW,
    RegisterClassW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    CS_DROPSHADOW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HWND_TOPMOST, IDC_ARROW, IDC_HAND,
    SWP_NOACTIVATE, SW_HIDE, SW_SHOW, WA_INACTIVE, WM_ACTIVATE, WM_ERASEBKGND, WM_KEYDOWN,
    WM_KILLFOCUS, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::models::{MonitorSnapshot, MonitorState};
use crate::tray::TrayAction;

const MENU_CLASS_NAME: &str = "GXNUGuardFluentTrayMenu";
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;
const DWMWCP_ROUND: u32 = 2;

const WM_MOUSELEAVE_MSG: u32 = 0x02A3;
const TME_LEAVE_FLAG: u32 = 0x00000002;
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
    fn SetProcessDpiAwarenessContext(value: isize) -> i32;
    fn GetDpiForWindow(hwnd: HWND) -> u32;
    fn TrackMouseEvent(lpeventtrack: *mut TrackMouseEventStruct) -> i32;
}

/// 启用守护进程的 Per-Monitor V2 DPI 感知，确保高分屏下托盘菜单清晰锐利。
pub fn enable_dpi_awareness() {
    unsafe {
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4
        let _ = SetProcessDpiAwarenessContext(-4);
    }
}

#[derive(Clone, Copy)]
struct MenuPalette {
    dark: bool,
    bg: COLORREF,
    border: COLORREF,
    divider: COLORREF,
    hover_bg: COLORREF,
    danger_hover_bg: COLORREF,
    text: COLORREF,
    text_muted: COLORREF,
    accent: COLORREF,
    danger: COLORREF,
}

impl MenuPalette {
    fn load(app_dir: &Path) -> Self {
        let dark = detect_dark_mode(app_dir);
        if dark {
            Self {
                dark: true,
                bg: rgb(0x1B, 0x1F, 0x27),
                border: rgb(0x2F, 0x36, 0x44),
                divider: rgb(0x28, 0x2E, 0x3A),
                hover_bg: rgb(0x27, 0x2D, 0x3A),
                danger_hover_bg: rgb(0x36, 0x1D, 0x22),
                text: rgb(0xF1, 0xF5, 0xF9),
                text_muted: rgb(0x94, 0xA3, 0xB8),
                accent: rgb(0x60, 0xA5, 0xFA),
                danger: rgb(0xF8, 0x71, 0x71),
            }
        } else {
            Self {
                dark: false,
                bg: rgb(0xFF, 0xFF, 0xFF),
                border: rgb(0xE2, 0xE8, 0xF0),
                divider: rgb(0xEE, 0xF2, 0xF6),
                hover_bg: rgb(0xF1, 0xF5, 0xF9),
                danger_hover_bg: rgb(0xFE, 0xF2, 0xF2),
                text: rgb(0x0F, 0x17, 0x2A),
                text_muted: rgb(0x64, 0x74, 0x8B),
                accent: rgb(0x25, 0x63, 0xEB),
                danger: rgb(0xDC, 0x26, 0x26),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ItemIcon {
    Window,
    Refresh,
    Pause,
    Play,
    Log,
    Exit,
}

#[derive(Clone)]
struct MenuItemSpec {
    action: TrayAction,
    label: &'static str,
    icon: ItemIcon,
    is_primary: bool,
    is_danger: bool,
    separator_before: bool,
    /// 命中区域（物理像素，由 layout 计算）
    rect: RECT,
}

struct MenuState {
    app_dir: PathBuf,
    on_action: Arc<dyn Fn(TrayAction) + Send + Sync>,
    latest_snapshot: Arc<Mutex<Option<MonitorSnapshot>>>,
    active_account_label: Arc<Mutex<String>>,
    palette: MenuPalette,
    items: Vec<MenuItemSpec>,
    hovered: Option<usize>,
    tracking_mouse: bool,
    dpi: u32,
    header_sub: String,
    status_color: COLORREF,
    status_halo: COLORREF,
}

pub struct FluentTrayMenu {
    hwnd: HWND,
    state_ptr: *mut MenuState,
}

impl FluentTrayMenu {
    pub fn new<F>(
        app_dir: PathBuf,
        latest_snapshot: Arc<Mutex<Option<MonitorSnapshot>>>,
        active_account_label: Arc<Mutex<String>>,
        on_action: F,
    ) -> Result<Self, String>
    where
        F: Fn(TrayAction) + Send + Sync + 'static,
    {
        unsafe {
            let hinstance = GetModuleHandleW(null());
            let class_w = to_wide(MENU_CLASS_NAME);
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW | CS_DROPSHADOW,
                lpfnWndProc: Some(menu_wnd_proc),
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

            let palette = MenuPalette::load(&app_dir);
            let boxed = Box::new(MenuState {
                app_dir,
                on_action: Arc::new(on_action),
                latest_snapshot,
                active_account_label,
                palette,
                items: Vec::new(),
                hovered: None,
                tracking_mouse: false,
                dpi: 96,
                header_sub: "后台轻量运行中".to_string(),
                status_color: rgb(0x22, 0xC5, 0x5E),
                status_halo: rgb(0xDC, 0xFC, 0xE7),
            });
            let state_ptr = Box::into_raw(boxed);

            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class_w.as_ptr(),
                class_w.as_ptr(),
                WS_POPUP,
                0,
                0,
                220,
                250,
                null_mut(),
                null_mut(),
                hinstance,
                null_mut(),
            );
            if hwnd.is_null() {
                drop(Box::from_raw(state_ptr));
                return Err("创建托盘弹出菜单窗口失败".to_string());
            }

            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);

            // 启用 Windows 11 原生圆角与阴影
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

    /// 在鼠标位置附近（智能避让任务栏与屏幕边缘）弹出刷新后的现代托盘菜单。
    pub fn show_at_cursor(&mut self) {
        unsafe {
            let state = &mut *self.state_ptr;
            state.palette = MenuPalette::load(&state.app_dir);
            state.hovered = None;
            state.tracking_mouse = false;

            let snap = state.latest_snapshot.lock().ok().and_then(|g| g.clone());
            refresh_menu_content(state, snap.as_ref());

            // 更新 DWM 深色模式与边框颜色
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
            let (win_w, win_h) = compute_layout(state, dpi);

            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);

            let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            let (work_left, work_top, work_right, work_bottom) =
                if GetMonitorInfoW(monitor, &mut mi) != 0 {
                    (
                        mi.rcWork.left,
                        mi.rcWork.top,
                        mi.rcWork.right,
                        mi.rcWork.bottom,
                    )
                } else {
                    (0, 0, 1920, 1040)
                };

            let margin = scale(8, dpi);
            // 优先弹在鼠标上方并右对齐光标，若越界则自动贴合工作区边缘
            let mut x = pt.x - win_w / 2;
            let mut y = pt.y - win_h - margin;
            if x + win_w > work_right - margin {
                x = work_right - win_w - margin;
            }
            if x < work_left + margin {
                x = work_left + margin;
            }
            if y < work_top + margin {
                y = pt.y + margin;
            }
            if y + win_h > work_bottom - margin {
                y = work_bottom - win_h - margin;
            }

            SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, win_w, win_h, SWP_NOACTIVATE);
            ShowWindow(self.hwnd, SW_SHOW);
            SetForegroundWindow(self.hwnd);
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    pub fn hide(&mut self) {
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

impl Drop for FluentTrayMenu {
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

fn refresh_menu_content(state: &mut MenuState, snap: Option<&MonitorSnapshot>) {
    let dark = state.palette.dark;
    let monitor_state = snap.map(|s| s.state).unwrap_or(MonitorState::Paused);
    let running = snap.is_some_and(|s| s.monitoring_enabled);

    let (title, dot, halo) = match monitor_state {
        MonitorState::Online => (
            "校园网已连接",
            rgb(0x22, 0xC5, 0x5E),
            if dark {
                rgb(0x16, 0x3A, 0x2A)
            } else {
                rgb(0xDC, 0xFC, 0xE7)
            },
        ),
        MonitorState::Checking => (
            "正在检测网络",
            rgb(0x38, 0xBD, 0xF8),
            if dark {
                rgb(0x16, 0x33, 0x45)
            } else {
                rgb(0xE0, 0xF2, 0xFE)
            },
        ),
        MonitorState::Authenticating | MonitorState::Verifying | MonitorState::Suspect => (
            "正在自动恢复",
            rgb(0xF5, 0x9E, 0x0B),
            if dark {
                rgb(0x3B, 0x2E, 0x15)
            } else {
                rgb(0xFE, 0xF3, 0xC7)
            },
        ),
        MonitorState::Offline => (
            "校园网已断开",
            rgb(0xEF, 0x44, 0x44),
            if dark {
                rgb(0x3D, 0x1B, 0x20)
            } else {
                rgb(0xFE, 0xE2, 0xE2)
            },
        ),
        MonitorState::Paused | MonitorState::Stopped => (
            "监控已暂停",
            rgb(0x94, 0xA3, 0xB8),
            if dark {
                rgb(0x27, 0x2D, 0x3A)
            } else {
                rgb(0xF1, 0xF5, 0xF9)
            },
        ),
    };

    state.status_color = dot;
    state.status_halo = halo;
    let account_label = state
        .active_account_label
        .lock()
        .ok()
        .map(|g| g.trim().to_string())
        .filter(|s| !s.is_empty() && s != "未配置账号");
    state.header_sub = match (snap, account_label.as_deref()) {
        (Some(s), Some(acct)) if s.state == MonitorState::Online && s.metrics.last_check_ms > 0 => {
            let ms = s.metrics.last_check_ms;
            let lat = if ms >= 1000 {
                format!("{:.1}s", ms as f64 / 1000.0)
            } else {
                format!("{ms} ms")
            };
            format!("{} · 已连接 {lat}", truncate_chars(acct, 10))
        }
        (Some(s), Some(acct)) if s.state == MonitorState::Online => {
            format!("{} · {title}", truncate_chars(acct, 10))
        }
        (Some(s), Some(acct)) if !s.message.is_empty() && s.message != title => {
            format!(
                "{} · {}",
                truncate_chars(acct, 8),
                truncate_chars(&s.message, 12)
            )
        }
        (_, Some(acct)) => format!("{} · {title}", truncate_chars(acct, 10)),
        (Some(s), None) if s.state == MonitorState::Online && s.metrics.last_check_ms > 0 => {
            let ms = s.metrics.last_check_ms;
            let lat = if ms >= 1000 {
                format!("{:.1}s", ms as f64 / 1000.0)
            } else {
                format!("{ms} ms")
            };
            format!("{title} · 网关延迟 {lat}")
        }
        (Some(s), None) if !s.message.is_empty() && s.message != title => {
            format!("{title} · {}", truncate_chars(&s.message, 16))
        }
        (_, None) => title.to_owned(),
    };

    let mut items = vec![
        MenuItemSpec {
            action: TrayAction::Show,
            label: "显示主窗口",
            icon: ItemIcon::Window,
            is_primary: true,
            is_danger: false,
            separator_before: true,
            rect: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
        },
        MenuItemSpec {
            action: TrayAction::Check,
            label: "立即检测",
            icon: ItemIcon::Refresh,
            is_primary: false,
            is_danger: false,
            separator_before: false,
            rect: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
        },
    ];

    if running {
        items.push(MenuItemSpec {
            action: TrayAction::Pause,
            label: "暂停监控",
            icon: ItemIcon::Pause,
            is_primary: false,
            is_danger: false,
            separator_before: false,
            rect: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
        });
    } else {
        items.push(MenuItemSpec {
            action: TrayAction::Start,
            label: "开始监控",
            icon: ItemIcon::Play,
            is_primary: false,
            is_danger: false,
            separator_before: false,
            rect: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
        });
    }

    items.push(MenuItemSpec {
        action: TrayAction::OpenLog,
        label: "打开运行日志",
        icon: ItemIcon::Log,
        is_primary: false,
        is_danger: false,
        separator_before: true,
        rect: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
    });
    items.push(MenuItemSpec {
        action: TrayAction::Exit,
        label: concat!("退出 ", env!("CARGO_PKG_DESCRIPTION")),
        icon: ItemIcon::Exit,
        is_primary: false,
        is_danger: true,
        separator_before: false,
        rect: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
    });

    state.items = items;
}

fn compute_layout(state: &mut MenuState, dpi: u32) -> (i32, i32) {
    let win_w = scale(256, dpi);
    let pad_x = scale(6, dpi);
    let pad_y = scale(6, dpi);
    let header_h = scale(48, dpi);
    let item_h = scale(34, dpi);
    let item_gap = scale(2, dpi);
    let sep_h = scale(9, dpi);

    let mut y = pad_y + header_h;
    for item in &mut state.items {
        if item.separator_before {
            y += sep_h;
        }
        item.rect = RECT {
            left: pad_x,
            top: y,
            right: win_w - pad_x,
            bottom: y + item_h,
        };
        y += item_h + item_gap;
    }
    let total_h = y - item_gap + pad_y;
    (win_w, total_h)
}

unsafe extern "system" fn menu_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut MenuState;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = &mut *ptr;

    match msg {
        WM_ERASEBKGND => 1,
        WM_ACTIVATE => {
            if (wparam & 0xFFFF) as u32 == WA_INACTIVE {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_KILLFOCUS => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_KEYDOWN => {
            if wparam as u16 == VK_ESCAPE {
                ShowWindow(hwnd, SW_HIDE);
            }
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
            let mx = (lparam & 0xFFFF) as i16 as i32;
            let my = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let new_hover = hit_test(&state.items, mx, my);
            if new_hover != state.hovered {
                state.hovered = new_hover;
                InvalidateRect(hwnd, null(), 0);
            }
            let cursor = if new_hover.is_some() {
                IDC_HAND
            } else {
                IDC_ARROW
            };
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
        WM_LBUTTONUP | WM_RBUTTONUP => {
            let mx = (lparam & 0xFFFF) as i16 as i32;
            let my = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            if let Some(idx) = hit_test(&state.items, mx, my) {
                let action = state.items[idx].action.clone();
                ShowWindow(hwnd, SW_HIDE);
                let cb = Arc::clone(&state.on_action);
                cb(action);
            }
            0
        }
        WM_PAINT => {
            paint_menu(hwnd, state);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn paint_menu(hwnd: HWND, state: &MenuState) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    if hdc.is_null() {
        return;
    }
    let w = ps.rcPaint.right - ps.rcPaint.left;
    let h = ps.rcPaint.bottom - ps.rcPaint.top;
    if w <= 0 || h <= 0 {
        EndPaint(hwnd, &ps);
        return;
    }

    let dpi = state.dpi;
    let p = state.palette;

    // 双缓冲离屏绘制，杜绝悬停闪烁
    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mem_dc, mem_bmp);

    let full_rect = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    let bg_brush = CreateSolidBrush(p.bg);
    FillRect(mem_dc, &full_rect, bg_brush);
    DeleteObject(bg_brush);

    SetBkMode(mem_dc, TRANSPARENT as i32);

    let font_title = create_ui_font(scale(14, dpi), true);
    let font_sub = create_ui_font(scale(11, dpi), false);
    let font_item = create_ui_font(scale(13, dpi), false);
    let font_item_bold = create_ui_font(scale(13, dpi), true);

    // ── 1. 顶部实时状态区 ────────────────────────────────────────────
    let pad_x = scale(14, dpi);
    let pad_y = scale(6, dpi);
    let header_h = scale(48, dpi);
    let dot_cx = pad_x + scale(8, dpi);
    let dot_cy = pad_y + header_h / 2;

    // 柔和状态光晕 + 实心状态圆点
    fill_circle(mem_dc, dot_cx, dot_cy, scale(8, dpi), state.status_halo);
    fill_circle(mem_dc, dot_cx, dot_cy, scale(4, dpi), state.status_color);

    let text_left = pad_x + scale(24, dpi);
    let mut r_title = RECT {
        left: text_left,
        top: pad_y + scale(6, dpi),
        right: w - scale(12, dpi),
        bottom: pad_y + scale(26, dpi),
    };
    let old_font = SelectObject(mem_dc, font_title);
    SetTextColor(mem_dc, p.text);
    let title_w = to_wide(crate::APP_NAME);
    DrawTextW(
        mem_dc,
        title_w.as_ptr(),
        (title_w.len() - 1) as i32,
        &mut r_title,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    let mut r_sub = RECT {
        left: text_left,
        top: pad_y + scale(26, dpi),
        right: w - scale(12, dpi),
        bottom: pad_y + scale(44, dpi),
    };
    SelectObject(mem_dc, font_sub);
    SetTextColor(mem_dc, p.text_muted);
    let sub_w = to_wide(&state.header_sub);
    DrawTextW(
        mem_dc,
        sub_w.as_ptr(),
        (sub_w.len() - 1) as i32,
        &mut r_sub,
        DT_END_ELLIPSIS | DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    // ── 2. 菜单项 + 细分隔线 ─────────────────────────────────────────
    let div_pen = CreatePen(PS_SOLID, 1, p.divider);
    let radius = scale(12, dpi);

    for (idx, item) in state.items.iter().enumerate() {
        if item.separator_before {
            let sep_y = item.rect.top - scale(5, dpi);
            let old_p = SelectObject(mem_dc, div_pen);
            MoveToEx(mem_dc, scale(12, dpi), sep_y, null_mut());
            LineTo(mem_dc, w - scale(12, dpi), sep_y);
            SelectObject(mem_dc, old_p);
        }

        let is_hovered = state.hovered == Some(idx);
        if is_hovered {
            let fill_c = if item.is_danger {
                p.danger_hover_bg
            } else {
                p.hover_bg
            };
            let h_brush = CreateSolidBrush(fill_c);
            let null_pen = windows_sys::Win32::Graphics::Gdi::GetStockObject(NULL_PEN);
            let old_b = SelectObject(mem_dc, h_brush);
            let old_p = SelectObject(mem_dc, null_pen);
            RoundRect(
                mem_dc,
                item.rect.left,
                item.rect.top,
                item.rect.right,
                item.rect.bottom,
                radius,
                radius,
            );
            SelectObject(mem_dc, old_p);
            SelectObject(mem_dc, old_b);
            DeleteObject(h_brush);
        }

        let fg = if item.is_danger {
            p.danger
        } else if item.is_primary {
            p.accent
        } else {
            p.text
        };

        let icon_cx = item.rect.left + scale(18, dpi);
        let icon_cy = (item.rect.top + item.rect.bottom) / 2;
        draw_item_icon(mem_dc, item.icon, icon_cx, icon_cy, dpi, fg);

        SelectObject(
            mem_dc,
            if item.is_primary {
                font_item_bold
            } else {
                font_item
            },
        );
        SetTextColor(mem_dc, fg);
        let mut r_lbl = RECT {
            left: item.rect.left + scale(36, dpi),
            top: item.rect.top,
            right: item.rect.right - scale(10, dpi),
            bottom: item.rect.bottom,
        };
        let lbl_w = to_wide(item.label);
        DrawTextW(
            mem_dc,
            lbl_w.as_ptr(),
            (lbl_w.len() - 1) as i32,
            &mut r_lbl,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }

    SelectObject(mem_dc, old_font);
    DeleteObject(div_pen);
    DeleteObject(font_title);
    DeleteObject(font_sub);
    DeleteObject(font_item);
    DeleteObject(font_item_bold);

    BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
    SelectObject(mem_dc, old_bmp);
    DeleteObject(mem_bmp);
    DeleteDC(mem_dc);
    EndPaint(hwnd, &ps);
}

unsafe fn draw_item_icon(
    hdc: *mut c_void,
    icon: ItemIcon,
    cx: i32,
    cy: i32,
    dpi: u32,
    color: COLORREF,
) {
    let stroke = scale(1, dpi).max(1);
    let pen = CreatePen(PS_SOLID, stroke, color);
    let null_brush = windows_sys::Win32::Graphics::Gdi::GetStockObject(
        windows_sys::Win32::Graphics::Gdi::NULL_BRUSH,
    );
    let old_p = SelectObject(hdc, pen);
    let old_b = SelectObject(hdc, null_brush);
    let s = scale(6, dpi);

    match icon {
        ItemIcon::Window => {
            RoundRect(
                hdc,
                cx - s,
                cy - s + 1,
                cx + s + 1,
                cy + s,
                scale(3, dpi),
                scale(3, dpi),
            );
            MoveToEx(hdc, cx - s, cy - s / 2 + 1, null_mut());
            LineTo(hdc, cx + s + 1, cy - s / 2 + 1);
        }
        ItemIcon::Refresh => {
            Ellipse(hdc, cx - s + 1, cy - s + 1, cx + s, cy + s);
            // 小箭头指向右上方
            MoveToEx(hdc, cx + s - 2, cy - s + 1, null_mut());
            LineTo(hdc, cx + s, cy - s + 4);
        }
        ItemIcon::Pause => {
            let g = scale(2, dpi).max(2);
            let h = scale(5, dpi);
            let b = CreateSolidBrush(color);
            let r1 = RECT {
                left: cx - g - stroke,
                top: cy - h,
                right: cx - g + stroke,
                bottom: cy + h,
            };
            let r2 = RECT {
                left: cx + g - stroke,
                top: cy - h,
                right: cx + g + stroke,
                bottom: cy + h,
            };
            FillRect(hdc, &r1, b);
            FillRect(hdc, &r2, b);
            DeleteObject(b);
        }
        ItemIcon::Play => {
            let h = scale(5, dpi);
            MoveToEx(hdc, cx - s / 2, cy - h, null_mut());
            LineTo(hdc, cx + s - 1, cy);
            LineTo(hdc, cx - s / 2, cy + h);
            LineTo(hdc, cx - s / 2, cy - h);
        }
        ItemIcon::Log => {
            RoundRect(
                hdc,
                cx - s + 1,
                cy - s,
                cx + s,
                cy + s + 1,
                scale(2, dpi),
                scale(2, dpi),
            );
            let y1 = cy - scale(2, dpi);
            let y2 = cy + scale(1, dpi);
            MoveToEx(hdc, cx - s / 2, y1, null_mut());
            LineTo(hdc, cx + s / 2 + 1, y1);
            MoveToEx(hdc, cx - s / 2, y2, null_mut());
            LineTo(hdc, cx + s / 2 - 1, y2);
        }
        ItemIcon::Exit => {
            // 简洁电源/退出图标：左门框 + 向右箭头
            let h = scale(5, dpi);
            MoveToEx(hdc, cx, cy - h, null_mut());
            LineTo(hdc, cx - s + 1, cy - h);
            LineTo(hdc, cx - s + 1, cy + h);
            LineTo(hdc, cx, cy + h);
            MoveToEx(hdc, cx - 1, cy, null_mut());
            LineTo(hdc, cx + s, cy);
            MoveToEx(hdc, cx + s - scale(3, dpi), cy - scale(3, dpi), null_mut());
            LineTo(hdc, cx + s, cy);
            LineTo(hdc, cx + s - scale(3, dpi), cy + scale(3, dpi));
        }
    }

    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    DeleteObject(pen);
}

unsafe fn fill_circle(hdc: *mut c_void, cx: i32, cy: i32, r: i32, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    let null_pen = windows_sys::Win32::Graphics::Gdi::GetStockObject(NULL_PEN);
    let old_b = SelectObject(hdc, brush);
    let old_p = SelectObject(hdc, null_pen);
    Ellipse(hdc, cx - r, cy - r, cx + r + 1, cy + r + 1);
    SelectObject(hdc, old_p);
    SelectObject(hdc, old_b);
    DeleteObject(brush);
}

unsafe fn create_ui_font(px_height: i32, bold: bool) -> *mut c_void {
    let face = to_wide("Microsoft YaHei UI");
    CreateFontW(
        -px_height,
        0,
        0,
        0,
        if bold {
            FW_SEMIBOLD as i32
        } else {
            FW_NORMAL as i32
        },
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        face.as_ptr(),
    )
}

fn hit_test(items: &[MenuItemSpec], x: i32, y: i32) -> Option<usize> {
    items.iter().position(|item| {
        x >= item.rect.left && x < item.rect.right && y >= item.rect.top && y < item.rect.bottom
    })
}

fn detect_dark_mode(app_dir: &Path) -> bool {
    let prefs_path = app_dir.join("ui_prefs.json");
    if let Ok(raw) = std::fs::read_to_string(prefs_path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(mode) = val.get("theme").and_then(|v| v.as_str()) {
                return mode == "dark";
            }
        }
    }
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    if let Ok(key) =
        hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
    {
        if let Ok(val) = key.get_value::<u32, _>("AppsUseLightTheme") {
            return val == 0;
        }
    }
    false
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

fn scale(logical: i32, dpi: u32) -> i32 {
    ((logical as f32) * (dpi as f32 / 96.0)).round() as i32
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
