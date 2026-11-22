//! 嵌入式运行日志卡（位于主页下半屏）：
//! - 经典三层英文标签：`全部 / INFO / WARN / ERR`，顶部分类与列表左侧胶囊 1 对 1 严格对应
//! - 深度降噪：自动过滤弹窗发送回执（`NOTIFY_SENT`）、登录中间握手态与重复状态行（1 次重连从 9 条压至 2 条核心记录）
//! - 右上角轻量等高搜索框（纯 Painter 24px 高度 + 零内边距中线锁定，图标与文字严格水平对齐）
//! - 严格单行限宽截断；超长或含折叠详情时鼠标悬停弹出结构化小卡片

use std::path::Path;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::{body_text, bold_text, parse_log_line, reason_label, with_alpha, Palette};
use crate::ui::widgets::{badge, card_padded, icon_button, section_heading, segmented_control};

#[derive(Default)]
pub struct LogsState {
    /// 0 = 全部，1 = INFO，2 = WARN，3 = ERR
    pub view_filter: usize,
    pub search: String,
}

/// 严格 1 对 1 的三层日志等级（1 种颜色 = 1 个英文标签 = 顶部 1 个筛选项）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogLevel {
    /// 绿：一切正常运行状态、重连成功、切号成功、检测通过、守护启停与设置生效
    Info,
    /// 黄：一切掉线触发、认证过期自动重连中、网络波动复核、等待重试
    Warn,
    /// 红：一切认证被拒、缺密码、切号失败、网卡断开或程序故障
    Err,
}

impl LogLevel {
    fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Err => "ERR",
        }
    }

    fn colors(self, p: &Palette) -> (Color32, Color32, Color32) {
        match self {
            Self::Info => (p.ok, p.ok_soft, p.ok_border),
            Self::Warn => (p.warn, p.warn_soft, p.warn_border),
            Self::Err => (p.danger, p.danger_soft, p.danger_border),
        }
    }
}

#[derive(Debug, Clone)]
struct HumanLogLine {
    full_stamp: String,
    time_short: String,
    level: LogLevel,
    summary: String,
    /// 若正文折叠了 IP/网关/路径等次要参数，放入此处供鼠标悬停小卡片查看。
    detail: Option<String>,
    raw: String,
}

pub fn draw_card(
    ui: &mut egui::Ui,
    p: &Palette,
    lines: &[String],
    state: &mut LogsState,
    log_path: &Path,
    target_height: f32,
) {
    let entries = build_visible_logs(lines);
    let query = state.search.trim().to_lowercase();
    let filtered: Vec<&HumanLogLine> = entries
        .iter()
        .filter(|entry| {
            let level_ok = match state.view_filter {
                1 => entry.level == LogLevel::Info,
                2 => entry.level == LogLevel::Warn,
                3 => entry.level == LogLevel::Err,
                _ => true,
            };
            if !level_ok {
                return false;
            }
            if query.is_empty() {
                return true;
            }
            entry.summary.to_lowercase().contains(&query)
                || entry.level.label().to_lowercase().contains(&query)
                || entry
                    .detail
                    .as_deref()
                    .is_some_and(|d| d.to_lowercase().contains(&query))
                || entry.raw.to_lowercase().contains(&query)
        })
        .collect();

    // 精确扣除所有内外边距与 1px 描边（外卡 12×2+2=26，工具栏 26，间距 7，内框 6×2+2=14）
    let inner_h = (target_height - 26.0).max(96.0);
    let scroll_h = (inner_h - 26.0 - 7.0 - 14.0).max(48.0);

    card_padded(ui, p, 12, |ui| {
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);
        ui.spacing_mut().item_spacing.y = 0.0;

        // ── 26px 统一中线工具栏：全部 / INFO / WARN / ERR + 紧凑等高搜索框 ──
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                section_heading(
                    ui,
                    Icon::Terminal,
                    p.text_secondary,
                    "运行日志",
                    p.text,
                    26.0,
                );

                let count_label = if filtered.len() == entries.len() {
                    format!("{} 条", entries.len())
                } else {
                    format!("{}/{}", filtered.len(), entries.len())
                };
                badge(ui, p.text_secondary, p.subtle, p.border, &count_label);

                let tabs = ["全部", "INFO", "WARN", "ERR"];
                segmented_control(ui, p, "inline_log_view", &mut state.view_filter, &tabs);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;

                    if icon_button(
                        ui,
                        p,
                        Icon::ExternalLink,
                        Vec2::splat(24.0),
                        12.5,
                        false,
                        "在文件管理器中定位 app.log",
                    )
                    .clicked()
                    {
                        crate::logging::reveal_in_explorer(log_path);
                    }

                    // 与左侧状态胶囊同体量（24px 高、96px 宽）且图标文字严格中线对齐的小搜索框
                    let box_w = (ui.available_width() - 4.0).clamp(76.0, 96.0);
                    if box_w >= 76.0 {
                        draw_aligned_mini_search(ui, p, &mut state.search, box_w);
                    }
                });
            },
        );

        ui.add_space(7.0);

        // ── 日志流圆角容器 ─────────────────────────────────────────────
        egui::Frame::new()
            .fill(p.subtle)
            .stroke(Stroke::new(1.0_f32, p.divider))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                let inner_w = ui.available_width();
                ui.set_min_size(Vec2::new(inner_w, scroll_h));
                ui.set_max_size(Vec2::new(inner_w, scroll_h));

                if filtered.is_empty() {
                    let empty_hint = match state.view_filter {
                        3 => "当前没有 ERR 故障记录",
                        2 => "当前没有 WARN 波动记录",
                        1 => "当前没有 INFO 运行记录",
                        _ => "没有匹配的日志记录",
                    };
                    ui.centered_and_justified(|ui| {
                        ui.label(body_text(empty_hint, 12.0, p.text_muted));
                    });
                    return;
                }

                let row_height = 23.0;
                egui::ScrollArea::vertical()
                    .id_salt("embedded_log_scroll")
                    .max_height(scroll_h)
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show_rows(ui, row_height, filtered.len(), |ui, range| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let row_w = (inner_w - 8.0).max(160.0);
                        for index in range {
                            paint_log_row(ui, p, filtered[index], index, row_height, row_w);
                        }
                    });
            });
    });
}

/// 纯 Painter 24px 迷你搜索框：与左侧徽章/分段器同高同体量，图标与文字强制共享同一水平中线 `cy`。
fn draw_aligned_mini_search(ui: &mut egui::Ui, p: &Palette, search: &mut String, width: f32) {
    let height = 24.0_f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let cy = rect.center().y;
    let active = !search.is_empty();

    let edit_id = ui.make_persistent_id("mini_log_search_edit");
    let focused = ui.ctx().memory(|m| m.has_focus(edit_id));
    let (fill, border_c) = if focused || active {
        (p.card, p.accent_border)
    } else {
        (p.subtle, p.border)
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(5),
        fill,
        Stroke::new(1.0_f32, border_c),
        StrokeKind::Inside,
    );

    // 左侧放大镜图标（严格居中于 cy）
    let icon_rect = Rect::from_center_size(Pos2::new(rect.left() + 12.0, cy), Vec2::splat(11.0));
    icons::paint(
        ui.painter(),
        icon_rect,
        Icon::Search,
        if focused || active {
            p.accent
        } else {
            p.text_muted
        },
    );

    let text_left = rect.left() + 21.0;
    let text_right = if active {
        rect.right() - 18.0
    } else {
        rect.right() - 6.0
    };

    // 空置且未聚焦时直接用 Painter 以 LEFT_CENTER 绘制占位文字，确保与放大镜图标 0 像素垂直误差
    if search.is_empty() && !focused {
        ui.painter().text(
            Pos2::new(text_left, cy),
            egui::Align2::LEFT_CENTER,
            "搜索…",
            egui::FontId::proportional(11.0),
            p.text_muted,
        );
    }

    let edit_rect = Rect::from_min_max(
        Pos2::new(text_left, cy - 8.0),
        Pos2::new(text_right.max(text_left + 24.0), cy + 8.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(edit_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().button_padding = Vec2::ZERO;
    let resp = child.add_sized(
        edit_rect.size(),
        egui::TextEdit::singleline(search)
            .id(edit_id)
            .font(egui::FontId::proportional(11.0))
            .vertical_align(egui::Align::Center)
            .frame(false),
    );
    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        search.clear();
        resp.surrender_focus();
    }

    if active {
        let clr_rect =
            Rect::from_center_size(Pos2::new(rect.right() - 10.0, cy), Vec2::splat(12.0));
        let clr_resp = ui
            .interact(
                clr_rect,
                ui.id().with("mini_log_search_clear"),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        icons::paint(
            ui.painter(),
            clr_rect.shrink(2.2),
            Icon::Close,
            if clr_resp.hovered() {
                p.text
            } else {
                p.text_muted
            },
        );
        if clr_resp.clicked() {
            search.clear();
        }
    }
}

fn paint_log_row(
    ui: &mut egui::Ui,
    p: &Palette,
    entry: &HumanLogLine,
    index: usize,
    row_height: f32,
    row_w: f32,
) {
    let (row_rect, resp) = ui.allocate_exact_size(Vec2::new(row_w, row_height), Sense::click());
    let hovered = resp.hovered();
    if hovered {
        ui.painter().rect_filled(
            row_rect.shrink2(Vec2::new(0.0, 1.0)),
            CornerRadius::same(4),
            with_alpha(p.card_hover, 210),
        );
    }

    let cy = row_rect.center().y;
    let painter = ui.painter();
    let font_regular = egui::FontId::proportional(11.5);
    let font_pill = egui::FontId::new(9.5, crate::ui::theme::bold_family());

    let mut cursor_x = row_rect.left() + 4.0;

    // 1. 时间列
    if !entry.time_short.is_empty() {
        let tg =
            painter.layout_no_wrap(entry.time_short.clone(), font_regular.clone(), p.text_muted);
        painter.galley(
            Pos2::new(cursor_x, cy - tg.size().y * 0.5),
            tg,
            p.text_muted,
        );
        cursor_x += 56.0;
    }

    // 2. 严格 1 对 1 英文等级胶囊（36px：INFO / WARN / ERR）
    let (st_fg, st_bg, st_bd) = entry.level.colors(p);
    let pill_w = 36.0_f32;
    let pill_rect = Rect::from_center_size(
        Pos2::new(cursor_x + pill_w * 0.5, cy),
        Vec2::new(pill_w, 16.0),
    );
    painter.rect(
        pill_rect,
        CornerRadius::same(4),
        st_bg,
        Stroke::new(1.0_f32, st_bd),
        StrokeKind::Inside,
    );
    let cg = painter.layout_no_wrap(entry.level.label().to_string(), font_pill, st_fg);
    painter.galley(
        Pos2::new(
            pill_rect.center().x - cg.size().x * 0.5,
            pill_rect.center().y - cg.size().y * 0.5,
        ),
        cg,
        st_fg,
    );
    cursor_x += pill_w + 8.0;

    // 3. 自然中文正文（严格限宽截断；超长或有折叠参数时鼠标悬停弹出详情卡）
    let msg_max_w = (row_rect.right() - cursor_x - 6.0).max(40.0);
    let text_color = if entry.level == LogLevel::Err {
        p.danger
    } else {
        p.text
    };
    let (msg_galley, truncated) =
        fit_single_line(painter, &entry.summary, font_regular, text_color, msg_max_w);
    painter.galley(
        Pos2::new(cursor_x, cy - msg_galley.size().y * 0.5),
        msg_galley,
        text_color,
    );

    if resp.clicked() {
        ui.ctx().copy_text(entry.raw.clone());
    }

    if hovered && (truncated || entry.detail.is_some()) {
        show_log_detail_tooltip(ui, p, ui.id().with(("log_row_detail", index)), entry);
    }
}

fn show_log_detail_tooltip(ui: &egui::Ui, p: &Palette, id: egui::Id, entry: &HumanLogLine) {
    let (st_fg, st_bg, st_bd) = entry.level.colors(p);
    egui::show_tooltip(ui.ctx(), ui.layer_id(), id, |ui| {
        ui.set_min_width(260.0);
        ui.set_max_width(356.0);
        ui.spacing_mut().item_spacing.y = 6.0;

        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                badge(ui, st_fg, st_bg, st_bd, entry.level.label());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let stamp = if entry.full_stamp.is_empty() {
                        &entry.time_short
                    } else {
                        &entry.full_stamp
                    };
                    ui.label(body_text(stamp, 10.5, p.text_muted));
                });
            },
        );

        let sep1 = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
        ui.painter().line_segment(
            [sep1.left_center(), sep1.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );

        ui.add(egui::Label::new(bold_text(&entry.summary, 11.8, p.text)).wrap());

        if let Some(detail) = &entry.detail {
            ui.add(egui::Label::new(body_text(detail, 10.8, p.text_secondary)).wrap());
        }

        let sep2 = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
        ui.painter().line_segment(
            [sep2.left_center(), sep2.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );

        ui.label(body_text("单击该行可复制完整日志", 10.0, p.text_muted));
    });
}

fn fit_single_line(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_w: f32,
) -> (Arc<egui::Galley>, bool) {
    let full = painter.layout_no_wrap(text.to_string(), font.clone(), color);
    if full.size().x <= max_w {
        return (full, false);
    }

    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        let g = painter.layout_no_wrap(candidate, font.clone(), color);
        if g.size().x <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let truncated_str: String = chars[..lo]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_string()
        + "…";
    let g = painter.layout_no_wrap(truncated_str, font, color);
    (g, true)
}

/// 将原始日志列表转换为去噪、合并且按真实语义分级的可视化日志流。
fn build_visible_logs(lines: &[String]) -> Vec<HumanLogLine> {
    let mut out: Vec<HumanLogLine> = Vec::with_capacity(lines.len());
    for line in lines {
        let Some(entry) = format_human_log(line) else {
            continue;
        };
        // 相邻行若在同一秒/相邻时刻表达同一结论（例如 LOGIN_VERIFIED 与紧随其后的 VERIFYING → ONLINE），自动合并去重
        if let Some(prev) = out.last() {
            if prev.level == entry.level && prev.summary == entry.summary {
                continue;
            }
        }
        out.push(entry);
    }
    out
}

/// 将单条底层日志映射为用户可读的 `(INFO/WARN/ERR, 中文描述)`；
/// 返回 `None` 表示该条属于内部握手中间态、弹窗发送回执或高频探测流水，主界面不展示。
fn format_human_log(line: &str) -> Option<HumanLogLine> {
    let full_stamp = if line.len() >= 19 && line.as_bytes().get(10) == Some(&b' ') {
        line[..19].to_string()
    } else {
        String::new()
    };
    let (time_short, level, tag, raw_msg) = parse_log_line(line);
    let tag_str = tag.as_deref().unwrap_or("");
    let is_warn = level == "WARNING";
    let is_err = level == "ERROR";

    // ── 第一层：过滤一般场景不需要展示的内部流水与弹窗回执 ──
    if matches!(
        tag_str,
        "NOTIFY_SENT"
            | "NOTIFY_RETRY"
            | "LOGIN_ACCEPTED"
            | "PORTAL_LOGOUT"
            | "FORCE_CHECK"
            | "CAMPUS_STATUS"
            | "NET_POLICY"
            | "CONFIG"
            | "UI"
            | "NETWORK_CHANGE"
            | "NETWORK_SWITCH"
            | "SELF_CHECK"
    ) {
        return None;
    }

    let (lvl, summary, detail) = match tag_str {
        "STATE_CHANGE" => rewrite_state_change(&raw_msg)?,
        "HEALTH_CHECK" => rewrite_health_check(&raw_msg),
        "FORCE_CHECK_RESULT" => {
            if is_warn || is_err || raw_msg.contains("异常") {
                (
                    LogLevel::Warn,
                    format!(
                        "手动检测发现异常：{}",
                        polish_fragment(&raw_msg, "检测异常：")
                    ),
                    None,
                )
            } else if raw_msg.contains("监控已暂停") {
                (
                    LogLevel::Info,
                    "手动检测通过，校园网在线（当前已暂停守护）".to_string(),
                    None,
                )
            } else {
                let ip = extract_kv(&raw_msg, "IP=");
                (
                    LogLevel::Info,
                    "手动检测通过，校园网连接正常".to_string(),
                    ip.map(|v| format!("本机 IP：{v}")),
                )
            }
        }
        "LOGIN_VERIFIED" => (
            LogLevel::Info,
            "自动重连成功，校园网已恢复上网".to_string(),
            None,
        ),
        "NETWORK_RECOVERED" => {
            let ip = extract_kv(&raw_msg, "IP=");
            (
                LogLevel::Info,
                "网络连接已恢复正常".to_string(),
                ip.map(|v| format!("本机 IP：{v}")),
            )
        }
        "AUTH_RETRY_OK" => (
            LogLevel::Info,
            "重试认证成功，校园网已恢复上网".to_string(),
            Some(raw_msg.clone()),
        ),
        "AUTH_RETRY" => (
            LogLevel::Warn,
            "认证被短暂拒绝，稍后将自动重试".to_string(),
            Some(polish_generic(&raw_msg)),
        ),
        "RETRY_BACKOFF" => {
            let secs = raw_msg
                .split_whitespace()
                .next()
                .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or("稍");
            let msg = if secs == "稍" {
                "将在稍后自动重试连接".to_string()
            } else {
                format!("将在 {secs} 秒后自动重试连接")
            };
            (LogLevel::Warn, msg, None)
        }
        "LOGIN_NOT_VERIFIED" => (
            LogLevel::Err,
            "认证提交后公网仍未恢复，将继续排查".to_string(),
            Some(polish_generic(&raw_msg)),
        ),
        "CREDENTIALS_MISSING" => (
            LogLevel::Err,
            "尚未保存当前账号密码，无法自动登录".to_string(),
            None,
        ),
        "LOGIN_REJECTED" | "PORTAL_REJECTED" | "PORTAL_ERROR" | "PORTAL_UNAVAILABLE" => (
            LogLevel::Err,
            format!("校园网认证失败：{}", strip_portal_code(&raw_msg)),
            None,
        ),
        "ACCOUNT_SWITCH" => {
            let acct = extract_quoted_name(&raw_msg);
            let msg = match acct {
                Some(name) => format!("正在切换账号至「{name}」…"),
                None => "正在切换校园网账号…".to_string(),
            };
            (LogLevel::Info, msg, None)
        }
        "ACCOUNT_SWITCHED" => {
            let acct = extract_quoted_name(&raw_msg);
            let msg = match acct {
                Some(name) => format!("已切换至「{name}」并成功联网"),
                None => "账号切换成功，校园网已连接".to_string(),
            };
            (LogLevel::Info, msg, None)
        }
        "SWITCH_ABORTED" | "SWITCH_NOT_VERIFIED" | "SWITCH_FAILED" => {
            (LogLevel::Err, strip_portal_code(&raw_msg), None)
        }
        "MONITOR_STARTED" => (LogLevel::Info, "已开启后台自动守护".to_string(), None),
        "MONITOR_PAUSED" => (LogLevel::Info, "已暂停后台自动守护".to_string(), None),
        "SETTINGS_SAVED" => (LogLevel::Info, "新设置已保存并生效".to_string(), None),
        "APP_STARTED" => {
            let detail = raw_msg
                .split("日志路径：")
                .nth(1)
                .map(|p| format!("日志路径：{}", p.trim()));
            (
                LogLevel::Info,
                format!("守护助手 v{} 已启动", env!("CARGO_PKG_VERSION")),
                detail,
            )
        }
        "APP_READY" => (
            LogLevel::Info,
            "后台守护已就绪，正在系统托盘静默运行".to_string(),
            None,
        ),
        "APP_EXIT" => (LogLevel::Info, "守护助手已退出".to_string(), None),
        "DIAGNOSTIC" => return None,
        "SELF_SERVICE" => {
            let lvl = if is_err {
                LogLevel::Err
            } else if is_warn {
                LogLevel::Warn
            } else {
                LogLevel::Info
            };
            (lvl, raw_msg.trim().to_string(), None)
        }
        "DIAGNOSTIC_ERROR" => (
            LogLevel::Err,
            "网络诊断过程发生异常".to_string(),
            Some(polish_generic(&raw_msg)),
        ),
        _ if is_err || tag_str.ends_with("_ERROR") || tag_str.ends_with("_FAILED") => {
            (LogLevel::Err, polish_generic(&raw_msg), None)
        }
        _ if is_warn => (LogLevel::Warn, polish_generic(&raw_msg), None),
        _ => (LogLevel::Info, polish_generic(&raw_msg), None),
    };

    Some(HumanLogLine {
        full_stamp,
        time_short,
        level: lvl,
        summary,
        detail,
        raw: line.to_string(),
    })
}

/// 过滤掉内部状态机握手中间态（`→ AUTHENTICATING` / `→ VERIFYING` / `→ CHECKING`），
/// 仅保留对用户有实质意义的「断联/恢复/启停」结果。
fn rewrite_state_change(raw_msg: &str) -> Option<(LogLevel, String, Option<String>)> {
    // 内部握手中间态直接静默
    if raw_msg.contains("→ AUTHENTICATING")
        || raw_msg.contains("-> AUTHENTICATING")
        || raw_msg.contains("→ VERIFYING")
        || raw_msg.contains("-> VERIFYING")
        || raw_msg.contains("→ CHECKING")
        || raw_msg.contains("-> CHECKING")
    {
        return None;
    }

    let reason_code = extract_kv(raw_msg, "原因=");
    let reason_zh = reason_code
        .as_deref()
        .filter(|c| !c.is_empty() && *c != "ONLINE")
        .map(reason_label);

    if raw_msg.contains("→ ONLINE") || raw_msg.contains("-> ONLINE") {
        if raw_msg.contains("已切换至账号") {
            // ACCOUNT_SWITCHED 已专门展示，避免重复打印
            return None;
        }
        let summary = if raw_msg.starts_with("PAUSED") {
            "已开启后台自动守护，当前校园网在线".to_string()
        } else if raw_msg.starts_with("VERIFYING") {
            "自动重连成功，校园网已恢复上网".to_string()
        } else if raw_msg.starts_with("OFFLINE") || raw_msg.starts_with("SUSPECT") {
            "校园网已恢复正常连接".to_string()
        } else {
            "检测通过，校园网连接正常".to_string()
        };
        return Some((LogLevel::Info, summary, None));
    }

    if raw_msg.contains("→ OFFLINE") || raw_msg.contains("-> OFFLINE") {
        let code = reason_code.as_deref().unwrap_or("");
        let is_err = matches!(
            code,
            "CREDENTIALS_MISSING"
                | "LOGIN_REJECTED"
                | "LINK_DOWN"
                | "NO_IP"
                | "NO_GATEWAY"
                | "GATEWAY_UNREACHABLE"
        );
        let level = if is_err {
            LogLevel::Err
        } else {
            LogLevel::Warn
        };
        let summary = if code == "AUTH_EXPIRED" {
            "校园网认证已过期，正在自动重连…".to_string()
        } else if code == "CREDENTIALS_MISSING" {
            "尚未保存账号密码，请前往设置补全".to_string()
        } else if let Some(r) = reason_zh {
            if raw_msg.contains("准备自动登录") {
                format!("{r}，正在自动重连…")
            } else {
                format!("校园网已断开（{r}）")
            }
        } else {
            "检测到校园网连接中断".to_string()
        };
        return Some((level, summary, None));
    }

    if raw_msg.contains("→ SUSPECT") || raw_msg.contains("-> SUSPECT") {
        let summary = match reason_zh {
            Some(r) => format!("网络出现短暂波动（{r}），正在二次复核"),
            None => "网络出现短暂波动，正在二次复核".to_string(),
        };
        return Some((LogLevel::Warn, summary, None));
    }

    if raw_msg.contains("→ PAUSED") || raw_msg.contains("-> PAUSED") {
        return Some((LogLevel::Info, "已暂停后台自动守护".to_string(), None));
    }

    None
}

fn rewrite_health_check(raw_msg: &str) -> (LogLevel, String, Option<String>) {
    let cost = extract_kv(raw_msg, "检测耗时=");
    let ip = extract_kv(raw_msg, "IP=");
    let gw = extract_kv(raw_msg, "网关=");
    let hb = extract_kv(raw_msg, "心跳间隔=");

    let summary = match cost.as_deref() {
        Some(c) => format!("例行巡检正常（耗时 {c}）"),
        None => "例行巡检正常，校园网保持在线".to_string(),
    };

    let mut extras = Vec::new();
    if let Some(v) = ip.filter(|s| s != "unknown") {
        extras.push(format!("本机 IP：{v}"));
    }
    if let Some(v) = gw.filter(|s| s != "unknown") {
        extras.push(format!("网关：{v}"));
    }
    if let Some(v) = hb {
        extras.push(format!("心跳周期：{v}"));
    }
    let detail = if extras.is_empty() {
        None
    } else {
        Some(extras.join(" · "))
    };
    (LogLevel::Info, summary, detail)
}

fn strip_portal_code(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    for suffix in ["；门户返回码=", "; 门户返回码="] {
        if let Some(pos) = s.find(suffix) {
            let head = s[..pos].trim();
            let after = s[pos + suffix.len()..].trim();
            if after.is_empty() || after == "-" || !head.is_empty() {
                s.truncate(pos);
                break;
            }
        }
    }
    polish_generic(&s)
}

fn polish_fragment(raw: &str, prefix: &str) -> String {
    polish_generic(raw.trim().strip_prefix(prefix).unwrap_or(raw))
}

fn polish_generic(raw: &str) -> String {
    let mut text = raw.trim().to_string();
    // 兼容历史日志中遗留的英文错误码与旧版自助服务重定向提示
    if text.contains("验证登录结果未跳转到预期页面（HTTP 302）") {
        text = text.replace(
            "验证登录结果未跳转到预期页面（HTTP 302）",
            "自助服务登录失败：学号或密码错误，请在设置中核对密码",
        );
    }
    for code in [
        "CREDENTIALS_MISSING",
        "GATEWAY_UNREACHABLE",
        "PORTAL_UNAVAILABLE",
        "UPLINK_UNREACHABLE",
        "INTERNET_TIMEOUT",
        "LOGIN_REJECTED",
        "AUTH_EXPIRED",
        "DNS_FAILURE",
        "NO_GATEWAY",
        "LINK_DOWN",
        "NO_IP",
    ] {
        let label = reason_label(code);
        text = text.replace(&format!("原因={code}"), &format!("原因：{label}"));
        text = text.replace(code, label);
    }
    text = text.replace("；原因=ONLINE", "");
    text = text.replace("; 原因=ONLINE", "");
    text = text.replace("门户返回码=", "返回码 ");
    text = text.replace("IP=", "IP ");
    text = text.replace("网关=", "网关 ");
    let parts: Vec<String> = text
        .split([';', '；'])
        .map(|seg| crate::authenticator::translate_drcom_message(seg.trim()))
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        crate::authenticator::translate_drcom_message(raw.trim())
    } else {
        parts.join(" · ")
    }
}

fn extract_kv(text: &str, key: &str) -> Option<String> {
    let start = text.find(key)? + key.len();
    let rest = &text[start..];
    let end = rest
        .find([';', '；', '，', ',', '）', ')'])
        .unwrap_or(rest.len());
    let val = rest[..end].trim();
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

fn extract_quoted_name(text: &str) -> Option<String> {
    let start = text.find('「')? + '「'.len_utf8();
    let rest = &text[start..];
    let end = rest.find('」')?;
    let name = rest[..end].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compresses_nine_line_reconnect_sequence_into_two_clean_entries() {
        let raw_burst = vec![
            "2026-11-22 03:43:09 INFO [STATE_CHANGE] ONLINE → OFFLINE 准备自动登录校园网; 原因=AUTH_EXPIRED".to_string(),
            "2026-11-22 03:43:09 INFO [STATE_CHANGE] OFFLINE → AUTHENTICATING 正在进行校园网认证; 原因=AUTH_EXPIRED".to_string(),
            "2026-11-22 03:43:09 INFO [NOTIFY_SENT] 系统弹窗：校园网认证过期。准备自动登录校园网".to_string(),
            "2026-11-22 03:43:10 INFO [LOGIN_ACCEPTED] 认证服务器未返回说明; 门户返回码=".to_string(),
            "2026-11-22 03:43:10 INFO [STATE_CHANGE] AUTHENTICATING → VERIFYING 认证响应成功, 正在验证网络; 原因=AUTH_EXPIRED".to_string(),
            "2026-11-22 03:43:11 INFO [LOGIN_VERIFIED] 校园网登录成功，公网已恢复".to_string(),
            "2026-11-22 03:43:11 INFO [STATE_CHANGE] VERIFYING → ONLINE; 校园网登录成功，公网已恢复; 原因=ONLINE".to_string(),
            "2026-11-22 03:43:11 INFO [NOTIFY_SENT] 网络恢复提醒；最后原因=校园网认证过期".to_string(),
            "2026-11-22 03:43:12 INFO [NOTIFY_SENT] 系统弹窗：网络已恢复（断网 2 秒）".to_string(),
        ];
        let visible = build_visible_logs(&raw_burst);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].level, LogLevel::Warn);
        assert_eq!(visible[0].level.label(), "WARN");
        assert_eq!(visible[0].summary, "校园网认证已过期，正在自动重连…");
        assert_eq!(visible[1].level, LogLevel::Info);
        assert_eq!(visible[1].level.label(), "INFO");
        assert_eq!(visible[1].summary, "自动重连成功，校园网已恢复上网");
    }

    #[test]
    fn maps_errors_and_health_checks_to_exact_three_levels() {
        let rejected = format_human_log(
            "2026-11-22 03:18:48 WARNING [LOGIN_REJECTED] 账号密码校验未通过；门户返回码=1",
        )
        .unwrap();
        assert_eq!(rejected.level, LogLevel::Err);
        assert_eq!(rejected.level.label(), "ERR");
        assert!(rejected.summary.contains("校园网认证失败"));

        let health = format_human_log(
            "2026-11-22 03:17:39 INFO [HEALTH_CHECK] 网络正常；IP=198.18.0.1；网关=198.18.0.2；检测耗时=0.03s；心跳间隔=30分钟",
        )
        .unwrap();
        assert_eq!(health.level, LogLevel::Info);
        assert_eq!(health.summary, "例行巡检正常（耗时 0.03s）");
        assert!(health.detail.as_deref().unwrap().contains("198.18.0.1"));
    }
}
