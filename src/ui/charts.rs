//! 主页可视化核心卡片：
//! - 左侧主卡（~62%）：连接稳定性与可上下滚动的断联事件明细（36 格圆角健康时间轴 + 悬停明细）
//! - 右侧辅卡（~38%）：本机流量统计（自适应双模式环形图 + 近 7 天比例柱状图）

use std::f32::consts::PI;

use chrono::{Datelike, Local, NaiveDate, TimeZone, Timelike, Weekday};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::metrics_store::{
    build_health_track, compute_stats, filter_range, outages_in_range, OutageEvent, TelemetryPoint,
    TimeRange, TrackSegment, TrackState,
};
use crate::models::{DailyTraffic, TrafficSummary};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::{
    body_text, bold_text, reason_hint, reason_label, with_alpha, Palette, DEFAULT_TRAFFIC_SPLIT,
    DEFAULT_WIDE_SPLIT,
};
use crate::ui::widgets::{
    badge, card_padded, inner_vertical_splitter, section_heading, segmented_control,
    vertical_splitter,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrafficPeriod {
    #[default]
    Today,
    Month,
}

impl TrafficPeriod {
    pub fn index(self) -> usize {
        match self {
            Self::Today => 0,
            Self::Month => 1,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        match idx {
            1 => Self::Month,
            _ => Self::Today,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_home_visual_row(
    ui: &mut egui::Ui,
    p: &Palette,
    all_points: &[TelemetryPoint],
    range: &mut TimeRange,
    traffic: &TrafficSummary,
    traffic_period: &mut TrafficPeriod,
    monthly_quota_gb: i64,
    show_traffic: bool,
    split_ratio: &mut f32,
    target_height: f32,
) -> bool {
    if !show_traffic {
        return draw_stability_wide_card(ui, p, all_points, range, split_ratio, target_height);
    }

    let total_w = ui.available_width();
    let splitter_w = 10.0_f32;
    let usable_w = (total_w - splitter_w).max(460.0);
    let min_left = 300.0_f32.min(usable_w * 0.52);
    let min_right = 210.0_f32.min(usable_w * 0.38);
    let left_w = (usable_w * (*split_ratio).clamp(0.48, 0.74))
        .clamp(min_left, (usable_w - min_right).max(min_left))
        .floor();
    let right_w = (usable_w - left_w).max(min_right);
    let mut split_changed = false;

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.allocate_ui_with_layout(
            Vec2::new(left_w, target_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                draw_stability_card(ui, p, all_points, range, target_height);
            },
        );
        let (dx, dbl) = vertical_splitter(ui, p, target_height);
        if dbl {
            *split_ratio = DEFAULT_TRAFFIC_SPLIT;
            split_changed = true;
        } else if dx.abs() > 0.01 && usable_w > 1.0 {
            *split_ratio = ((left_w + dx) / usable_w).clamp(0.48, 0.74);
            split_changed = true;
        }
        ui.allocate_ui_with_layout(
            Vec2::new(right_w, target_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                draw_traffic_card(
                    ui,
                    p,
                    traffic,
                    traffic_period,
                    monthly_quota_gb,
                    target_height,
                );
            },
        );
    });
    split_changed
}

/// 关闭「本机流量」时的通栏宽屏版稳定性面板：
/// - 左侧：自绘在线率环形图 + 健康结论/主因分布 + 饱满通断时间轴
/// - 中央：可左右拖拽调整双栏比例（双击恢复默认）
/// - 右侧：独占整卡高度的可上下滚动断联事件明细
pub fn draw_stability_wide_card(
    ui: &mut egui::Ui,
    p: &Palette,
    all_points: &[TelemetryPoint],
    range: &mut TimeRange,
    split_ratio: &mut f32,
    target_height: f32,
) -> bool {
    let now = Local::now().timestamp();
    let points = filter_range(all_points, now, *range);
    let stats = compute_stats(&points);
    let outages = outages_in_range(all_points, now, *range);
    let track = build_health_track(all_points, now, *range);
    let reasons = outages.reason_counts();
    let has_outages = !outages.events.is_empty();
    let mut split_changed = false;

    let link_id = ui.id().with("wide_linked_outage");
    let prev_linked: Option<usize> = ui.ctx().data(|d| d.get_temp(link_id)).flatten();
    let mut hovered_from_pin: Option<usize> = None;
    let mut hovered_from_row: Option<usize> = None;

    card_padded(ui, p, 12, |ui| {
        let inner_h = (target_height - 24.0).max(170.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);

        // 1. 顶部标题 + 1h / 7h / 24h 分段切换
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                section_heading(ui, Icon::Shield, p.ok, "连接稳定性与断联记录", p.text, 26.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut idx = range.index();
                    let labels = ["1h", "7h", "24h"];
                    if segmented_control(ui, p, "stab_wide_range_seg", &mut idx, &labels) {
                        *range = TimeRange::from_index(idx);
                    }
                });
            },
        );

        ui.add_space(8.0);

        let body_h = (inner_h - 34.0).max(134.0);
        let total_w = ui.available_width();
        let splitter_w = 18.0_f32;
        let usable_w = (total_w - splitter_w).max(420.0);
        let min_left = 240.0_f32.min(usable_w * 0.45);
        let min_right = 210.0_f32.min(usable_w * 0.42);
        let left_w = (usable_w * (*split_ratio).clamp(0.38, 0.68))
            .clamp(min_left, (usable_w - min_right).max(min_left))
            .floor();
        let right_w = (usable_w - left_w).max(min_right);

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;

            // ── 左侧视觉概览：在线率环形图 + 原因构成 + 健康时间轴 ──
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, body_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.set_min_size(Vec2::new(left_w, body_h));

                    // 上半：环形图 + 结构化摘要
                    ui.allocate_ui_with_layout(
                        Vec2::new(left_w, 78.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;

                            // 74×74 自绘在线率环形图
                            let (ring_rect, ring_resp) =
                                ui.allocate_exact_size(Vec2::splat(74.0), Sense::hover());
                            let center = ring_rect.center();
                            let radius = 30.0_f32;
                            let thickness = 7.5_f32;
                            let painter = ui.painter();

                            painter.circle_stroke(
                                center,
                                radius,
                                Stroke::new(thickness, p.divider),
                            );
                            if let Some(pct) = stats.uptime_pct {
                                let ratio = (pct as f32 / 100.0).clamp(0.0, 1.0);
                                let arc_color = if pct >= 98.0 {
                                    p.ok
                                } else if pct >= 90.0 {
                                    p.warn
                                } else {
                                    p.danger
                                };
                                if ratio > 0.005 {
                                    paint_ring_arc(
                                        painter,
                                        center,
                                        radius,
                                        -PI * 0.5,
                                        -PI * 0.5 + ratio * 2.0 * PI,
                                        Stroke::new(thickness, arc_color),
                                    );
                                }
                                let rate_text = format_uptime_pct(pct, has_outages);
                                painter.text(
                                    Pos2::new(center.x, center.y - 6.0),
                                    egui::Align2::CENTER_CENTER,
                                    rate_text,
                                    egui::FontId::new(14.5, crate::ui::theme::bold_family()),
                                    p.text,
                                );
                                painter.text(
                                    Pos2::new(center.x, center.y + 10.0),
                                    egui::Align2::CENTER_CENTER,
                                    "在线率",
                                    egui::FontId::proportional(9.5),
                                    p.text_muted,
                                );
                            } else {
                                painter.text(
                                    Pos2::new(center.x, center.y - 5.0),
                                    egui::Align2::CENTER_CENTER,
                                    "—",
                                    egui::FontId::new(15.0, crate::ui::theme::bold_family()),
                                    p.text_muted,
                                );
                                painter.text(
                                    Pos2::new(center.x, center.y + 10.0),
                                    egui::Align2::CENTER_CENTER,
                                    "待采样",
                                    egui::FontId::proportional(9.5),
                                    p.text_muted,
                                );
                            }
                            ring_resp.on_hover_text(format!(
                                "统计过去 {} 内 {} 次巡检样本的公网连通占比",
                                range.label(),
                                stats.check_count
                            ));

                            // 右侧健康结论 + 断联原因构成（无冗余平均恢复/次数 KPI）
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 5.0;
                                let (verdict, verdict_color) = match stats.uptime_pct {
                                    Some(pct) if outages.events.is_empty() && pct >= 99.5 => {
                                        ("全程保持稳定", p.ok)
                                    }
                                    Some(pct) if pct >= 95.0 => ("偶发波动，已自动恢复", p.text),
                                    Some(pct) if pct >= 85.0 => ("存在明显掉线波动", p.warn),
                                    Some(_) => ("连接持续不稳定", p.danger),
                                    None => ("等待巡检采样", p.text_muted),
                                };
                                ui.allocate_ui_with_layout(
                                    Vec2::new(ui.available_width(), 18.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.label(bold_text(verdict, 12.5, verdict_color));
                                        if stats.check_count > 0 {
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(body_text(
                                                        format!("{} 次巡检", stats.check_count),
                                                        10.5,
                                                        p.text_muted,
                                                    ));
                                                },
                                            );
                                        }
                                    },
                                );

                                let div = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
                                ui.painter().line_segment(
                                    [div.left_center(), div.right_center()],
                                    Stroke::new(1.0_f32, p.divider),
                                );

                                if reasons.is_empty() {
                                    if stats.check_count > 0 {
                                        legend_kv_row(
                                            ui,
                                            p,
                                            Some(p.ok),
                                            &format!("过去 {} 确认断联", range.label()),
                                            "0 次",
                                        );
                                        let avg_str = stats
                                            .avg_ms
                                            .map(|ms| format!("{ms} ms"))
                                            .unwrap_or_else(|| "正常".to_string());
                                        legend_kv_row(ui, p, None, "平均网关响应", &avg_str);
                                    } else {
                                        ui.label(body_text(
                                            "开启监控后自动记录通断状态与断联主因",
                                            11.0,
                                            p.text_muted,
                                        ));
                                    }
                                } else {
                                    for (reason, cnt) in reasons.iter().take(2) {
                                        let dot = reason_severity_color(p, reason);
                                        legend_kv_row(
                                            ui,
                                            p,
                                            Some(dot),
                                            reason_label(reason),
                                            &format!("{cnt} 次"),
                                        );
                                    }
                                    if reasons.len() == 1 {
                                        ui.add(
                                            egui::Label::new(body_text(
                                                reason_hint(reasons[0].0),
                                                10.0,
                                                p.text_muted,
                                            ))
                                            .truncate(),
                                        );
                                    } else if reasons.len() > 2 {
                                        let other_cnt: usize =
                                            reasons[2..].iter().map(|(_, c)| *c).sum();
                                        ui.label(body_text(
                                            format!(
                                                "其余 {} 类原因共 {} 次（详见右侧列表）",
                                                reasons.len() - 2,
                                                other_cnt
                                            ),
                                            10.0,
                                            p.text_muted,
                                        ));
                                    }
                                }
                            });
                        },
                    );

                    ui.add_space(7.0);
                    let hsep = ui.allocate_space(Vec2::new(left_w, 1.0)).1;
                    ui.painter().line_segment(
                        [hsep.left_center(), hsep.right_center()],
                        Stroke::new(1.0_f32, p.divider),
                    );
                    ui.add_space(6.0);

                    // 下半：方案 A 连续状态轨道 + 精准事件针 + 真实时刻标尺
                    draw_ribbon_legend_header(ui, p, left_w);
                    ui.add_space(3.0);
                    hovered_from_pin = draw_health_ribbon(
                        ui,
                        p,
                        &track,
                        &outages.events,
                        now,
                        *range,
                        prev_linked,
                        true,
                    );
                },
            );

            // 中央可拖拽竖向分隔条
            let (dx, dbl) = inner_vertical_splitter(ui, p, body_h);
            if dbl {
                *split_ratio = DEFAULT_WIDE_SPLIT;
                split_changed = true;
            } else if dx.abs() > 0.01 && usable_w > 1.0 {
                *split_ratio = ((left_w + dx) / usable_w).clamp(0.38, 0.68);
                split_changed = true;
            }

            // ── 右侧独立可滚动断联事件明细（享整卡高度，同屏展示 4–5 条） ──
            ui.allocate_ui_with_layout(
                Vec2::new(right_w, body_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(right_w, 16.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let count_text = if outages.events.is_empty() {
                                format!("过去 {} 无断联事件", range.label())
                            } else {
                                format!(
                                    "断联事件明细（共 {} 次，可上下滑动）",
                                    outages.events.len()
                                )
                            };
                            ui.label(bold_text(count_text, 11.0, p.text_secondary));
                            if !outages.events.is_empty() {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(body_text("按时间倒序", 10.0, p.text_muted));
                                    },
                                );
                            }
                        },
                    );
                    ui.add_space(5.0);

                    let list_h = (body_h - 21.0).max(76.0);
                    if outages.events.is_empty() {
                        egui::Frame::new()
                            .fill(p.subtle)
                            .stroke(Stroke::new(1.0_f32, p.divider))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width((right_w - 24.0).max(160.0));
                                ui.set_min_height((list_h - 22.0).max(60.0));
                                ui.centered_and_justified(|ui| {
                                    ui.horizontal(|ui| {
                                        let (r, _) = ui
                                            .allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                                        icons::paint(ui.painter(), r, Icon::Check, p.ok);
                                        ui.label(body_text(
                                            format!(
                                                "过去 {} 校园网保持稳定，未发生确认断联",
                                                range.label()
                                            ),
                                            11.5,
                                            p.text_secondary,
                                        ));
                                    });
                                });
                            });
                    } else {
                        ui.allocate_ui_with_layout(
                            Vec2::new(right_w, list_h),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("home_outages_wide_scroll")
                                    .max_height(list_h)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_width(right_w);
                                        ui.spacing_mut().item_spacing.y = 5.0;
                                        for (orig_idx, ev) in
                                            outages.events.iter().enumerate().rev()
                                        {
                                            let is_linked = prev_linked == Some(orig_idx);
                                            if draw_outage_event_row(ui, p, ev, is_linked) {
                                                hovered_from_row = Some(orig_idx);
                                            }
                                        }
                                    });
                            },
                        );
                    }
                },
            );
        });
    });

    let next_linked = hovered_from_pin.or(hovered_from_row);
    if next_linked != prev_linked {
        ui.ctx().data_mut(|d| d.insert_temp(link_id, next_linked));
        ui.ctx().request_repaint();
    }
    split_changed
}

fn reason_severity_color(p: &Palette, reason: &str) -> Color32 {
    if matches!(
        reason,
        "LINK_DOWN" | "NO_IP" | "NO_GATEWAY" | "GATEWAY_UNREACHABLE" | "LOGIN_REJECTED"
    ) {
        p.danger
    } else {
        p.warn
    }
}

/// 左侧 62% 主卡：在线率 + 连续状态轨道（方案 A）+ 可上下滚动断联事件列表。
pub fn draw_stability_card(
    ui: &mut egui::Ui,
    p: &Palette,
    all_points: &[TelemetryPoint],
    range: &mut TimeRange,
    target_height: f32,
) {
    let now = Local::now().timestamp();
    let points = filter_range(all_points, now, *range);
    let stats = compute_stats(&points);
    let outages = outages_in_range(all_points, now, *range);
    let track = build_health_track(all_points, now, *range);
    let has_outages = !outages.events.is_empty();

    let link_id = ui.id().with("compact_linked_outage");
    let prev_linked: Option<usize> = ui.ctx().data(|d| d.get_temp(link_id)).flatten();
    let mut hovered_from_pin: Option<usize> = None;
    let mut hovered_from_row: Option<usize> = None;

    card_padded(ui, p, 12, |ui| {
        let inner_h = (target_height - 24.0).max(196.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);

        // 1. 顶部标题 + 1h / 7h / 24h 分段切换
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                section_heading(ui, Icon::Shield, p.ok, "连接稳定性与断联记录", p.text, 26.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut idx = range.index();
                    let labels = ["1h", "7h", "24h"];
                    if segmented_control(ui, p, "stab_range_seg", &mut idx, &labels) {
                        *range = TimeRange::from_index(idx);
                    }
                });
            },
        );

        ui.add_space(5.0);

        // 2. 在线率大字 + 一句话主因/稳定摘要 + 紧凑图例
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 24.0),
            egui::Layout::left_to_right(egui::Align::BOTTOM),
            |ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                if let Some(pct) = stats.uptime_pct {
                    let rate_color = if pct >= 98.0 {
                        p.text
                    } else if pct >= 90.0 {
                        p.warn
                    } else {
                        p.danger
                    };
                    ui.label(bold_text(
                        format_uptime_pct(pct, has_outages),
                        21.0,
                        rate_color,
                    ));
                    ui.label(body_text("在线率", 10.5, p.text_muted));
                } else {
                    ui.label(bold_text("—", 21.0, p.text_muted));
                    ui.label(body_text("等待巡检采样", 10.5, p.text_muted));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(primary) = outages.primary_reason() {
                        ui.label(body_text(
                            format!("主因：{}", reason_label(primary)),
                            10.5,
                            p.text_secondary,
                        ));
                    } else if stats.check_count > 0 {
                        ui.label(body_text("当前时段全程稳定，无断联", 10.5, p.ok));
                    } else {
                        ui.label(body_text("开启监控后自动记录", 10.5, p.text_muted));
                    }
                });
            },
        );

        ui.add_space(4.0);

        // 3. 方案 A 连续状态轨道 + 精准事件针 + 真实时刻标尺
        hovered_from_pin = draw_health_ribbon(
            ui,
            p,
            &track,
            &outages.events,
            now,
            *range,
            prev_linked,
            false,
        );

        ui.add_space(6.0);

        // 4. 分隔线 + 断联事件列表标题
        let sep = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
        ui.painter().line_segment(
            [sep.left_center(), sep.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );
        ui.add_space(4.0);

        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 16.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let count_text = if outages.events.is_empty() {
                    format!("过去 {} 无断联事件", range.label())
                } else {
                    format!("断联事件明细（共 {} 次，可上下滑动）", outages.events.len())
                };
                ui.label(body_text(count_text, 10.5, p.text_muted));
                if !outages.events.is_empty() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(body_text("悬停联动时间轴", 10.0, p.text_muted));
                    });
                }
            },
        );

        ui.add_space(4.0);

        // 5. 独立可上下滚动断联事件列表
        let list_h = (ui.available_height() - 1.0).max(48.0);
        if outages.events.is_empty() {
            egui::Frame::new()
                .fill(p.subtle)
                .stroke(Stroke::new(1.0_f32, p.divider))
                .corner_radius(CornerRadius::same(7))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height((list_h - 18.0).max(32.0));
                    ui.centered_and_justified(|ui| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                            icons::paint(ui.painter(), r, Icon::Check, p.ok);
                            ui.label(body_text(
                                format!("过去 {} 校园网保持稳定，未发生确认断联", range.label()),
                                11.5,
                                p.text_secondary,
                            ));
                        });
                    });
                });
        } else {
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), list_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("home_outages_scroll")
                        .max_height(list_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 5.0;
                            for (orig_idx, ev) in outages.events.iter().enumerate().rev() {
                                let is_linked = prev_linked == Some(orig_idx);
                                if draw_outage_event_row(ui, p, ev, is_linked) {
                                    hovered_from_row = Some(orig_idx);
                                }
                            }
                        });
                },
            );
        }
    });

    let next_linked = hovered_from_pin.or(hovered_from_row);
    if next_linked != prev_linked {
        ui.ctx().data_mut(|d| d.insert_temp(link_id, next_linked));
        ui.ctx().request_repaint();
    }
}

fn format_uptime_pct(pct: f64, has_outages: bool) -> String {
    if !has_outages && pct >= 99.95 {
        "100%".to_string()
    } else {
        let capped = if has_outages { pct.min(99.9) } else { pct };
        format!("{capped:.1}%")
    }
}

fn draw_ribbon_legend_header(ui: &mut egui::Ui, p: &Palette, width: f32) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, 15.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(bold_text("通断时间轴", 11.0, p.text_secondary));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                legend_dot_item(ui, p, None, "未运行");
                legend_dot_item(ui, p, Some(p.warn), "断联");
                legend_dot_item(ui, p, Some(Color32::from_rgb(0x10, 0xB9, 0x81)), "在线");
            });
        },
    );
}

fn legend_dot_item(ui: &mut egui::Ui, p: &Palette, filled: Option<Color32>, text: &str) {
    let font = egui::FontId::proportional(9.8);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, p.text_muted);
    let total_w = 7.0 + 3.5 + galley.size().x;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total_w, 14.0), Sense::hover());
    let dot_center = Pos2::new(rect.left() + 3.5, rect.center().y);
    match filled {
        Some(c) => ui.painter().circle_filled(dot_center, 3.0, c),
        None => ui
            .painter()
            .circle_stroke(dot_center, 2.8, Stroke::new(1.1_f32, p.text_muted)),
    };
    ui.painter().galley(
        Pos2::new(rect.left() + 10.5, rect.center().y - galley.size().y * 0.5),
        galley,
        p.text_muted,
    );
}

/// 方案 A：连续健康轨道 + 精准断联事件针（带防重叠时刻胶囊）+ 真实时钟标尺 + 双向悬停联动。
#[allow(clippy::too_many_arguments)]
fn draw_health_ribbon(
    ui: &mut egui::Ui,
    p: &Palette,
    segments: &[TrackSegment],
    outages: &[OutageEvent<'_>],
    now: i64,
    range: TimeRange,
    linked_idx: Option<usize>,
    wide: bool,
) -> Option<usize> {
    let total_w = ui.available_width();
    let window_secs = range.secs().max(60);
    let window_start = now - window_secs;

    let pin_h = if wide { 20.0_f32 } else { 18.0_f32 };
    let track_h = if wide { 12.0_f32 } else { 11.0_f32 };
    let ruler_gap = 4.0_f32;
    let ruler_h = 17.0_f32;
    let total_h = pin_h + track_h + ruler_gap + ruler_h;

    let (full_rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, total_h), Sense::hover());
    let track_rect = Rect::from_min_size(
        Pos2::new(full_rect.left(), full_rect.top() + pin_h),
        Vec2::new(total_w, track_h),
    );
    let ruler_top = track_rect.bottom() + ruler_gap;
    let painter = ui.painter();

    let ts_to_x = |ts: i64| -> f32 {
        let frac = ((ts - window_start) as f32 / window_secs as f32).clamp(0.0, 1.0);
        track_rect.left() + frac * track_rect.width()
    };

    // 1. 轨道底色与各连续状态段（在线翡翠绿 / 休眠未运行轻斜纹 / 离线警示色）
    let track_radius = CornerRadius::same((track_h * 0.5).round() as u8);
    painter.rect_filled(track_rect, track_radius, p.subtle);

    let online_fill = Color32::from_rgb(0x10, 0xB9, 0x81);
    let inner_track = track_rect.shrink(1.0);
    for seg in segments {
        let x0 = ts_to_x(seg.start_ts).max(inner_track.left());
        let x1 = ts_to_x(seg.end_ts).min(inner_track.right());
        if x1 - x0 < 0.5 {
            continue;
        }
        let seg_rect = Rect::from_min_max(
            Pos2::new(x0, inner_track.top()),
            Pos2::new(x1, inner_track.bottom()),
        );
        let seg_r = CornerRadius::same(3);
        match seg.state {
            TrackState::Online => {
                painter.rect_filled(seg_rect, seg_r, with_alpha(online_fill, 225));
                // 顶部极淡光泽发丝线，提升轨道层次感
                painter.line_segment(
                    [
                        Pos2::new(seg_rect.left() + 1.0, seg_rect.top() + 1.0),
                        Pos2::new(seg_rect.right() - 1.0, seg_rect.top() + 1.0),
                    ],
                    Stroke::new(1.0_f32, with_alpha(Color32::WHITE, 45)),
                );
            }
            TrackState::Offline => {
                painter.rect_filled(seg_rect, seg_r, with_alpha(p.warn, 220));
            }
            TrackState::NoData => {
                // 未运行/休眠区：轻盈 45° 斜纹，视觉退后
                let clip_p = painter.with_clip_rect(seg_rect);
                let hatch_color = with_alpha(p.text_muted, 36);
                let step = 6.0_f32;
                let h = seg_rect.height();
                let mut sx = seg_rect.left() - h;
                while sx < seg_rect.right() + h {
                    clip_p.line_segment(
                        [
                            Pos2::new(sx, seg_rect.bottom()),
                            Pos2::new(sx + h, seg_rect.top()),
                        ],
                        Stroke::new(1.1_f32, hatch_color),
                    );
                    sx += step;
                }
            }
        }
    }

    // 轨道外描边（收口干净）
    painter.rect_stroke(
        track_rect,
        track_radius,
        Stroke::new(1.0_f32, with_alpha(p.border_strong, 145)),
        StrokeKind::Inside,
    );

    // 2. 精准断联事件针与轨道高亮细条
    let mut hovered_pin: Option<usize> = None;
    let pointer_pos = resp.hover_pos();
    if let Some(pos) = pointer_pos {
        if pos.y >= full_rect.top() && pos.y <= track_rect.bottom() + 2.0 {
            let mut best: Option<(usize, f32)> = None;
            for (idx, ev) in outages.iter().enumerate() {
                let x = ts_to_x(ev.start.ts);
                let dist = (pos.x - x).abs();
                if dist <= 10.0 && best.is_none_or(|(_, d)| dist < d) {
                    best = Some((idx, dist));
                }
            }
            hovered_pin = best.map(|(i, _)| i);
        }
    }
    let active_pin = hovered_pin.or(linked_idx);

    let mut placed_pills: Vec<(f32, f32)> = Vec::new();
    let show_static_pills = outages.len() <= 3;

    // 先绘制非高亮事件针，再把激活事件针叠在最顶层
    for pass in 0..2 {
        for (idx, ev) in outages.iter().enumerate() {
            let is_active = active_pin == Some(idx);
            if (pass == 0 && is_active) || (pass == 1 && !is_active) {
                continue;
            }
            let pin_color = if !ev.recovered {
                p.danger
            } else {
                reason_severity_color(p, &ev.start.reason)
            };
            let x = ts_to_x(ev.start.ts);
            let dur_w = ((ev.duration_secs.unwrap_or(1) as f32 / window_secs as f32)
                * track_rect.width())
            .max(3.0);
            let sliver_x = x.clamp(
                inner_track.left(),
                (inner_track.right() - dur_w).max(inner_track.left()),
            );
            let sliver = Rect::from_min_size(
                Pos2::new(sliver_x, inner_track.top()),
                Vec2::new(dur_w, inner_track.height()),
            );
            painter.rect_filled(sliver, CornerRadius::same(1), pin_color);

            let stem_x = sliver.center().x;
            let stem_top = full_rect.top() + if is_active { 14.0 } else { 15.0 };
            painter.line_segment(
                [
                    Pos2::new(stem_x, stem_top),
                    Pos2::new(stem_x, track_rect.bottom() + 1.0),
                ],
                Stroke::new(if is_active { 2.4_f32 } else { 1.8_f32 }, pin_color),
            );
            if is_active {
                painter.circle_filled(
                    Pos2::new(stem_x, track_rect.center().y),
                    6.5,
                    with_alpha(pin_color, 55),
                );
            }
            painter.circle_filled(
                Pos2::new(stem_x, track_rect.bottom() + 0.5),
                if is_active { 3.2 } else { 2.2 },
                pin_color,
            );

            // 时刻胶囊标签（激活态必显；常规最多 3 条且自动防重叠）
            if is_active || show_static_pills {
                let tag = match ev.duration_secs {
                    Some(secs) if ev.recovered => {
                        format!(
                            "{} · {}",
                            format_hm(ev.start.ts),
                            format_duration_short(secs)
                        )
                    }
                    _ => format!("{} · 离线", format_hm(ev.start.ts)),
                };
                let font = egui::FontId::new(9.5, crate::ui::theme::bold_family());
                let galley = painter.layout_no_wrap(tag, font, pin_color);
                let pill_w = galley.size().x + 10.0;
                let pill_h = 15.0_f32;
                let pill_x = (stem_x - pill_w * 0.5).clamp(
                    track_rect.left(),
                    (track_rect.right() - pill_w).max(track_rect.left()),
                );
                let overlaps = placed_pills
                    .iter()
                    .any(|(l, r)| pill_x < *r + 4.0 && pill_x + pill_w > *l - 4.0);
                if is_active || !overlaps {
                    let pill_rect = Rect::from_min_size(
                        Pos2::new(pill_x, full_rect.top()),
                        Vec2::new(pill_w, pill_h),
                    );
                    let (bg, border) = if pin_color == p.danger {
                        (p.danger_soft, p.danger_border)
                    } else {
                        (p.warn_soft, p.warn_border)
                    };
                    painter.rect(
                        pill_rect,
                        CornerRadius::same(4),
                        bg,
                        Stroke::new(if is_active { 1.2_f32 } else { 0.9_f32 }, border),
                        StrokeKind::Inside,
                    );
                    painter.galley(
                        Pos2::new(
                            pill_rect.center().x - galley.size().x * 0.5,
                            pill_rect.center().y - galley.size().y * 0.5,
                        ),
                        galley,
                        pin_color,
                    );
                    placed_pills.push((pill_x, pill_x + pill_w));
                }
            }
        }
    }

    // 3. 悬停交互：命中事件针显示断联详情；命中轨道空白处显示纵线时间游标与瞬时状态
    if let Some(idx) = hovered_pin {
        if let Some(ev) = outages.get(idx) {
            egui::show_tooltip(
                ui.ctx(),
                ui.layer_id(),
                ui.id().with("ribbon_pin_tip"),
                |ui| {
                    ui.set_max_width(230.0);
                    ui.label(bold_text(
                        format!(
                            "{} · {}",
                            ev.start.local_md_hm(),
                            reason_label(&ev.start.reason)
                        ),
                        11.5,
                        p.text,
                    ));
                    let status = match (ev.recovered, ev.auto_reconnected, ev.duration_secs) {
                        (true, true, Some(s)) => {
                            format!("断联 {} · 守护已自动重连", format_duration_short(s))
                        }
                        (true, false, Some(s)) => {
                            format!("断联 {} · 网络已恢复", format_duration_short(s))
                        }
                        (false, _, Some(s)) => {
                            format!("已持续离线 {}", format_duration_short(s))
                        }
                        _ => "断联事件".to_string(),
                    };
                    ui.label(body_text(status, 10.8, p.text_secondary));
                    ui.label(body_text(reason_hint(&ev.start.reason), 10.2, p.text_muted));
                },
            );
        }
    } else if let Some(pos) = pointer_pos {
        if pos.y >= track_rect.top() - 3.0 && pos.y <= track_rect.bottom() + 3.0 {
            let cx = pos.x.clamp(track_rect.left(), track_rect.right());
            painter.line_segment(
                [
                    Pos2::new(cx, track_rect.top() - 2.0),
                    Pos2::new(cx, track_rect.bottom() + 2.0),
                ],
                Stroke::new(1.2_f32, with_alpha(p.text, 185)),
            );
            let frac = ((cx - track_rect.left()) / track_rect.width().max(1.0)).clamp(0.0, 1.0);
            let hover_ts = window_start + (frac * window_secs as f32).round() as i64;
            let state = segments
                .iter()
                .find(|s| hover_ts >= s.start_ts && hover_ts <= s.end_ts)
                .map(|s| s.state)
                .unwrap_or(TrackState::NoData);
            let (st_text, st_color) = match state {
                TrackState::Online => ("校园网在线", p.ok),
                TrackState::Offline => ("网络连接中断", p.warn),
                TrackState::NoData => ("守护未运行或电脑休眠", p.text_muted),
            };
            egui::show_tooltip(
                ui.ctx(),
                ui.layer_id(),
                ui.id().with("ribbon_cursor_tip"),
                |ui| {
                    ui.label(bold_text(format_hm(hover_ts), 11.5, p.text));
                    ui.label(body_text(st_text, 10.8, st_color));
                },
            );
        }
    }

    // 4. 真实时钟标尺（顶部细基线 + 自然整点短刻度线 + 真实 HH:MM 标签）
    painter.line_segment(
        [
            Pos2::new(track_rect.left(), ruler_top),
            Pos2::new(track_rect.right(), ruler_top),
        ],
        Stroke::new(1.0_f32, p.divider),
    );
    let tick_stroke = Stroke::new(1.0_f32, with_alpha(p.text_muted, 130));
    let tick_font = egui::FontId::proportional(9.5);
    let label_y = ruler_top + 11.5;

    // 左端起点时刻
    painter.line_segment(
        [
            Pos2::new(track_rect.left(), ruler_top),
            Pos2::new(track_rect.left(), ruler_top + 4.0),
        ],
        tick_stroke,
    );
    painter.text(
        Pos2::new(track_rect.left(), label_y),
        egui::Align2::LEFT_CENTER,
        format_hm(window_start),
        tick_font.clone(),
        p.text_muted,
    );

    // 右端「现在」
    painter.line_segment(
        [
            Pos2::new(track_rect.right(), ruler_top),
            Pos2::new(track_rect.right(), ruler_top + 4.0),
        ],
        tick_stroke,
    );
    painter.text(
        Pos2::new(track_rect.right(), label_y),
        egui::Align2::RIGHT_CENTER,
        "现在",
        egui::FontId::new(9.5, crate::ui::theme::bold_family()),
        p.text_secondary,
    );

    // 中间自然整点刻度（24h 每 6 小时 / 7h 每 2 小时 / 1h 每 15 分钟），自动防碰撞
    let step_secs: i64 = match range {
        TimeRange::H1 => 15 * 60,
        TimeRange::H7 => 2 * 3600,
        TimeRange::H24 => 6 * 3600,
    };
    let mut last_tick_x = track_rect.left() + 42.0;
    let right_limit = track_rect.right() - 40.0;
    for tick_ts in aligned_clock_ticks(window_start, now, step_secs) {
        let tx = ts_to_x(tick_ts);
        if tx < last_tick_x || tx > right_limit {
            continue;
        }
        painter.line_segment(
            [Pos2::new(tx, ruler_top), Pos2::new(tx, ruler_top + 4.0)],
            tick_stroke,
        );
        painter.text(
            Pos2::new(tx, label_y),
            egui::Align2::CENTER_CENTER,
            format_hm(tick_ts),
            tick_font.clone(),
            p.text_muted,
        );
        last_tick_x = tx + 44.0;
    }

    hovered_pin
}

fn aligned_clock_ticks(start_ts: i64, end_ts: i64, step_secs: i64) -> Vec<i64> {
    let mut out = Vec::new();
    let Some(start_dt) = Local.timestamp_opt(start_ts, 0).single() else {
        return out;
    };
    let secs_since_midnight =
        (start_dt.hour() * 3600 + start_dt.minute() * 60 + start_dt.second()) as i64;
    let rem = secs_since_midnight % step_secs;
    let mut first_tick = if rem == 0 {
        start_ts + step_secs
    } else {
        start_ts + (step_secs - rem)
    };
    while first_tick < end_ts {
        out.push(first_tick);
        first_tick += step_secs;
    }
    out
}

fn draw_outage_event_row(
    ui: &mut egui::Ui,
    p: &Palette,
    ev: &OutageEvent<'_>,
    highlighted: bool,
) -> bool {
    let dot_color = if !ev.recovered
        || matches!(
            ev.start.reason.as_str(),
            "LINK_DOWN" | "NO_IP" | "NO_GATEWAY" | "GATEWAY_UNREACHABLE" | "LOGIN_REJECTED"
        ) {
        p.danger
    } else {
        p.warn
    };

    let (fill, stroke) = if highlighted {
        (p.card_hover, Stroke::new(1.1_f32, p.accent_border))
    } else {
        (p.subtle, Stroke::new(1.0_f32, p.divider))
    };

    let frame_resp = egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(9, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    let (dot_r, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                    ui.painter().circle_filled(dot_r.center(), 3.2, dot_color);
                    ui.label(body_text(ev.start.local_md_hm(), 10.5, p.text_muted));
                    ui.label(bold_text(reason_label(&ev.start.reason), 11.5, p.text));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if ev.recovered {
                            let tag = if ev.auto_reconnected {
                                "已重连"
                            } else {
                                "已恢复"
                            };
                            badge(ui, p.ok, p.ok_soft, p.ok_border, tag);
                        } else {
                            badge(ui, p.danger, p.danger_soft, p.danger_border, "离线中");
                        }
                        if let Some(secs) = ev.duration_secs {
                            let prefix = if ev.recovered {
                                "断联 "
                            } else {
                                "已离线 "
                            };
                            ui.label(body_text(
                                format!("{prefix}{}", format_duration_short(secs)),
                                10.5,
                                p.text_muted,
                            ));
                        }
                    });
                },
            );
        });

    let hovered = frame_resp.response.hovered();
    frame_resp
        .response
        .on_hover_text(reason_hint(&ev.start.reason));
    hovered
}

/// 右侧 38% 辅卡：双模式自适应环形流量图 + 近 7 天比例柱状图。
pub fn draw_traffic_card(
    ui: &mut egui::Ui,
    p: &Palette,
    traffic: &TrafficSummary,
    period: &mut TrafficPeriod,
    monthly_quota_gb: i64,
    target_height: f32,
) {
    card_padded(ui, p, 12, |ui| {
        let inner_h = (target_height - 24.0).max(196.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);

        // 1. 顶部标题 + 今日/本月 切换
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                section_heading(ui, Icon::ArrowUpDown, p.accent, "本机流量", p.text, 26.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut idx = period.index();
                    let labels = ["今日", "本月"];
                    if segmented_control(ui, p, "traffic_period_seg", &mut idx, &labels) {
                        *period = TrafficPeriod::from_index(idx);
                    }
                });
            },
        );

        ui.add_space(8.0);

        // 2. 上半区：双模式环形图 + 紧凑 3 行图例
        let (rx, tx) = match *period {
            TrafficPeriod::Today => (traffic.today_rx_bytes, traffic.today_tx_bytes),
            TrafficPeriod::Month => (traffic.month_rx_bytes, traffic.month_tx_bytes),
        };
        let total = rx.saturating_add(tx);
        let month_total = traffic.month_total_bytes();
        let today_total = traffic.today_total_bytes();
        let quota_bytes = (monthly_quota_gb.max(0) as u64).saturating_mul(1024 * 1024 * 1024);
        let use_quota_ring = quota_bytes > 0 && *period == TrafficPeriod::Month;

        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 78.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;

                // 左侧 76×76 自绘抗锯齿环形图
                let (ring_rect, ring_resp) =
                    ui.allocate_exact_size(Vec2::splat(76.0), Sense::hover());
                let center = ring_rect.center();
                let radius = 31.0_f32;
                let thickness = 7.5_f32;
                let painter = ui.painter();

                painter.circle_stroke(center, radius, Stroke::new(thickness, p.divider));

                if use_quota_ring {
                    let ratio = (month_total as f32 / quota_bytes as f32).clamp(0.0, 1.0);
                    let arc_color = if ratio >= 0.95 {
                        p.danger
                    } else if ratio >= 0.80 {
                        p.warn
                    } else {
                        p.accent
                    };
                    if ratio > 0.005 {
                        paint_ring_arc(
                            painter,
                            center,
                            radius,
                            -PI * 0.5,
                            -PI * 0.5 + ratio * 2.0 * PI,
                            Stroke::new(thickness, arc_color),
                        );
                    }
                    let pct_str = format!("{:.0}%", (ratio * 100.0).min(999.0));
                    painter.text(
                        Pos2::new(center.x, center.y - 6.0),
                        egui::Align2::CENTER_CENTER,
                        pct_str,
                        egui::FontId::new(13.5, crate::ui::theme::bold_family()),
                        p.text,
                    );
                    painter.text(
                        Pos2::new(center.x, center.y + 9.0),
                        egui::Align2::CENTER_CENTER,
                        "本月额度",
                        egui::FontId::proportional(9.5),
                        p.text_muted,
                    );
                } else {
                    if total > 0 {
                        let rx_frac = (rx as f32 / total as f32).clamp(0.0, 1.0);
                        let tx_frac = 1.0 - rx_frac;
                        let gap_rad = if rx > 0 && tx > 0 { 0.06_f32 } else { 0.0 };
                        let start = -PI * 0.5;
                        if rx_frac > 0.005 {
                            let sweep = (rx_frac * 2.0 * PI - gap_rad).max(0.04);
                            paint_ring_arc(
                                painter,
                                center,
                                radius,
                                start,
                                start + sweep,
                                Stroke::new(thickness, p.accent),
                            );
                        }
                        if tx_frac > 0.005 {
                            let tx_start = start + rx_frac * 2.0 * PI + gap_rad * 0.5;
                            let sweep = (tx_frac * 2.0 * PI - gap_rad).max(0.04);
                            paint_ring_arc(
                                painter,
                                center,
                                radius,
                                tx_start,
                                tx_start + sweep,
                                Stroke::new(thickness, p.ok),
                            );
                        }
                    }
                    painter.text(
                        Pos2::new(center.x, center.y - 6.0),
                        egui::Align2::CENTER_CENTER,
                        format_bytes_compact(total),
                        egui::FontId::new(12.5, crate::ui::theme::bold_family()),
                        p.text,
                    );
                    painter.text(
                        Pos2::new(center.x, center.y + 9.0),
                        egui::Align2::CENTER_CENTER,
                        match *period {
                            TrafficPeriod::Today => "今日合计",
                            TrafficPeriod::Month => "本月合计",
                        },
                        egui::FontId::proportional(9.5),
                        p.text_muted,
                    );
                }

                ring_resp.on_hover_text(
                    "统计校园网在线期间本机物理网卡流量，已自动排除 Clash/Mihomo TUN 虚拟网卡",
                );

                // 右侧 3 行图例
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 5.0;
                    if use_quota_ring {
                        legend_kv_row(
                            ui,
                            p,
                            Some(p.accent),
                            "本月已用",
                            &format!(
                                "{} / {}G",
                                format_bytes_compact(month_total),
                                monthly_quota_gb
                            ),
                        );
                        legend_kv_row(ui, p, None, "今日用量", &format_bytes_compact(today_total));
                        let sep = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
                        ui.painter().line_segment(
                            [sep.left_center(), sep.right_center()],
                            Stroke::new(1.0_f32, p.divider),
                        );
                        let rem = quota_bytes.saturating_sub(month_total);
                        let rem_lbl = if month_total > quota_bytes {
                            "已超出上限".to_string()
                        } else {
                            format!("余 {}", format_bytes_compact(rem))
                        };
                        legend_kv_row(ui, p, None, "剩余参考", &rem_lbl);
                    } else {
                        legend_kv_row(ui, p, Some(p.accent), "下载", &format_bytes_compact(rx));
                        legend_kv_row(ui, p, Some(p.ok), "上传", &format_bytes_compact(tx));
                        let sep = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
                        ui.painter().line_segment(
                            [sep.left_center(), sep.right_center()],
                            Stroke::new(1.0_f32, p.divider),
                        );
                        match *period {
                            TrafficPeriod::Today => legend_kv_row(
                                ui,
                                p,
                                None,
                                "本月累计",
                                &format_bytes_compact(month_total),
                            ),
                            TrafficPeriod::Month => legend_kv_row(
                                ui,
                                p,
                                None,
                                "今日用量",
                                &format_bytes_compact(today_total),
                            ),
                        }
                    }
                });
            },
        );

        ui.add_space(9.0);
        let sep = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
        ui.painter().line_segment(
            [sep.left_center(), sep.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );
        ui.add_space(6.0);

        // 3. 下半区：近 7 天比例柱状图
        let chart_h = (ui.available_height() - 1.0).max(66.0);
        draw_7day_bars(ui, p, &traffic.last_7_days, chart_h);
    });
}

fn legend_kv_row(ui: &mut egui::Ui, p: &Palette, dot: Option<Color32>, label: &str, value: &str) {
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 17.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            if let Some(color) = dot {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.2, color);
            }
            ui.label(body_text(label, 11.0, p.text_secondary));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(bold_text(value, 11.5, p.text));
            });
        },
    );
}

fn draw_7day_bars(ui: &mut egui::Ui, p: &Palette, days: &[DailyTraffic], height: f32) {
    let today = Local::now().date_naive();
    let fallback: Vec<DailyTraffic> = if days.len() == 7 {
        days.to_vec()
    } else {
        (0..7)
            .rev()
            .map(|offset| {
                let d = today - chrono::Duration::days(offset);
                DailyTraffic {
                    date: d.format("%Y-%m-%d").to_string(),
                    rx_bytes: 0,
                    tx_bytes: 0,
                }
            })
            .collect()
    };

    let max_bytes = fallback
        .iter()
        .map(DailyTraffic::total_bytes)
        .max()
        .unwrap_or(0)
        .max(1);

    let total_w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total_w, height), Sense::hover());
    let col_w = total_w / 7.0;
    let top_lbl_h = 13.0_f32;
    let bot_lbl_h = 14.0_f32;
    let track_top = rect.top() + top_lbl_h + 2.0;
    let track_bot = rect.bottom() - bot_lbl_h - 2.0;
    let track_h = (track_bot - track_top).max(16.0);
    let bar_w = (col_w * 0.52).clamp(10.0, 20.0);

    let mut hovered: Option<&DailyTraffic> = None;

    for (idx, day) in fallback.iter().enumerate() {
        let is_today = idx == 6;
        let col_rect = Rect::from_min_max(
            Pos2::new(rect.left() + idx as f32 * col_w, rect.top()),
            Pos2::new(rect.left() + (idx + 1) as f32 * col_w, rect.bottom()),
        );
        let resp = ui.interact(col_rect, ui.id().with(("traffic_bar", idx)), Sense::hover());
        let total = day.total_bytes();
        let frac = if total == 0 {
            0.0
        } else {
            (total as f32 / max_bytes as f32).clamp(0.08, 1.0)
        };
        let bh = if total == 0 {
            3.0
        } else {
            (track_h * frac).max(5.0)
        };
        let bar_rect = Rect::from_min_max(
            Pos2::new(col_rect.center().x - bar_w * 0.5, track_bot - bh),
            Pos2::new(col_rect.center().x + bar_w * 0.5, track_bot),
        );

        let fill = if total == 0 {
            with_alpha(p.border_strong, 95)
        } else if is_today || resp.hovered() {
            p.accent
        } else {
            with_alpha(p.accent, 135)
        };
        if resp.hovered() {
            hovered = Some(day);
        }

        ui.painter().rect_filled(
            bar_rect,
            CornerRadius {
                nw: 3,
                ne: 3,
                sw: 1,
                se: 1,
            },
            fill,
        );

        if total > 0 {
            ui.painter().text(
                Pos2::new(
                    col_rect.center().x,
                    (bar_rect.top() - 6.0).max(rect.top() + 6.0),
                ),
                egui::Align2::CENTER_CENTER,
                format_bytes_bar(total),
                egui::FontId::proportional(8.8),
                if is_today {
                    p.text_secondary
                } else {
                    p.text_muted
                },
            );
        }

        let lbl = if is_today {
            "今"
        } else {
            weekday_short(&day.date)
        };
        ui.painter().text(
            Pos2::new(col_rect.center().x, rect.bottom() - 6.0),
            egui::Align2::CENTER_CENTER,
            lbl,
            egui::FontId::proportional(9.5),
            if is_today { p.accent } else { p.text_muted },
        );
    }

    if let Some(day) = hovered {
        egui::show_tooltip(
            ui.ctx(),
            ui.layer_id(),
            ui.id().with("traffic_bar_tip"),
            |ui| {
                ui.label(bold_text(&day.date, 11.5, p.text));
                ui.label(body_text(
                    format!("合计 {}", format_bytes_compact(day.total_bytes())),
                    11.0,
                    p.text_secondary,
                ));
                ui.label(body_text(
                    format!(
                        "下载 {} · 上传 {}",
                        format_bytes_compact(day.rx_bytes),
                        format_bytes_compact(day.tx_bytes)
                    ),
                    10.5,
                    p.text_muted,
                ));
            },
        );
    }
}

fn paint_ring_arc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    start_angle: f32,
    end_angle: f32,
    stroke: Stroke,
) {
    let span = (end_angle - start_angle).abs();
    let steps = ((span / (2.0 * PI)) * 64.0).ceil().clamp(6.0, 64.0) as usize;
    let pts: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let a = start_angle + (end_angle - start_angle) * (i as f32 / steps as f32);
            Pos2::new(center.x + radius * a.cos(), center.y + radius * a.sin())
        })
        .collect();
    painter.add(egui::Shape::line(pts, stroke));
}

fn format_hm(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".to_string())
}

fn weekday_short(date: &str) -> &'static str {
    let Ok(parsed) = NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
        return "·";
    };
    match parsed.weekday() {
        Weekday::Mon => "一",
        Weekday::Tue => "二",
        Weekday::Wed => "三",
        Weekday::Thu => "四",
        Weekday::Fri => "五",
        Weekday::Sat => "六",
        Weekday::Sun => "日",
    }
}

pub fn format_duration_short(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s:02}s")
        }
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn format_bytes_compact(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        if b >= 100.0 * MB {
            format!("{:.0} MB", b / MB)
        } else {
            format!("{:.1} MB", b / MB)
        }
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn format_bytes_bar(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1}G", b / GB)
    } else if b >= MB {
        format!("{:.0}M", b / MB)
    } else {
        "<1M".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes_and_durations_compactly() {
        assert_eq!(format_bytes_compact(0), "0 B");
        assert_eq!(format_bytes_compact(1536), "2 KB");
        assert_eq!(format_bytes_compact(25 * 1024 * 1024), "25.0 MB");
        assert_eq!(format_bytes_compact(256 * 1024 * 1024), "256 MB");
        assert_eq!(
            format_bytes_compact((2.4 * 1024.0 * 1024.0 * 1024.0) as u64),
            "2.4 GB"
        );
        assert_eq!(format_duration_short(24), "24s");
        assert_eq!(format_duration_short(72), "1m 12s");
        assert_eq!(format_duration_short(3720), "1h 02m");
    }
}
