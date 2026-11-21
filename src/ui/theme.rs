//! 双主题设计令牌、Windows 系统主题检测、UI 偏好持久化与字体栈安装。
//!
//! 视觉语言：现代 Windows 11 Fluent 清爽蓝调质感。
//! - 中性淡冷灰层次替代发闷配色，与晴空蓝独秀峰图标自然呼应
//! - 克制的交互主色（Fluent 蓝 `#2563EB` / `#3B82F6`），语义状态色仅用于状态反馈
//! - 单一微软雅黑 Regular + Bold 字体栈，中英文与数字基线严格齐平

use std::path::Path;
use std::sync::Arc;

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::models::{MonitorMetrics, MonitorState, StepState};

pub const SPACE_XS: f32 = 4.0;
pub const SPACE_SM: f32 = 8.0;
pub const SPACE_MD: f32 = 14.0;
pub const SPACE_LG: f32 = 20.0;

pub const RADIUS_SM: u8 = 6;
pub const RADIUS_MD: u8 = 10;
pub const RADIUS_LG: u8 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
}

impl ThemeMode {
    pub fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UiPrefs {
    theme: ThemeMode,
}

/// 加载 UI 主题偏好（存于 `ui_prefs.json`）。
pub fn load_theme_mode(app_dir: &Path) -> ThemeMode {
    if let Ok(env_theme) = std::env::var("GXNU_THEME") {
        match env_theme.to_lowercase().as_str() {
            "dark" => return ThemeMode::Dark,
            "light" => return ThemeMode::Light,
            _ => {}
        }
    }
    let prefs_path = app_dir.join("ui_prefs.json");
    if let Ok(text) = std::fs::read_to_string(&prefs_path) {
        if let Ok(prefs) = serde_json::from_str::<UiPrefs>(&text) {
            return prefs.theme;
        }
    }
    detect_windows_theme()
}

pub fn save_theme_mode(app_dir: &Path, mode: ThemeMode) {
    let prefs_path = app_dir.join("ui_prefs.json");
    if let Ok(text) = serde_json::to_string_pretty(&UiPrefs { theme: mode }) {
        let _ = std::fs::write(prefs_path, text);
    }
}

fn detect_windows_theme() -> ThemeMode {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    if let Ok(subkey) = key {
        if let Ok(value) = subkey.get_value::<u32, _>("AppsUseLightTheme") {
            return if value == 0 {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            };
        }
    }
    ThemeMode::Light
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub mode: ThemeMode,
    pub bg: egui::Color32,
    pub sidebar: egui::Color32,
    pub titlebar: egui::Color32,
    pub card: egui::Color32,
    pub card_hover: egui::Color32,
    pub subtle: egui::Color32,
    pub input_bg: egui::Color32,
    pub border: egui::Color32,
    pub border_strong: egui::Color32,
    pub window_border: egui::Color32,
    pub divider: egui::Color32,
    pub text: egui::Color32,
    pub text_secondary: egui::Color32,
    pub text_muted: egui::Color32,
    pub accent: egui::Color32,
    pub accent_hover: egui::Color32,
    pub accent_soft: egui::Color32,
    pub accent_border: egui::Color32,
    pub on_accent: egui::Color32,
    pub ok: egui::Color32,
    pub ok_soft: egui::Color32,
    pub ok_border: egui::Color32,
    pub warn: egui::Color32,
    pub warn_soft: egui::Color32,
    pub warn_border: egui::Color32,
    pub danger: egui::Color32,
    pub danger_soft: egui::Color32,
    pub danger_border: egui::Color32,
    pub neutral: egui::Color32,
    pub neutral_soft: egui::Color32,
    pub neutral_border: egui::Color32,
    pub shadow: egui::Color32,
    pub toggle_off: egui::Color32,
    pub toggle_knob: egui::Color32,
    pub chart_ema: egui::Color32,
}

impl Palette {
    pub fn of(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
        }
    }

    pub fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            bg: egui::Color32::from_rgb(0xF5, 0xF6, 0xF8),
            sidebar: egui::Color32::from_rgb(0xEC, 0xEF, 0xF4),
            titlebar: egui::Color32::from_rgb(0xEC, 0xEF, 0xF4),
            card: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
            card_hover: egui::Color32::from_rgb(0xF8, 0xFA, 0xFC),
            subtle: egui::Color32::from_rgb(0xF1, 0xF4, 0xF8),
            input_bg: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
            border: egui::Color32::from_rgb(0xE2, 0xE6, 0xEE),
            border_strong: egui::Color32::from_rgb(0xCB, 0xD2, 0xDE),
            window_border: egui::Color32::from_rgb(0xC8, 0xCF, 0xDC),
            divider: egui::Color32::from_rgb(0xEA, 0xEE, 0xF4),
            text: egui::Color32::from_rgb(0x11, 0x18, 0x27),
            text_secondary: egui::Color32::from_rgb(0x4B, 0x55, 0x63),
            text_muted: egui::Color32::from_rgb(0x73, 0x7D, 0x8F),
            accent: egui::Color32::from_rgb(0x25, 0x63, 0xEB),
            accent_hover: egui::Color32::from_rgb(0x1D, 0x4E, 0xD8),
            accent_soft: egui::Color32::from_rgb(0xEB, 0xF3, 0xFF),
            accent_border: egui::Color32::from_rgb(0xBF, 0xDB, 0xFE),
            on_accent: egui::Color32::WHITE,
            ok: egui::Color32::from_rgb(0x05, 0x96, 0x69),
            ok_soft: egui::Color32::from_rgb(0xEC, 0xFD, 0xF5),
            ok_border: egui::Color32::from_rgb(0xA7, 0xF3, 0xD0),
            warn: egui::Color32::from_rgb(0xD9, 0x77, 0x06),
            warn_soft: egui::Color32::from_rgb(0xFF, 0xFB, 0xEB),
            warn_border: egui::Color32::from_rgb(0xFD, 0xE6, 0x8A),
            danger: egui::Color32::from_rgb(0xDC, 0x26, 0x26),
            danger_soft: egui::Color32::from_rgb(0xFE, 0xF2, 0xF2),
            danger_border: egui::Color32::from_rgb(0xFE, 0xCA, 0xCA),
            neutral: egui::Color32::from_rgb(0x64, 0x74, 0x8B),
            neutral_soft: egui::Color32::from_rgb(0xF1, 0xF5, 0xF9),
            neutral_border: egui::Color32::from_rgb(0xCB, 0xD5, 0xE1),
            shadow: egui::Color32::from_rgba_unmultiplied(15, 23, 42, 14),
            toggle_off: egui::Color32::from_rgb(0xCB, 0xD2, 0xDF),
            toggle_knob: egui::Color32::WHITE,
            chart_ema: egui::Color32::from_rgb(0x0E, 0xA5, 0xE9),
        }
    }

    pub fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            bg: egui::Color32::from_rgb(0x13, 0x15, 0x19),
            sidebar: egui::Color32::from_rgb(0x19, 0x1C, 0x21),
            titlebar: egui::Color32::from_rgb(0x19, 0x1C, 0x21),
            card: egui::Color32::from_rgb(0x1F, 0x22, 0x29),
            card_hover: egui::Color32::from_rgb(0x26, 0x2A, 0x33),
            subtle: egui::Color32::from_rgb(0x18, 0x1A, 0x20),
            input_bg: egui::Color32::from_rgb(0x15, 0x17, 0x1C),
            border: egui::Color32::from_rgb(0x2C, 0x30, 0x3A),
            border_strong: egui::Color32::from_rgb(0x3B, 0x41, 0x4E),
            window_border: egui::Color32::from_rgb(0x36, 0x3B, 0x47),
            divider: egui::Color32::from_rgb(0x28, 0x2C, 0x36),
            text: egui::Color32::from_rgb(0xF3, 0xF4, 0xF6),
            text_secondary: egui::Color32::from_rgb(0xA6, 0xAD, 0xBB),
            text_muted: egui::Color32::from_rgb(0x74, 0x7C, 0x8D),
            accent: egui::Color32::from_rgb(0x3B, 0x82, 0xF6),
            accent_hover: egui::Color32::from_rgb(0x60, 0xA5, 0xFA),
            accent_soft: egui::Color32::from_rgb(0x1C, 0x2A, 0x44),
            accent_border: egui::Color32::from_rgb(0x2A, 0x45, 0x75),
            on_accent: egui::Color32::WHITE,
            ok: egui::Color32::from_rgb(0x10, 0xB9, 0x81),
            ok_soft: egui::Color32::from_rgb(0x11, 0x2D, 0x26),
            ok_border: egui::Color32::from_rgb(0x19, 0x52, 0x42),
            warn: egui::Color32::from_rgb(0xF5, 0x9E, 0x0B),
            warn_soft: egui::Color32::from_rgb(0x31, 0x25, 0x13),
            warn_border: egui::Color32::from_rgb(0x5C, 0x41, 0x18),
            danger: egui::Color32::from_rgb(0xF8, 0x71, 0x71),
            danger_soft: egui::Color32::from_rgb(0x33, 0x1A, 0x1D),
            danger_border: egui::Color32::from_rgb(0x5E, 0x28, 0x2E),
            neutral: egui::Color32::from_rgb(0x94, 0xA3, 0xB8),
            neutral_soft: egui::Color32::from_rgb(0x25, 0x29, 0x32),
            neutral_border: egui::Color32::from_rgb(0x38, 0x3E, 0x4B),
            shadow: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 60),
            toggle_off: egui::Color32::from_rgb(0x3C, 0x42, 0x50),
            toggle_knob: egui::Color32::from_rgb(0xF8, 0xFA, 0xFC),
            chart_ema: egui::Color32::from_rgb(0x38, 0xBD, 0xF8),
        }
    }

    pub fn state_colors(
        &self,
        state: Option<MonitorState>,
    ) -> (egui::Color32, egui::Color32, egui::Color32) {
        match state {
            Some(MonitorState::Online) => (self.ok, self.ok_soft, self.ok_border),
            Some(MonitorState::Authenticating)
            | Some(MonitorState::Verifying)
            | Some(MonitorState::Suspect)
            | Some(MonitorState::Checking) => (self.warn, self.warn_soft, self.warn_border),
            Some(MonitorState::Offline) => (self.danger, self.danger_soft, self.danger_border),
            _ => (self.neutral, self.neutral_soft, self.neutral_border),
        }
    }

    pub fn step_colors(&self, state: StepState) -> (egui::Color32, egui::Color32, egui::Color32) {
        match state {
            StepState::Ok => (self.ok, self.ok_soft, self.ok_border),
            StepState::Warn => (self.warn, self.warn_soft, self.warn_border),
            StepState::Fail => (self.danger, self.danger_soft, self.danger_border),
            StepState::Pending | StepState::Skipped => {
                (self.neutral, self.neutral_soft, self.neutral_border)
            }
        }
    }
}

pub fn with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    egui::Color32::from_rgba_unmultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

/// 粗体字体族名（Segoe UI Bold + 微软雅黑 Bold）。
pub fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name(Arc::from("bold"))
}

pub fn bold_text(text: impl Into<String>, size: f32, color: egui::Color32) -> egui::RichText {
    egui::RichText::new(text)
        .family(bold_family())
        .size(size)
        .color(color)
}

pub fn body_text(text: impl Into<String>, size: f32, color: egui::Color32) -> egui::RichText {
    egui::RichText::new(text).size(size).color(color)
}

pub fn mono_text(text: impl Into<String>, size: f32, color: egui::Color32) -> egui::RichText {
    egui::RichText::new(text)
        .monospace()
        .size(size)
        .color(color)
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let load_with_tweak = |fonts: &mut egui::FontDefinitions,
                           key: &str,
                           path: &str,
                           index: u32,
                           tweak: egui::FontTweak|
     -> bool {
        if let Ok(bytes) = std::fs::read(path) {
            let mut data = egui::FontData::from_owned(bytes).tweak(tweak);
            data.index = index;
            fonts.font_data.insert(key.to_owned(), data.into());
            true
        } else {
            false
        }
    };

    let default_tweak = egui::FontTweak::default();

    let has_consolas = load_with_tweak(
        &mut fonts,
        "consolas",
        r"C:\Windows\Fonts\consola.ttf",
        0,
        default_tweak,
    );

    // 主字体直接使用单一系统微软雅黑（自身已内置与中文完全对齐的西文与数字字形），
    // 彻底杜绝 Segoe UI 与微软雅黑混排导致的中英/数字基线高低偏差。
    let cjk_path = [
        (r"C:\Windows\Fonts\msyh.ttc", 0u32),
        (r"C:\Windows\Fonts\Deng.ttf", 0u32),
        (r"C:\Windows\Fonts\simhei.ttf", 0u32),
    ]
    .into_iter()
    .find(|(path, _)| Path::new(path).exists());

    let cjk_regular = if let Some((path, idx)) = cjk_path {
        if load_with_tweak(&mut fonts, "cjk-reg", path, idx, default_tweak) {
            Some("cjk-reg")
        } else {
            None
        }
    } else {
        None
    };

    // 粗体同样优先使用单一微软雅黑粗体（仅在按需打开的 --ui 进程中加载，关窗即释放，守护进程 0 占用）
    let cjk_bold = if load_with_tweak(
        &mut fonts,
        "cjk-bold",
        r"C:\Windows\Fonts\msyhbd.ttc",
        0,
        default_tweak,
    ) || load_with_tweak(
        &mut fonts,
        "cjk-bold",
        r"C:\Windows\Fonts\Dengb.ttf",
        0,
        default_tweak,
    ) {
        Some("cjk-bold")
    } else {
        cjk_regular
    };

    // Proportional: 微软雅黑常规体统一渲染中文、英文与数字
    let prop = fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default();
    if let Some(cjk) = cjk_regular {
        prop.insert(0, cjk.to_owned());
    }

    // Bold family: 微软雅黑粗体统一渲染粗体中英文与数字
    let mut bold_list = Vec::new();
    if let Some(cjk) = cjk_bold {
        bold_list.push(cjk.to_owned());
    }
    if bold_list.is_empty() {
        bold_list = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
    }
    fonts.families.insert(bold_family(), bold_list);

    // Monospace: Consolas 优先渲染时间与标签，已校准的 cjk-mono 承接中文
    let mono = fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default();
    let mut mono_prefix = Vec::new();
    if has_consolas {
        mono_prefix.push("consolas".to_owned());
    }
    if fonts.font_data.contains_key("cjk-mono") {
        mono_prefix.push("cjk-mono".to_owned());
    } else if let Some(cjk) = cjk_regular {
        mono_prefix.push(cjk.to_owned());
    }
    for (idx, name) in mono_prefix.into_iter().enumerate() {
        mono.insert(idx, name);
    }

    if cjk_regular.is_some() {
        crate::logging::info("[UI] 已加载中文字体（含粗体与等宽栈）");
    } else {
        crate::logging::warn("[UI] 未找到中文字体，界面中文可能显示为方块");
    }
    ctx.set_fonts(fonts);
}

pub fn apply_theme(ctx: &egui::Context, p: &Palette) {
    let mut visuals = match p.mode {
        ThemeMode::Light => egui::Visuals::light(),
        ThemeMode::Dark => egui::Visuals::dark(),
    };

    visuals.panel_fill = p.bg;
    visuals.window_fill = p.card;
    visuals.extreme_bg_color = p.input_bg;
    visuals.faint_bg_color = p.subtle;
    visuals.window_stroke = egui::Stroke::new(1.0_f32, p.border);
    visuals.selection.bg_fill = with_alpha(p.accent, 64);
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, p.accent);
    visuals.hyperlink_color = p.accent;
    visuals.override_text_color = Some(p.text);

    visuals.widgets.noninteractive.bg_fill = p.subtle;
    visuals.widgets.noninteractive.weak_bg_fill = p.subtle;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, p.border);

    visuals.widgets.inactive.bg_fill = p.card;
    visuals.widgets.inactive.weak_bg_fill = p.card;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, p.border);

    visuals.widgets.hovered.bg_fill = p.card_hover;
    visuals.widgets.hovered.weak_bg_fill = p.card_hover;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, p.border_strong);

    visuals.widgets.active.bg_fill = p.accent_soft;
    visuals.widgets.active.weak_bg_fill = p.accent_soft;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, p.accent);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, p.accent);

    visuals.widgets.open.bg_fill = p.card;
    visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0_f32, p.accent);

    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(RADIUS_SM);
    }

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(SPACE_SM, SPACE_SM);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size = egui::vec2(32.0, 28.0);
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(18.0, bold_family()),
    );
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, egui::FontId::monospace(12.0));
    ctx.set_style(style);
}

pub fn status_title(state: Option<MonitorState>) -> &'static str {
    match state {
        Some(MonitorState::Online) => "校园网已连接",
        Some(MonitorState::Offline) => "网络连接异常",
        Some(MonitorState::Suspect) => "检测到波动，复核中",
        Some(MonitorState::Authenticating) => "正在自动认证",
        Some(MonitorState::Verifying) => "正在复验公网",
        Some(MonitorState::Checking) => "正在诊断链路",
        Some(MonitorState::Paused) => "守护已暂停",
        Some(MonitorState::Stopped) => "守护已停止",
        None => "正在初始化",
    }
}

pub fn status_short(state: Option<MonitorState>) -> &'static str {
    match state {
        Some(MonitorState::Online) => "在线",
        Some(MonitorState::Offline) => "离线",
        Some(MonitorState::Suspect) => "复核中",
        Some(MonitorState::Authenticating) => "登录中",
        Some(MonitorState::Verifying) => "复验中",
        Some(MonitorState::Checking) => "检测中",
        Some(MonitorState::Paused) => "已暂停",
        Some(MonitorState::Stopped) => "已停止",
        None => "启动中",
    }
}

pub fn carrier_label(carrier: &str) -> &'static str {
    match carrier {
        "0" => "校园用户",
        "1" => "中国电信",
        "2" => "中国联通",
        "3" => "中国移动",
        _ => "中国联通",
    }
}

pub fn mask_student_id(student_id: &str) -> String {
    if student_id.is_empty() {
        return "未配置学号".to_string();
    }
    let chars: Vec<char> = student_id.chars().collect();
    if chars.len() <= 4 {
        return student_id.to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len().saturating_sub(2)..].iter().collect();
    format!("{head}****{tail}")
}

pub fn format_ip(metrics: &MonitorMetrics) -> String {
    if metrics.local_ip.is_empty() {
        "—".to_string()
    } else {
        metrics.local_ip.clone()
    }
}

pub fn format_gateway(metrics: &MonitorMetrics) -> String {
    if metrics.gateway.is_empty() {
        "—".to_string()
    } else {
        metrics.gateway.clone()
    }
}

pub fn format_check_ms(metrics: &MonitorMetrics) -> String {
    if metrics.last_check_ms == 0 {
        "—".to_string()
    } else if metrics.last_check_ms >= 1000 {
        format!("{:.2}s", metrics.last_check_ms as f64 / 1000.0)
    } else {
        format!("{} ms", metrics.last_check_ms)
    }
}

pub fn format_check_time(metrics: &MonitorMetrics) -> String {
    if metrics.last_check_text.is_empty() {
        "—".to_string()
    } else {
        metrics.last_check_text.clone()
    }
}

pub fn format_uptime(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// 拆解 `2000-01-01 12:00:00 INFO [TAG] message` 为 `(时间, 级别, 标签, 正文)`。
pub fn parse_log_line(line: &str) -> (String, String, Option<String>, String) {
    if line.len() >= 20 && line.as_bytes().get(10) == Some(&b' ') {
        let stamp = line.get(11..19).unwrap_or("").to_string();
        let rest = &line[20..];
        let (level, body) = rest.split_once(' ').unwrap_or(("", rest));
        let trimmed = body.trim_start();
        if let Some(after_open) = trimmed.strip_prefix('[') {
            if let Some((tag, msg)) = after_open.split_once(']') {
                return (
                    stamp,
                    level.to_string(),
                    Some(tag.to_string()),
                    msg.trim_start().to_string(),
                );
            }
        }
        return (stamp, level.to_string(), None, body.to_string());
    }
    (String::new(), String::new(), None, line.to_string())
}

pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut iter = text.chars();
    let collected: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

/// 将断网原因码映射为大白话中文（用于图表悬停详情与断网记录）。
pub fn reason_label(code: &str) -> &'static str {
    match code {
        "ONLINE" => "网络正常",
        "LINK_DOWN" => "Wi-Fi 或网线断开",
        "NO_IP" => "未获取到 IP 地址",
        "NO_GATEWAY" => "未获取到默认网关",
        "GATEWAY_UNREACHABLE" => "路由器/网关无响应",
        "DNS_FAILURE" => "域名解析失败 (DNS)",
        "AUTH_EXPIRED" => "校园网认证掉线",
        "INTERNET_TIMEOUT" => "访问公网超时",
        "UPLINK_UNREACHABLE" => "校园网出口异常",
        "LOGIN_REJECTED" => "账号认证被拒绝",
        "PORTAL_UNAVAILABLE" => "认证页面打不开",
        "CREDENTIALS_MISSING" => "尚未保存学号密码",
        _ => "网络连接中断",
    }
}

/// 断网原因的通俗解释（鼠标悬停断网记录时显示）。
pub fn reason_hint(code: &str) -> &'static str {
    match code {
        "LINK_DOWN" => "本机 Wi-Fi 已断开或网线松动，请检查无线连接状态",
        "NO_IP" => "网卡已连上但未分配到 IPv4 地址，可能是 DHCP 异常",
        "NO_GATEWAY" => "未获取到校园网默认网关，建议重连 Wi-Fi 或网口",
        "GATEWAY_UNREACHABLE" => "无法 ping 通校园网网关，可能是宿舍交换机或 AP 波动",
        "DNS_FAILURE" => "无法解析公网域名，可能是 DNS 异常或被劫持",
        "AUTH_EXPIRED" => "校园网认证会话已过期或被挤下线，守护程序会自动重登",
        "INTERNET_TIMEOUT" => "访问公网超时，通常是校园网出口拥堵或短暂抖动",
        "UPLINK_UNREACHABLE" => "校园网网关在线，但外网出口链路暂时不可达",
        "LOGIN_REJECTED" => "门户拒绝了登录请求，请检查学号、密码或运营商出口设置",
        "PORTAL_UNAVAILABLE" => "校园网认证服务器暂时无响应，稍后会自动重试",
        "CREDENTIALS_MISSING" => "检测到需要认证，但尚未在设置中保存学号和密码",
        _ => "检测到公网不可达，程序会自动复核并尝试恢复连接",
    }
}

pub fn format_ms_compact(ms: u64) -> String {
    if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}
