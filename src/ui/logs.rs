//! 嵌入式运行日志卡（位于主页下半屏）：
//! - 面向普通用户的精简中文筛选栏（全部 / 关键事件 / 仅异常）+ 紧凑可展开搜索 + 滚底芯片 + 打开原文件
//! - 纯展示层人性化翻译：将 `[STATE_CHANGE]`、`ONLINE → OFFLINE`、`AUTH_EXPIRED` 等技术代码转成易读中文，
//!   同时保持磁盘 `app.log` 原格式 100% 兼容不变
//! - 严格单行限宽截断（无横向滚动条）；内容过长时鼠标悬停展示结构化详情小卡片，单击行可复制完整日志

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

pub struct LogsState {
    /// 0 = 全部，1 = 关键事件（隐藏例行巡检流水），2 = 仅异常
    pub view_filter: usize,
    pub search: String,
    pub search_open: bool,
    pub focus_search: bool,
    pub follow: bool,
}

impl Default for LogsState {
    fn default() -> Self {
        Self {
            view_filter: 0,
            search: String::new(),
            search_open: false,
            focus_search: false,
            follow: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogTone {
    Ok,
    Accent,
    Warn,
    Danger,
    Neutral,
}

#[derive(Debug, Clone)]
struct HumanLogLine {
    full_stamp: String,
    time_short: String,
    tone: LogTone,
    category: String,
    summary: String,
    is_key_event: bool,
    is_anomaly: bool,
    raw: String,
}

impl LogTone {
    fn colors(self, p: &Palette) -> (Color32, Color32, Color32) {
        match self {
            Self::Ok => (p.ok, p.ok_soft, p.ok_border),
            Self::Accent => (p.accent, p.accent_soft, p.accent_border),
            Self::Warn => (p.warn, p.warn_soft, p.warn_border),
            Self::Danger => (p.danger, p.danger_soft, p.danger_border),
            Self::Neutral => (p.neutral, p.neutral_soft, p.neutral_border),
        }
    }
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
            let category_ok = match state.view_filter {
                1 => entry.is_key_event,
                2 => entry.is_anomaly,
                _ => true,
            };
            if !category_ok {
                return false;
            }
            if query.is_empty() {
                return true;
            }
            entry.summary.to_lowercase().contains(&query)
                || entry.category.to_lowercase().contains(&query)
                || entry.raw.to_lowercase().contains(&query)
        })
        .collect();

    card_padded(ui, p, 12, |ui| {
        let inner_h = (target_height - 24.0).max(120.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);

        // ── 26px 极简语义工具栏 ────────────────────────────────────────
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

                let tabs = ["全部", "关键事件", "仅异常"];
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

                    draw_follow_chip(ui, p, &mut state.follow);

                    // 紧凑可折叠搜索：默认只占 26×26 放大镜按钮，点击后展开为 136px 精致搜索框
                    if state.search_open || !state.search.is_empty() {
                        let box_w = (ui.available_width() - 6.0).clamp(92.0, 136.0);
                        draw_compact_search_box(ui, p, state, box_w);
                    } else if icon_button(
                        ui,
                        p,
                        Icon::Search,
                        Vec2::splat(26.0),
                        13.0,
                        false,
                        "搜索日志关键字",
                    )
                    .clicked()
                    {
                        state.search_open = true;
                        state.focus_search = true;
                    }
                });
            },
        );

        ui.add_space(7.0);

        // ── 日志流容器（严格单行截断，无横向滚动条） ───────────────────
        egui::Frame::new()
            .fill(p.subtle)
            .stroke(Stroke::new(1.0_f32, p.divider))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height((ui.available_height() - 2.0).max(70.0));

                if filtered.is_empty() {
                    let empty_hint = match state.view_filter {
                        2 => "当前没有异常日志记录",
                        1 => "当前没有关键变动记录",
                        _ => "没有匹配的日志记录",
                    };
                    ui.centered_and_justified(|ui| {
                        ui.label(body_text(empty_hint, 12.0, p.text_muted));
                    });
                    return;
                }

                let row_height = 24.0;
                let view_w = ui.available_width();
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt("embedded_log_scroll")
                    .auto_shrink([false, false]);
                if state.follow {
                    scroll = scroll.stick_to_bottom(true);
                }

                scroll.show_rows(ui, row_height, filtered.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // 留出 6px 右侧安全边距避让垂直滚动条
                    let row_w = (view_w - 6.0).max(180.0);
                    for index in range {
                        paint_log_row(ui, p, &filtered[index], index, row_height, row_w);
                    }
                });
            });
    });
}

fn draw_follow_chip(ui: &mut egui::Ui, p: &Palette, follow: &mut bool) {
    let label = "滚底";
    let font = if *follow {
        egui::FontId::new(11.0, crate::ui::theme::bold_family())
    } else {
        egui::FontId::proportional(11.0)
    };
    let (fill, stroke, fg, dot_c) = if *follow {
        (
            p.accent_soft,
            Stroke::new(1.0_f32, p.accent_border),
            p.accent,
            p.accent,
        )
    } else {
        (
            p.subtle,
            Stroke::new(1.0_f32, p.border),
            p.text_muted,
            p.text_muted,
        )
    };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    let w = 8.0 + 6.0 + 5.0 + galley.size().x + 8.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let hover_fill = if resp.hovered() && !*follow {
        p.card_hover
    } else {
        fill
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(RADIUS_SM),
        hover_fill,
        stroke,
        StrokeKind::Inside,
    );
    let cy = rect.center().y;
    ui.painter()
        .circle_filled(Pos2::new(rect.left() + 10.5, cy), 2.8, dot_c);
    ui.painter().galley(
        Pos2::new(rect.left() + 18.0, cy - galley.size().y * 0.5),
        galley,
        fg,
    );
    if resp.clicked() {
        *follow = !*follow;
    }
    resp.on_hover_text(if *follow {
        "自动滚底：已开启（有新日志时自动跳到底部）"
    } else {
        "自动滚底：已暂停（点击恢复自动跟踪最新日志）"
    });
}

fn draw_compact_search_box(ui: &mut egui::Ui, p: &Palette, state: &mut LogsState, width: f32) {
    egui::Frame::new()
        .fill(p.input_bg)
        .stroke(Stroke::new(1.0_f32, p.accent_border))
        .corner_radius(CornerRadius::same(RADIUS_SM))
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new((width - 14.0).max(76.0), 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let (s_rect, _) = ui.allocate_exact_size(Vec2::splat(11.5), Sense::hover());
                    icons::paint(ui.painter(), s_rect, Icon::Search, p.accent);
                    let edit_w = (ui.available_width() - 16.0).max(40.0);
                    let edit_resp = ui.add_sized(
                        [edit_w, 18.0],
                        egui::TextEdit::singleline(&mut state.search)
                            .hint_text(egui::RichText::new("搜索日志…").color(p.text_muted))
                            .vertical_align(egui::Align::Center)
                            .frame(false),
                    );
                    if state.focus_search {
                        edit_resp.request_focus();
                        state.focus_search = false;
                    }
                    if edit_resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        state.search.clear();
                        state.search_open = false;
                    }
                    let (clr, resp) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::click());
                    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                    icons::paint(
                        ui.painter(),
                        clr.shrink(1.5),
                        Icon::Close,
                        if resp.hovered() { p.text } else { p.text_muted },
                    );
                    if resp.clicked() {
                        state.search.clear();
                        state.search_open = false;
                    }
                },
            );
        });
}

/// 绘制单条人性化日志行（严格限宽，过长截断并在鼠标悬停时弹出结构化详情小卡片）。
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
    let font_cat = egui::FontId::new(10.0, crate::ui::theme::bold_family());

    let mut cursor_x = row_rect.left() + 4.0;

    // 1. 时间列（固定宽度，严格对齐）
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

    // 2. 中文语义分类胶囊（固定 58px 宽度，彻底告别 INFO/WARN/ERR 术语）
    let (cat_fg, cat_bg, cat_bd) = entry.tone.colors(p);
    let pill_w = 58.0_f32;
    let pill_rect = Rect::from_center_size(
        Pos2::new(cursor_x + pill_w * 0.5, cy),
        Vec2::new(pill_w, 17.0),
    );
    painter.rect(
        pill_rect,
        CornerRadius::same(4),
        cat_bg,
        Stroke::new(1.0_f32, cat_bd),
        StrokeKind::Inside,
    );
    let cg = painter.layout_no_wrap(entry.category.clone(), font_cat, cat_fg);
    painter.galley(
        Pos2::new(
            pill_rect.center().x - cg.size().x * 0.5,
            pill_rect.center().y - cg.size().y * 0.5,
        ),
        cg,
        cat_fg,
    );
    cursor_x += pill_w + 8.0;

    // 3. 正文（严格限宽截断；过长时末尾显示 … 并在悬停时弹出小卡片）
    let msg_max_w = (row_rect.right() - cursor_x - 6.0).max(40.0);
    let text_color = match entry.tone {
        LogTone::Danger => p.danger,
        LogTone::Warn => p.text,
        _ => p.text,
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

    if hovered && truncated {
        show_log_detail_tooltip(ui, p, ui.id().with(("log_row_detail", index)), entry);
    }
}

fn show_log_detail_tooltip(ui: &egui::Ui, p: &Palette, id: egui::Id, entry: &HumanLogLine) {
    let (cat_fg, cat_bg, cat_bd) = entry.tone.colors(p);
    egui::show_tooltip(ui.ctx(), ui.layer_id(), id, |ui| {
        ui.set_min_width(268.0);
        ui.set_max_width(360.0);
        ui.spacing_mut().item_spacing.y = 6.0;

        // 标题行：中文分类胶囊 + 完整日期时间
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                badge(ui, cat_fg, cat_bg, cat_bd, &entry.category);
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

        // 完整换行正文
        ui.add(egui::Label::new(bold_text(&entry.summary, 11.8, p.text)).wrap());

        let sep2 = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
        ui.painter().line_segment(
            [sep2.left_center(), sep2.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );

        ui.label(body_text(
            "单击该行可复制原始日志到剪贴板",
            10.0,
            p.text_muted,
        ));
    });
}

/// 将单行文本精确适配到 `max_w` 像素宽度内，超出则二分截断并附加 `…`。
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

/// 将底层结构化日志翻译为普通用户易读的中文分类与自然表述（不修改磁盘 `app.log`）。
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

    let (category, tone, is_key_event, is_anomaly) = match tag_str {
        "STATE_CHANGE" => {
            let going_offline = raw_msg.contains("→ OFFLINE") || raw_msg.contains("-> OFFLINE");
            let going_online = raw_msg.contains("→ ONLINE") || raw_msg.contains("-> ONLINE");
            let going_suspect = raw_msg.contains("→ SUSPECT") || raw_msg.contains("-> SUSPECT");
            let tone = if going_offline {
                LogTone::Warn
            } else if going_online {
                LogTone::Ok
            } else if going_suspect {
                LogTone::Warn
            } else {
                LogTone::Accent
            };
            ("状态变化", tone, true, going_offline || going_suspect)
        }
        "HEALTH_CHECK" => ("日常巡检", LogTone::Ok, false, false),
        "FORCE_CHECK" => ("手动检测", LogTone::Neutral, true, false),
        "FORCE_CHECK_RESULT" => {
            let bad = is_warn || is_err || raw_msg.contains("异常");
            (
                "手动检测",
                if bad { LogTone::Warn } else { LogTone::Ok },
                true,
                bad,
            )
        }
        "LOGIN_VERIFIED" | "NETWORK_RECOVERED" | "AUTH_RETRY_OK" => {
            ("恢复在线", LogTone::Ok, true, false)
        }
        "LOGIN_ACCEPTED" => ("自动登录", LogTone::Accent, true, false),
        "AUTH_RETRY" | "RETRY_BACKOFF" => ("等待重试", LogTone::Warn, true, false),
        "LOGIN_NOT_VERIFIED"
        | "LOGIN_REJECTED"
        | "PORTAL_REJECTED"
        | "PORTAL_ERROR"
        | "PORTAL_UNAVAILABLE"
        | "CREDENTIALS_MISSING" => ("认证异常", LogTone::Danger, true, true),
        "ACCOUNT_SWITCH" | "PORTAL_LOGOUT" => ("切换账号", LogTone::Accent, true, false),
        "ACCOUNT_SWITCHED" => ("切换成功", LogTone::Ok, true, false),
        "SWITCH_ABORTED" | "SWITCH_NOT_VERIFIED" | "SWITCH_FAILED" => {
            ("切号失败", LogTone::Danger, true, true)
        }
        "NOTIFY_SENT" => ("系统通知", LogTone::Neutral, true, false),
        "NOTIFY_RETRY" => ("通知异常", LogTone::Warn, true, true),
        "SETTINGS_SAVED" => ("配置更新", LogTone::Accent, true, false),
        "CONFIG" | "NET_POLICY" | "UI" => ("运行配置", LogTone::Neutral, false, false),
        "CAMPUS_STATUS" => ("门户状态", LogTone::Neutral, false, false),
        "MONITOR_STARTED" | "MONITOR_PAUSED" => ("守护状态", LogTone::Accent, true, false),
        "NETWORK_CHANGE" | "NETWORK_SWITCH" => ("网络变化", LogTone::Accent, true, false),
        "APP_STARTED" | "APP_READY" | "APP_EXIT" | "STARTUP" | "SELF_CHECK" => {
            ("程序状态", LogTone::Neutral, true, is_err)
        }
        "DIAGNOSTIC" | "DIAGNOSTIC_ERROR" => (
            "网络诊断",
            if is_err {
                LogTone::Danger
            } else if is_warn {
                LogTone::Warn
            } else {
                LogTone::Neutral
            },
            true,
            is_warn || is_err,
        ),
        _ if is_err || tag_str.ends_with("_ERROR") || tag_str.ends_with("_FAILED") => {
            ("运行异常", LogTone::Danger, true, true)
        }
        _ if is_warn => ("运行警告", LogTone::Warn, true, true),
        _ => ("系统记录", LogTone::Neutral, true, false),
    };

    let summary = humanize_message(tag_str, &raw_msg);
    HumanLogLine {
        full_stamp,
        time_short,
        tone,
        category: category.to_string(),
        summary,
        is_key_event,
        is_anomaly,
        raw: line.to_string(),
    }
}

fn humanize_message(tag: &str, raw_msg: &str) -> String {
    let mut text = raw_msg.trim().to_string();

    // 1. 先处理 `原因=<CODE>`：去掉冗余的 `原因=ONLINE`，其余原因码翻译为大白话中文
    for prefix in [
        "；原因=ONLINE",
        "; 原因=ONLINE",
        ";原因=ONLINE",
        "原因=ONLINE",
    ] {
        text = text.replace(prefix, "");
    }

    const REASON_CODES: &[&str] = &[
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
    ];
    for code in REASON_CODES {
        let label = reason_label(code);
        text = text.replace(&format!("原因={code}"), &format!("原因：{label}"));
        text = text.replace(code, label);
    }

    // 2. 清理空返回码与布尔技术值
    for empty_ret in [
        "；门户返回码=-",
        "; 门户返回码=-",
        "；门户返回码=",
        "; 门户返回码=",
    ] {
        if text.ends_with(empty_ret) {
            text.truncate(text.len() - empty_ret.len());
        }
    }
    text = text.replace("门户返回码=", "返回码 ");
    text = text.replace("success=true", "注销成功");
    text = text.replace("success=false", "门户未确认注销");

    // 3. 翻译所有状态机英文枚举（按长到短避免子串冲突）
    const STATE_TOKENS: &[(&str, &str)] = &[
        ("AUTHENTICATING", "认证中"),
        ("VERIFYING", "验证中"),
        ("CHECKING", "检测中"),
        ("SUSPECT", "待复核"),
        ("OFFLINE", "已断开"),
        ("STOPPED", "已停止"),
        ("PAUSED", "已暂停"),
        ("ONLINE", "已在线"),
    ];
    for (eng, zh) in STATE_TOKENS {
        text = text.replace(eng, zh);
    }

    // 4. 优化常见键值对与标点排版
    text = text.replace("检测耗时=", "耗时 ");
    text = text.replace("心跳间隔=", "心跳 ");
    text = text.replace("最后原因=", "原断联原因：");
    text = text.replace("IP=", "IP ");
    text = text.replace("网关=", "网关 ");
    text = text.replace("门户识别IP ", "门户识别 IP ");
    text = text.replace("系统弹窗: ", "系统弹窗：");
    text = text.replace("系统弹窗:", "系统弹窗：");

    // 5. 针对「认证服务器未返回说明」补全更自然的语义
    if tag == "LOGIN_ACCEPTED" && text.trim() == "认证服务器未返回说明" {
        return "认证请求已受理，等待网络验证".to_string();
    }

    // 6. 将生硬的英文/中文分号统一为轻盈的 ` · ` 分隔，并规整空白
    let parts: Vec<String> = text
        .split([';', '；'])
        .map(|seg| seg.trim().trim_end_matches('·').trim())
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            // STATE_CHANGE 行常见的 `已在线 → 已断开 准备自动登录` 补上清晰分隔
            if let Some(arrow_pos) = seg.find('→') {
                let after = &seg[arrow_pos + '→'.len_utf8()..];
                if let Some((to_state, rest)) = after.trim_start().split_once(' ') {
                    let rest = rest.trim();
                    if !rest.is_empty() {
                        let before = seg[..arrow_pos].trim();
                        return format!("{before} → {to_state} · {rest}");
                    }
                }
            }
            seg.to_string()
        })
        .collect();

    if parts.is_empty() {
        raw_msg.trim().to_string()
    } else {
        parts.join(" · ")
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
    fn humanizes_state_change_and_reason_codes_cleanly() {
        let offline = format_human_log(
            "2026-11-22 02:47:41 INFO [STATE_CHANGE] ONLINE → OFFLINE 准备自动登录校园网; 原因=AUTH_EXPIRED",
        );
        assert_eq!(offline.category, "状态变化");
        assert!(offline.is_key_event);
        assert!(offline.is_anomaly);
        assert_eq!(
            offline.summary,
            "已在线 → 已断开 · 准备自动登录校园网 · 原因：校园网认证掉线"
        );

        let recovered = format_human_log(
            "2026-11-22 02:47:42 INFO [STATE_CHANGE] VERIFYING → ONLINE; 校园网登录成功，公网已恢复; 原因=ONLINE",
        );
        assert_eq!(recovered.category, "状态变化");
        assert!(!recovered.is_anomaly);
        assert_eq!(
            recovered.summary,
            "验证中 → 已在线 · 校园网登录成功，公网已恢复"
        );

        let accepted = format_human_log(
            "2026-11-22 02:47:41 INFO [LOGIN_ACCEPTED] 认证服务器未返回说明; 门户返回码=",
        );
        assert_eq!(accepted.category, "自动登录");
        assert_eq!(accepted.summary, "认证请求已受理，等待网络验证");

        let health = format_human_log(
            "2026-11-22 01:29:23 INFO [HEALTH_CHECK] 网络正常；IP=198.18.0.1；网关=198.18.0.2；检测耗时=0.12s；心跳间隔=30分钟",
        );
        assert_eq!(health.category, "日常巡检");
        assert!(!health.is_key_event);
        assert!(!health.is_anomaly);
        assert!(health.summary.contains("耗时 0.12s"));
        assert!(!health.summary.contains('='));
    }
}
