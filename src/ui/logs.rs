//! 嵌入式运行日志卡（位于主页下半屏）：
//! - 极简直观工具栏：`全部 / 断联与登录 / 仅异常` + 单个紧凑搜索框 + 打开原文件（自动智能滚底，无多余开关）
//! - 严格 1 对 1 色彩语义标签：绿「正常」、蓝「动态」、黄「波动」、红「异常」，绝不混色
//! - 自然大白话中文重写（保持磁盘 `app.log` 原格式 100% 兼容不变）；超长或含补充详情时悬停展示小卡片

use std::path::Path;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::{
    body_text, bold_text, parse_log_line, reason_label, with_alpha, Palette, RADIUS_SM,
};
use crate::ui::widgets::{badge, card_padded, icon_button, section_heading, segmented_control};

#[derive(Default)]
pub struct LogsState {
    /// 0 = 全部，1 = 断联与登录（断网、自动重登、切号、恢复），2 = 仅异常
    pub view_filter: usize,
    pub search: String,
}

/// 严格 1 对 1 的日志状态色阶（每种颜色只对应唯一一个中文标签）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogStatus {
    /// 绿：巡检通过、在线、重连成功、切号成功
    Normal,
    /// 蓝：启停守护、开始登录、切号过程、网络变动复查、设置更新、常规通知
    Activity,
    /// 黄：掉线/认证过期、网络波动复核、等待重试、轻度警告
    Notice,
    /// 红：登录被拒、缺密码、切号失败、程序或网络故障
    Error,
}

impl LogStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Normal => "正常",
            Self::Activity => "动态",
            Self::Notice => "波动",
            Self::Error => "异常",
        }
    }

    fn colors(self, p: &Palette) -> (Color32, Color32, Color32) {
        match self {
            Self::Normal => (p.ok, p.ok_soft, p.ok_border),
            Self::Activity => (p.accent, p.accent_soft, p.accent_border),
            Self::Notice => (p.warn, p.warn_soft, p.warn_border),
            Self::Error => (p.danger, p.danger_soft, p.danger_border),
        }
    }
}

#[derive(Debug, Clone)]
struct HumanLogLine {
    full_stamp: String,
    time_short: String,
    status: LogStatus,
    summary: String,
    /// 若正文精简掉了 IP/网关/路径等次要技术参数，放入此处供鼠标悬停详情卡查看。
    detail: Option<String>,
    /// 是否属于「断联与登录」关注范畴（掉线、重登、切号、恢复、认证异常）。
    is_conn_or_auth: bool,
    /// 是否属于「仅异常」范畴（波动与异常）。
    is_anomaly: bool,
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
    let query = state.search.trim().to_lowercase();
    let filtered: Vec<HumanLogLine> = lines
        .iter()
        .map(|line| format_human_log(line))
        .filter(|entry| {
            let tab_ok = match state.view_filter {
                1 => entry.is_conn_or_auth,
                2 => entry.is_anomaly,
                _ => true,
            };
            if !tab_ok {
                return false;
            }
            if query.is_empty() {
                return true;
            }
            entry.summary.to_lowercase().contains(&query)
                || entry.status.label().contains(&query)
                || entry
                    .detail
                    .as_deref()
                    .is_some_and(|d| d.to_lowercase().contains(&query))
                || entry.raw.to_lowercase().contains(&query)
        })
        .collect();

    // 精确扣除所有内外边距与 1px 描边（外卡 12×2+2=26，工具栏 26，间距 7，内框 6×2+2=14），确保底部圆角与外边距完全对称
    let inner_h = (target_height - 26.0).max(96.0);
    let scroll_h = (inner_h - 26.0 - 7.0 - 14.0).max(48.0);

    card_padded(ui, p, 12, |ui| {
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);
        ui.spacing_mut().item_spacing.y = 0.0;

        // ── 26px 极简工具栏：清晰中文分类 + 单个紧凑搜索框 + 打开原文件 ──
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

                let count_label = if filtered.len() == lines.len() {
                    format!("{} 条", lines.len())
                } else {
                    format!("{}/{}", filtered.len(), lines.len())
                };
                badge(ui, p.text_secondary, p.subtle, p.border, &count_label);

                let tabs = ["全部", "断联与登录", "仅异常"];
                segmented_control(ui, p, "inline_log_view", &mut state.view_filter, &tabs);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;

                    if icon_button(
                        ui,
                        p,
                        Icon::ExternalLink,
                        Vec2::splat(26.0),
                        13.0,
                        false,
                        "打开原始日志文件 (app.log)",
                    )
                    .clicked()
                    {
                        open_path(log_path);
                    }

                    // 单个紧凑搜索框（常驻不占地方）
                    let box_w = (ui.available_width() - 6.0).clamp(86.0, 128.0);
                    if box_w >= 86.0 {
                        draw_compact_search_box(ui, p, &mut state.search, box_w);
                    }
                });
            },
        );

        ui.add_space(7.0);

        // ── 日志流圆角容器（严格锁定高度，确保内外圆角完整闭合） ─────────
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
                        2 => "当前没有波动或异常记录",
                        1 => "当前没有断联或登录记录",
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
                            paint_log_row(ui, p, &filtered[index], index, row_height, row_w);
                        }
                    });
            });
    });
}

fn draw_compact_search_box(ui: &mut egui::Ui, p: &Palette, search: &mut String, width: f32) {
    let active = !search.is_empty();
    egui::Frame::new()
        .fill(p.input_bg)
        .stroke(Stroke::new(
            1.0_f32,
            if active { p.accent_border } else { p.border },
        ))
        .corner_radius(CornerRadius::same(RADIUS_SM))
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new((width - 14.0).max(66.0), 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let (s_rect, _) = ui.allocate_exact_size(Vec2::splat(11.5), Sense::hover());
                    icons::paint(
                        ui.painter(),
                        s_rect,
                        Icon::Search,
                        if active { p.accent } else { p.text_muted },
                    );
                    let edit_w = (ui.available_width() - if active { 16.0 } else { 2.0 }).max(40.0);
                    ui.add_sized(
                        [edit_w, 18.0],
                        egui::TextEdit::singleline(search)
                            .hint_text(egui::RichText::new("搜索…").color(p.text_muted))
                            .vertical_align(egui::Align::Center)
                            .frame(false),
                    );
                    if active {
                        let (clr, resp) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::click());
                        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                        icons::paint(
                            ui.painter(),
                            clr.shrink(1.5),
                            Icon::Close,
                            if resp.hovered() { p.text } else { p.text_muted },
                        );
                        if resp.clicked() {
                            search.clear();
                        }
                    }
                },
            );
        });
}

/// 绘制单条自然中文日志行：时间 + 1 对 1 颜色状态胶囊 + 限宽正文（超长或有补充详情时悬停展示小卡片）。
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
    let font_pill = egui::FontId::new(10.0, crate::ui::theme::bold_family());

    let mut cursor_x = row_rect.left() + 4.0;

    // 1. 时间列（固定对齐）
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

    // 2. 严格 1 对 1 语义状态胶囊（38px：正常/动态/波动/异常）
    let (st_fg, st_bg, st_bd) = entry.status.colors(p);
    let pill_w = 38.0_f32;
    let pill_rect = Rect::from_center_size(
        Pos2::new(cursor_x + pill_w * 0.5, cy),
        Vec2::new(pill_w, 17.0),
    );
    painter.rect(
        pill_rect,
        CornerRadius::same(4),
        st_bg,
        Stroke::new(1.0_f32, st_bd),
        StrokeKind::Inside,
    );
    let cg = painter.layout_no_wrap(entry.status.label().to_string(), font_pill, st_fg);
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
    let text_color = if entry.status == LogStatus::Error {
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
    let (st_fg, st_bg, st_bd) = entry.status.colors(p);
    egui::show_tooltip(ui.ctx(), ui.layer_id(), id, |ui| {
        ui.set_min_width(260.0);
        ui.set_max_width(356.0);
        ui.spacing_mut().item_spacing.y = 6.0;

        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                badge(ui, st_fg, st_bg, st_bd, entry.status.label());
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

/// 将底层结构化日志重写为自然中文与 1 对 1 色彩状态（不改变磁盘 `app.log` 原文）。
fn format_human_log(line: &str) -> HumanLogLine {
    let full_stamp = if line.len() >= 19 && line.as_bytes().get(10) == Some(&b' ') {
        line[..19].to_string()
    } else {
        String::new()
    };
    let (time_short, level, tag, raw_msg) = parse_log_line(line);
    let tag_str = tag.as_deref().unwrap_or("");
    let is_warn = level == "WARNING";
    let is_err = level == "ERROR";

    let (status, summary, detail, is_conn_or_auth) = match tag_str {
        "STATE_CHANGE" => rewrite_state_change(&raw_msg),
        "HEALTH_CHECK" => rewrite_health_check(&raw_msg),
        "FORCE_CHECK" => (
            LogStatus::Activity,
            "手动发起了一次网络检测".to_string(),
            None,
            false,
        ),
        "FORCE_CHECK_RESULT" => {
            if is_warn || is_err || raw_msg.contains("异常") {
                (
                    LogStatus::Notice,
                    format!(
                        "手动检测发现异常：{}",
                        polish_fragment(&raw_msg, "检测异常：")
                    ),
                    None,
                    true,
                )
            } else if raw_msg.contains("监控已暂停") {
                (
                    LogStatus::Normal,
                    "手动检测通过，校园网在线（当前已暂停守护）".to_string(),
                    None,
                    false,
                )
            } else {
                let ip = extract_kv(&raw_msg, "IP=");
                (
                    LogStatus::Normal,
                    "手动检测通过，校园网连接正常".to_string(),
                    ip.map(|v| format!("本机 IP：{v}")),
                    false,
                )
            }
        }
        "LOGIN_ACCEPTED" => {
            let clean = strip_portal_code(&raw_msg);
            if clean.is_empty() || clean == "认证服务器未返回说明" {
                (
                    LogStatus::Activity,
                    "校园网认证已受理，等待网络放行".to_string(),
                    None,
                    true,
                )
            } else {
                (
                    LogStatus::Activity,
                    format!("认证请求已受理：{clean}"),
                    None,
                    true,
                )
            }
        }
        "LOGIN_VERIFIED" => (
            LogStatus::Normal,
            "校园网登录成功，已恢复上网".to_string(),
            None,
            true,
        ),
        "NETWORK_RECOVERED" => {
            let ip = extract_kv(&raw_msg, "IP=");
            (
                LogStatus::Normal,
                "网络连接已自行恢复正常".to_string(),
                ip.map(|v| format!("本机 IP：{v}")),
                true,
            )
        }
        "AUTH_RETRY_OK" => (
            LogStatus::Normal,
            "重试认证成功，校园网已放行".to_string(),
            Some(raw_msg.clone()),
            true,
        ),
        "AUTH_RETRY" => (
            LogStatus::Notice,
            "认证被短暂拒绝，稍后将自动重试".to_string(),
            Some(polish_generic(&raw_msg)),
            true,
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
            (LogStatus::Notice, msg, None, true)
        }
        "LOGIN_NOT_VERIFIED" => (
            LogStatus::Error,
            "认证提交后公网仍未恢复，将继续排查".to_string(),
            Some(polish_generic(&raw_msg)),
            true,
        ),
        "CREDENTIALS_MISSING" => (
            LogStatus::Error,
            "尚未保存当前账号密码，无法自动登录".to_string(),
            None,
            true,
        ),
        "LOGIN_REJECTED" | "PORTAL_REJECTED" | "PORTAL_ERROR" | "PORTAL_UNAVAILABLE" => (
            LogStatus::Error,
            format!("校园网认证失败：{}", strip_portal_code(&raw_msg)),
            None,
            true,
        ),
        "ACCOUNT_SWITCH" => {
            let acct = extract_quoted_name(&raw_msg);
            let msg = match acct {
                Some(name) => format!("开始切换账号至「{name}」"),
                None => "开始切换校园网账号".to_string(),
            };
            (LogStatus::Activity, msg, None, true)
        }
        "PORTAL_LOGOUT" => {
            let msg = if raw_msg.contains("success=true") {
                "已注销原账号会话，准备登录新账号".to_string()
            } else {
                "已尝试注销原会话，继续登录新账号".to_string()
            };
            (LogStatus::Activity, msg, None, true)
        }
        "ACCOUNT_SWITCHED" => {
            let acct = extract_quoted_name(&raw_msg);
            let msg = match acct {
                Some(name) => format!("已切换至「{name}」并成功联网"),
                None => "账号切换成功，校园网已连接".to_string(),
            };
            (LogStatus::Normal, msg, None, true)
        }
        "SWITCH_ABORTED" | "SWITCH_NOT_VERIFIED" | "SWITCH_FAILED" => {
            (LogStatus::Error, strip_portal_code(&raw_msg), None, true)
        }
        "MONITOR_STARTED" => (
            LogStatus::Activity,
            "已开启后台自动守护".to_string(),
            None,
            false,
        ),
        "MONITOR_PAUSED" => (
            LogStatus::Activity,
            "已暂停后台自动守护".to_string(),
            None,
            false,
        ),
        "SETTINGS_SAVED" => (
            LogStatus::Activity,
            "新设置已保存并生效".to_string(),
            None,
            false,
        ),
        "NETWORK_CHANGE" => (
            LogStatus::Activity,
            "检测到网络变动或电脑唤醒，正在检查连接".to_string(),
            None,
            false,
        ),
        "NETWORK_SWITCH" => (
            LogStatus::Activity,
            "网络接口正在切换，等待稳定后重新检测".to_string(),
            Some(polish_generic(&raw_msg)),
            false,
        ),
        "NOTIFY_SENT" => rewrite_notify(&raw_msg),
        "NOTIFY_RETRY" => (
            LogStatus::Notice,
            "系统弹窗提醒发送失败".to_string(),
            Some(raw_msg.clone()),
            false,
        ),
        "CAMPUS_STATUS" => {
            if raw_msg.contains("在线") && !raw_msg.contains("不在线") {
                (
                    LogStatus::Normal,
                    "校园网门户确认当前处于在线状态".to_string(),
                    extract_paren(&raw_msg).map(polish_generic),
                    false,
                )
            } else if raw_msg.contains("不在线") {
                (
                    LogStatus::Notice,
                    "校园网门户提示当前账号未登录".to_string(),
                    extract_paren(&raw_msg).map(polish_generic),
                    true,
                )
            } else {
                (
                    LogStatus::Notice,
                    "暂时无法查询校园网门户状态".to_string(),
                    Some(polish_generic(&raw_msg)),
                    false,
                )
            }
        }
        "APP_STARTED" => {
            let detail = raw_msg
                .split("日志路径：")
                .nth(1)
                .map(|p| format!("日志路径：{}", p.trim()));
            (
                LogStatus::Activity,
                format!("守护助手 v{} 已启动", env!("CARGO_PKG_VERSION")),
                detail,
                false,
            )
        }
        "APP_READY" => (
            LogStatus::Normal,
            "后台守护已就绪，正在系统托盘静默运行".to_string(),
            None,
            false,
        ),
        "APP_EXIT" => (
            LogStatus::Activity,
            "守护助手已退出".to_string(),
            None,
            false,
        ),
        "NET_POLICY" => (
            LogStatus::Activity,
            "已启用直连探测策略（自动绕过系统代理）".to_string(),
            None,
            false,
        ),
        "CONFIG" => (
            LogStatus::Activity,
            "已加载本地守护配置".to_string(),
            Some(polish_generic(&raw_msg)),
            false,
        ),
        "DIAGNOSTIC" => (LogStatus::Notice, polish_generic(&raw_msg), None, true),
        "DIAGNOSTIC_ERROR" => (
            LogStatus::Error,
            "网络诊断过程发生异常".to_string(),
            Some(polish_generic(&raw_msg)),
            true,
        ),
        _ if is_err || tag_str.ends_with("_ERROR") || tag_str.ends_with("_FAILED") => {
            (LogStatus::Error, polish_generic(&raw_msg), None, true)
        }
        _ if is_warn => (LogStatus::Notice, polish_generic(&raw_msg), None, true),
        _ => (LogStatus::Activity, polish_generic(&raw_msg), None, false),
    };

    let is_anomaly = matches!(status, LogStatus::Notice | LogStatus::Error);
    HumanLogLine {
        full_stamp,
        time_short,
        status,
        summary,
        detail,
        is_conn_or_auth,
        is_anomaly,
        raw: line.to_string(),
    }
}

/// 将 `[STATE_CHANGE] A → B ...; 原因=CODE` 改写为地道中文句式，彻底去除状态机箭头术语。
fn rewrite_state_change(raw_msg: &str) -> (LogStatus, String, Option<String>, bool) {
    let reason_code = extract_kv(raw_msg, "原因=");
    let reason_zh = reason_code
        .as_deref()
        .filter(|c| !c.is_empty() && *c != "ONLINE")
        .map(reason_label);

    if raw_msg.contains("→ ONLINE") || raw_msg.contains("-> ONLINE") {
        let summary = if raw_msg.starts_with("PAUSED") {
            "已恢复守护，当前校园网在线".to_string()
        } else if raw_msg.starts_with("VERIFYING") {
            "校园网重连成功，已恢复上网".to_string()
        } else if raw_msg.starts_with("OFFLINE") || raw_msg.starts_with("SUSPECT") {
            "校园网已重新恢复在线".to_string()
        } else {
            "检测通过，校园网连接正常".to_string()
        };
        let conn = raw_msg.starts_with("VERIFYING")
            || raw_msg.starts_with("OFFLINE")
            || raw_msg.starts_with("SUSPECT");
        return (LogStatus::Normal, summary, None, conn);
    }

    if raw_msg.contains("→ AUTHENTICATING") || raw_msg.contains("-> AUTHENTICATING") {
        let summary = if let Some(label) = extract_quoted_name(raw_msg) {
            format!("正在登录账号「{label}」…")
        } else {
            "正在自动登录校园网…".to_string()
        };
        return (LogStatus::Activity, summary, None, true);
    }

    if raw_msg.contains("→ VERIFYING") || raw_msg.contains("-> VERIFYING") {
        let summary = if let Some(label) = extract_quoted_name(raw_msg) {
            format!("账号「{label}」认证已通过，正在确认公网连接")
        } else {
            "认证已通过，正在确认公网连通性".to_string()
        };
        return (LogStatus::Activity, summary, None, true);
    }

    if raw_msg.contains("→ OFFLINE") || raw_msg.contains("-> OFFLINE") {
        let is_auth_expired = reason_code.as_deref() == Some("AUTH_EXPIRED");
        let is_missing = reason_code.as_deref() == Some("CREDENTIALS_MISSING");
        let is_rejected = reason_code.as_deref() == Some("LOGIN_REJECTED");
        let status = if is_missing || is_rejected {
            LogStatus::Error
        } else {
            LogStatus::Notice
        };
        let summary = if is_auth_expired && raw_msg.contains("准备自动登录") {
            "校园网认证已过期，准备自动重连".to_string()
        } else if is_missing {
            "尚未保存账号密码，请前往设置补全".to_string()
        } else if let Some(r) = reason_zh {
            format!("校园网已断开（{r}）")
        } else {
            "检测到校园网已断开".to_string()
        };
        return (status, summary, None, true);
    }

    if raw_msg.contains("→ SUSPECT") || raw_msg.contains("-> SUSPECT") {
        let summary = match reason_zh {
            Some(r) => format!("网络出现波动（{r}），正在二次复核"),
            None => "网络出现短暂波动，正在二次复核".to_string(),
        };
        return (LogStatus::Notice, summary, None, true);
    }

    if raw_msg.contains("→ PAUSED") || raw_msg.contains("-> PAUSED") {
        return (
            LogStatus::Activity,
            "后台守护已暂停".to_string(),
            None,
            false,
        );
    }

    if raw_msg.contains("→ CHECKING") || raw_msg.contains("-> CHECKING") {
        return (
            LogStatus::Activity,
            "正在检测校园网连接状态…".to_string(),
            None,
            false,
        );
    }

    (LogStatus::Activity, polish_generic(raw_msg), None, false)
}

fn rewrite_health_check(raw_msg: &str) -> (LogStatus, String, Option<String>, bool) {
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
    (LogStatus::Normal, summary, detail, false)
}

fn rewrite_notify(raw_msg: &str) -> (LogStatus, String, Option<String>, bool) {
    if let Some(body) = raw_msg
        .strip_prefix("系统弹窗：")
        .or_else(|| raw_msg.strip_prefix("系统弹窗:"))
    {
        let body = body.trim();
        let is_outage = body.contains("过期") || body.contains("断开") || body.contains("异常");
        return (
            if is_outage {
                LogStatus::Notice
            } else {
                LogStatus::Activity
            },
            format!("已发送系统提醒：{body}"),
            None,
            is_outage || body.contains("恢复"),
        );
    }
    if raw_msg.starts_with("网络恢复提醒") {
        let cause = extract_kv(raw_msg, "最后原因=");
        let summary = match cause {
            Some(c) => format!("已发送恢复提醒（原原因：{c}）"),
            None => "已发送网络恢复提醒".to_string(),
        };
        return (LogStatus::Normal, summary, None, true);
    }
    (LogStatus::Activity, polish_generic(raw_msg), None, false)
}

fn strip_portal_code(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    for suffix in [
        "；门户返回码=-",
        "; 门户返回码=-",
        "；门户返回码=",
        "; 门户返回码=",
    ] {
        if let Some(pos) = s.find(suffix) {
            let after = s[pos + suffix.len()..].trim();
            if after.is_empty() || after == "-" {
                s.truncate(pos);
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
    let parts: Vec<&str> = text
        .split([';', '；'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        raw.trim().to_string()
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

fn extract_paren(text: &str) -> Option<&str> {
    let start = text.find('（')? + '（'.len_utf8();
    let rest = &text[start..];
    let end = rest.find('）')?;
    let inner = rest[..end].trim();
    if inner.is_empty() {
        None
    } else {
        Some(inner)
    }
}

fn open_path(path: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", &path.to_string_lossy()])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_logs_into_natural_chinese_with_strict_four_status_colors() {
        let offline = format_human_log(
            "2026-11-22 02:47:41 INFO [STATE_CHANGE] ONLINE → OFFLINE 准备自动登录校园网; 原因=AUTH_EXPIRED",
        );
        assert_eq!(offline.status, LogStatus::Notice);
        assert_eq!(offline.status.label(), "波动");
        assert!(offline.is_conn_or_auth);
        assert!(offline.is_anomaly);
        assert_eq!(offline.summary, "校园网认证已过期，准备自动重连");

        let resumed = format_human_log(
            "2026-11-22 03:17:39 INFO [STATE_CHANGE] PAUSED → ONLINE; 公网连接正常; 原因=ONLINE",
        );
        assert_eq!(resumed.status, LogStatus::Normal);
        assert_eq!(resumed.summary, "已恢复守护，当前校园网在线");

        let recovered = format_human_log(
            "2026-11-22 02:47:42 INFO [STATE_CHANGE] VERIFYING → ONLINE; 校园网登录成功，公网已恢复; 原因=ONLINE",
        );
        assert_eq!(recovered.status, LogStatus::Normal);
        assert!(recovered.is_conn_or_auth);
        assert!(!recovered.is_anomaly);
        assert_eq!(recovered.summary, "校园网重连成功，已恢复上网");

        let accepted = format_human_log(
            "2026-11-22 02:47:41 INFO [LOGIN_ACCEPTED] 认证服务器未返回说明; 门户返回码=",
        );
        assert_eq!(accepted.status, LogStatus::Activity);
        assert_eq!(accepted.summary, "校园网认证已受理，等待网络放行");

        let health = format_human_log(
            "2026-11-22 03:17:39 INFO [HEALTH_CHECK] 网络正常；IP=198.18.0.1；网关=198.18.0.2；检测耗时=0.03s；心跳间隔=30分钟",
        );
        assert_eq!(health.status, LogStatus::Normal);
        assert!(!health.is_conn_or_auth);
        assert_eq!(health.summary, "例行巡检正常（耗时 0.03s）");
        assert!(health.detail.as_deref().unwrap().contains("198.18.0.1"));

        let net_chg = format_human_log(
            "2026-11-22 03:18:26 INFO [NETWORK_CHANGE] 接口变化或系统唤醒，开始只读复查",
        );
        assert_eq!(net_chg.status, LogStatus::Activity);
        assert!(!net_chg.is_conn_or_auth);
        assert_eq!(net_chg.summary, "检测到网络变动或电脑唤醒，正在检查连接");
    }
}
