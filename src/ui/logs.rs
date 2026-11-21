//! 嵌入式运行日志卡（位于右侧下半屏）：
//! - 26px 严格统一中线工具栏（标题、计数胶囊、级别分段、搜索框、滚底开关、复制/打开）
//! - 纯 Painter 单中线行排版：时间与标签等宽对齐，中文正文用 Proportional（Segoe UI + 雅黑）彻底消除中英基线错位

use std::path::Path;

use chrono::{Local, NaiveDateTime, TimeZone};
use eframe::egui::{self, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::{body_text, parse_log_line, Palette, RADIUS_SM};
use crate::ui::widgets::{
    badge, card_padded, icon_button, section_heading, segmented_control, toggle_switch,
};

pub struct LogsState {
    pub level_filter: usize, // 0=全部, 1=INFO, 2=WARN, 3=ERROR
    pub time_filter: usize,  // 0=全部, 1=近1h, 2=近6h, 3=近24h
    pub search: String,
    pub follow: bool,
}

impl Default for LogsState {
    fn default() -> Self {
        Self {
            level_filter: 0,
            time_filter: 0,
            search: String::new(),
            follow: true,
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
    let cutoff_ts: Option<i64> = match state.time_filter {
        1 => Some(Local::now().timestamp() - 3600),
        2 => Some(Local::now().timestamp() - 6 * 3600),
        3 => Some(Local::now().timestamp() - 24 * 3600),
        _ => None,
    };

    let query = state.search.trim().to_lowercase();
    let filtered: Vec<&String> = lines
        .iter()
        .filter(|line| {
            if let Some(cutoff) = cutoff_ts {
                if let Some(ts) = parse_log_timestamp(line) {
                    if ts < cutoff {
                        return false;
                    }
                }
            }
            let (_, level, _, _) = parse_log_line(line);
            let level_ok = match state.level_filter {
                1 => level == "INFO",
                2 => level == "WARNING",
                3 => level == "ERROR",
                _ => true,
            };
            if !level_ok {
                return false;
            }
            query.is_empty() || line.to_lowercase().contains(&query)
        })
        .collect();

    card_padded(ui, p, 12, |ui| {
        let inner_h = (target_height - 24.0).max(120.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);

        // ── 26px 严格统一中线工具栏 ──────────────────────────────────────
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
                    format!("{}", lines.len())
                } else {
                    format!("{}/{}", filtered.len(), lines.len())
                };
                badge(ui, p.text_secondary, p.subtle, p.border, &count_label);

                let levels = ["全部", "INFO", "WARN", "ERR"];
                segmented_control(ui, p, "inline_log_lvl", &mut state.level_filter, &levels);

                let time_opts = ["全部", "近1h", "近6h", "近24h"];
                segmented_control(ui, p, "inline_log_time", &mut state.time_filter, &time_opts);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;

                    if icon_button(
                        ui,
                        p,
                        Icon::ExternalLink,
                        Vec2::splat(26.0),
                        13.5,
                        false,
                        "打开 app.log 原文件",
                    )
                    .clicked()
                    {
                        open_path(log_path);
                    }

                    toggle_switch(ui, p, &mut state.follow, true);
                    ui.label(body_text("滚底", 11.0, p.text_muted));

                    // 26px 等高搜索框
                    let search_w = (ui.available_width() - 8.0).clamp(80.0, 180.0);
                    if search_w >= 86.0 {
                        egui::Frame::new()
                            .fill(p.input_bg)
                            .stroke(Stroke::new(1.0_f32, p.border))
                            .corner_radius(CornerRadius::same(RADIUS_SM))
                            .inner_margin(egui::Margin::symmetric(7, 2))
                            .show(ui, |ui| {
                                ui.allocate_ui_with_layout(
                                    Vec2::new((search_w - 16.0).max(60.0), 20.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.x = 5.0;
                                        let (s_rect, _) = ui
                                            .allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                                        icons::paint(
                                            ui.painter(),
                                            s_rect,
                                            Icon::Search,
                                            p.text_muted,
                                        );
                                        let edit_w = (ui.available_width()
                                            - if state.search.is_empty() { 2.0 } else { 18.0 })
                                        .max(40.0);
                                        ui.add_sized(
                                            [edit_w, 18.0],
                                            egui::TextEdit::singleline(&mut state.search)
                                                .hint_text("筛选关键字…")
                                                .vertical_align(egui::Align::Center)
                                                .frame(false),
                                        );
                                        if !state.search.is_empty() {
                                            let (clr, resp) = ui.allocate_exact_size(
                                                Vec2::splat(12.0),
                                                Sense::click(),
                                            );
                                            let resp =
                                                resp.on_hover_cursor(CursorIcon::PointingHand);
                                            icons::paint(
                                                ui.painter(),
                                                clr.shrink(1.5),
                                                Icon::Close,
                                                if resp.hovered() { p.text } else { p.text_muted },
                                            );
                                            if resp.clicked() {
                                                state.search.clear();
                                            }
                                        }
                                    },
                                );
                            });
                    }
                });
            },
        );

        ui.add_space(7.0);

        // ── 日志流容器 ──────────────────────────────────────────────────
        egui::Frame::new()
            .fill(p.subtle)
            .stroke(Stroke::new(1.0_f32, p.divider))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height((ui.available_height() - 2.0).max(70.0));

                if filtered.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(body_text("没有匹配的日志记录", 12.0, p.text_muted));
                    });
                    return;
                }

                let row_height = 22.0;
                let view_w = ui.available_width();
                let mut scroll = egui::ScrollArea::both()
                    .id_salt("embedded_log_scroll")
                    .auto_shrink([false, false]);
                if state.follow {
                    scroll = scroll.stick_to_bottom(true);
                }

                scroll.show_rows(ui, row_height, filtered.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for index in range {
                        let line = filtered[index];
                        paint_log_row(ui, p, line, row_height, view_w);
                    }
                });
            });
    });
}

fn parse_log_timestamp(line: &str) -> Option<i64> {
    let prefix = line.get(..19)?;
    let naive = NaiveDateTime::parse_from_str(prefix, "%Y-%m-%d %H:%M:%S").ok()?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.timestamp())
}

/// 单 LayoutJob + 纯 Painter 绘制单条日志：
/// 时间戳、[TAG] 与中英正文在同一个 LayoutJob 内排版，共享完全一致的排版基线；
/// 级别徽章精确嵌入时间与 [TAG] 之间，垂直中线对齐文字视觉中线。
fn paint_log_row(ui: &mut egui::Ui, p: &Palette, line: &str, row_height: f32, min_w: f32) {
    let (time, level, tag, message) = parse_log_line(line);
    let (lvl_text, lvl_fg, lvl_bg, lvl_bd) = match level.as_str() {
        "ERROR" => ("ERR", p.danger, p.danger_soft, p.danger_border),
        "WARNING" => ("WARN", p.warn, p.warn_soft, p.warn_border),
        _ => ("INFO", p.neutral, p.neutral_soft, p.neutral_border),
    };

    let pill_w = 34.0_f32;
    let gap = 7.0_f32;
    let font_regular = egui::FontId::proportional(11.5);
    let font_bold = egui::FontId::new(11.5, crate::ui::theme::bold_family());

    // 先测时间戳宽度（用同一 11.5pt 字族，确保与后续文字基线 100% 相同）
    let time_w = if time.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(time.clone(), font_regular.clone(), p.text_muted)
            .size()
            .x
    };

    // 将「时间 + 徽章留白 + [TAG] + 正文」合并到同一个 LayoutJob，保证基线零偏差
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;

    if !time.is_empty() {
        job.append(
            &time,
            0.0,
            egui::TextFormat {
                font_id: font_regular.clone(),
                color: p.text_muted,
                ..Default::default()
            },
        );
    }

    let badge_slot = if time.is_empty() {
        pill_w + gap
    } else {
        gap + pill_w + gap
    };

    if let Some(tag_str) = tag.as_ref() {
        let tag_color = match tag_str.as_str() {
            "HEALTH_CHECK" | "LOGIN_VERIFIED" | "NETWORK_RECOVERED" => p.ok,
            "FORCE_CHECK_RESULT" if level != "WARNING" && level != "ERROR" => p.ok,
            "STATE_CHANGE" | "APP_STARTED" | "APP_READY" => p.accent,
            _ if level == "ERROR" => p.danger,
            _ if level == "WARNING" => p.warn,
            _ => p.text_secondary,
        };
        job.append(
            &format!("[{tag_str}]"),
            badge_slot,
            egui::TextFormat {
                font_id: font_bold,
                color: tag_color,
                ..Default::default()
            },
        );
        job.append(
            &message,
            gap,
            egui::TextFormat {
                font_id: font_regular,
                color: p.text,
                ..Default::default()
            },
        );
    } else {
        job.append(
            &message,
            badge_slot,
            egui::TextFormat {
                font_id: font_regular,
                color: p.text,
                ..Default::default()
            },
        );
    }

    let galley = ui.painter().layout_job(job);
    let content_w = galley.size().x + 12.0;

    let (row_rect, _) =
        ui.allocate_exact_size(Vec2::new(content_w.max(min_w), row_height), Sense::hover());
    let cy = row_rect.center().y;
    let text_top = cy - galley.size().y * 0.5;
    let painter = ui.painter();

    // 绘制整行统一基线文本（时间 + [TAG] + 正文）
    painter.galley(Pos2::new(row_rect.left(), text_top), galley, p.text);

    // 在时间与 [TAG] 之间的预留槽位中绘制级别徽章，与整行垂直中线齐平
    let pill_left = if time.is_empty() {
        row_rect.left()
    } else {
        row_rect.left() + time_w + gap
    };
    let pill_rect = Rect::from_center_size(
        Pos2::new(pill_left + pill_w * 0.5, cy),
        Vec2::new(pill_w, 15.0),
    );
    painter.rect(
        pill_rect,
        CornerRadius::same(4),
        lvl_bg,
        Stroke::new(1.0_f32, lvl_bd),
        StrokeKind::Inside,
    );
    painter.text(
        pill_rect.center(),
        egui::Align2::CENTER_CENTER,
        lvl_text,
        egui::FontId::new(9.0, crate::ui::theme::bold_family()),
        lvl_fg,
    );
}

fn open_path(path: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", &path.to_string_lossy()])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}
