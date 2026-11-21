//! 主页可视化核心卡片：
//! - 左侧主卡（~62%）：连接稳定性与可上下滚动的断联事件明细（36 格圆角健康时间轴 + 悬停明细）
//! - 右侧辅卡（~38%）：本机流量统计（自适应双模式环形图 + 近 7 天比例柱状图）

use std::f32::consts::PI;

use chrono::{Datelike, Local, NaiveDate, TimeZone, Weekday};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::metrics_store::{
    compute_stats, filter_range, outages_in_range, uptime_timeline, OutageEvent, SlotStatus,
    TelemetryPoint, TimeRange, TimelineSlot,
};
use crate::models::{DailyTraffic, TrafficSummary};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::{
    body_text, bold_text, reason_hint, reason_label, with_alpha, Palette, ThemeMode,
};
use crate::ui::widgets::{badge, card_padded, section_heading, segmented_control};

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
    target_height: f32,
) {
    if !show_traffic {
        draw_stability_wide_card(ui, p, all_points, range, target_height);
        return;
    }

    let total_w = ui.available_width();
    let gap = 10.0_f32;
    let left_w = ((total_w - gap) * 0.615).floor().max(320.0);
    let right_w = (total_w - gap - left_w).max(220.0);

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.allocate_ui_with_layout(
            Vec2::new(left_w, target_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                draw_stability_card(ui, p, all_points, range, target_height);
            },
        );
        ui.add_space(gap);
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
}

/// 关闭「本机流量」时的通栏宽屏版稳定性面板：
/// - 左侧 ~51%：自绘在线率环形图 + 健康结论/主因分布 + 饱满 36 格通断时间轴
/// - 右侧 ~49%：独占整卡高度的可上下滚动断联事件明细（同屏展示 4–5 条）
pub fn draw_stability_wide_card(
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
    let slots = uptime_timeline(&points, now, *range, 36);
    let reasons = outages.reason_counts();

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
                    if segmented_control(ui, p, "stab_wide_range_seg", &mut idx, &labels) {
                        *range = TimeRange::from_index(idx);
                    }
                });
            },
        );

        ui.add_space(10.0);

        let body_h = (inner_h - 36.0).max(148.0);
        let total_w = ui.available_width();
        let sep_total = 25.0_f32; // 12 + 1 + 12
        let left_w = ((total_w - sep_total) * 0.515).floor().max(260.0);
        let right_w = (total_w - left_w - sep_total).max(220.0);

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;

            // ── 左侧视觉概览：在线率环形图 + 原因构成 + 36 格健康时间轴 ──
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, body_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.set_min_size(Vec2::new(left_w, body_h));

                    // 上半：环形图 + 结构化摘要
                    ui.allocate_ui_with_layout(
                        Vec2::new(left_w, 84.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 14.0;

                            // 80×80 自绘在线率环形图
                            let (ring_rect, ring_resp) =
                                ui.allocate_exact_size(Vec2::splat(80.0), Sense::hover());
                            let center = ring_rect.center();
                            let radius = 33.0_f32;
                            let thickness = 8.0_f32;
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
                                let rate_text = if (pct - 100.0).abs() < 0.05 {
                                    "100%".to_string()
                                } else {
                                    format!("{pct:.1}%")
                                };
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

                    ui.add_space(12.0);
                    let hsep = ui.allocate_space(Vec2::new(left_w, 1.0)).1;
                    ui.painter().line_segment(
                        [hsep.left_center(), hsep.right_center()],
                        Stroke::new(1.0_f32, p.divider),
                    );
                    ui.add_space(10.0);

                    // 下半：通断健康时间轴
                    ui.allocate_ui_with_layout(
                        Vec2::new(left_w, 16.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(bold_text("通断健康时间轴", 11.0, p.text_secondary));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(body_text("悬停色条查看时段", 10.0, p.text_muted));
                                },
                            );
                        },
                    );
                    ui.add_space(7.0);
                    draw_uptime_bars_sized(ui, p, &slots, *range, 26.0);
                },
            );

            // 中央 1px 竖向分割线
            ui.add_space(12.0);
            let vsep = ui.allocate_space(Vec2::new(1.0, body_h)).1;
            ui.painter().line_segment(
                [vsep.center_top(), vsep.center_bottom()],
                Stroke::new(1.0_f32, p.divider),
            );
            ui.add_space(12.0);

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
                    ui.add_space(6.0);

                    let list_h = (body_h - 22.0).max(80.0);
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
                                        for ev in outages.events.iter().rev() {
                                            draw_outage_event_row(ui, p, ev);
                                        }
                                    });
                            },
                        );
                    }
                },
            );
        });
    });
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

/// 左侧 62% 主卡：在线率 + 主因摘要 + 36 格通断健康时间轴 + 可上下滚动断联事件列表。
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
    let slots = uptime_timeline(&points, now, *range, 36);

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

        ui.add_space(6.0);

        // 2. 在线率大字 + 一句话主因/稳定摘要
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 26.0),
            egui::Layout::left_to_right(egui::Align::BOTTOM),
            |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if let Some(pct) = stats.uptime_pct {
                    let rate_color = if pct >= 98.0 {
                        p.text
                    } else if pct >= 90.0 {
                        p.warn
                    } else {
                        p.danger
                    };
                    let rate_text = if (pct - 100.0).abs() < 0.05 {
                        "100%".to_string()
                    } else {
                        format!("{pct:.1}%")
                    };
                    ui.label(bold_text(rate_text, 22.0, rate_color));
                    ui.label(body_text("在线率", 11.0, p.text_muted));
                } else {
                    ui.label(bold_text("—", 22.0, p.text_muted));
                    ui.label(body_text("等待巡检采样", 11.0, p.text_muted));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(primary) = outages.primary_reason() {
                        ui.label(body_text(
                            format!("主因：{} · 悬停色条看时段", reason_label(primary)),
                            11.0,
                            p.text_secondary,
                        ));
                    } else if stats.check_count > 0 {
                        ui.label(body_text("当前时段全程稳定，无断联", 11.0, p.ok));
                    } else {
                        ui.label(body_text("开启监控后自动记录", 11.0, p.text_muted));
                    }
                });
            },
        );

        ui.add_space(7.0);

        // 3. 36 格饱满圆角通断健康时间轴
        draw_uptime_bars(ui, p, &slots, *range);

        ui.add_space(7.0);

        // 4. 分隔线 + 断联事件列表标题
        let sep = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
        ui.painter().line_segment(
            [sep.left_center(), sep.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );
        ui.add_space(5.0);

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
                        ui.label(body_text("按时间倒序", 10.0, p.text_muted));
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
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 5.0;
                            for ev in outages.events.iter().rev() {
                                draw_outage_event_row(ui, p, ev);
                            }
                        });
                },
            );
        }
    });
}

fn draw_uptime_bars(ui: &mut egui::Ui, p: &Palette, slots: &[TimelineSlot], range: TimeRange) {
    draw_uptime_bars_sized(ui, p, slots, range, 22.0);
}

fn draw_uptime_bars_sized(
    ui: &mut egui::Ui,
    p: &Palette,
    slots: &[TimelineSlot],
    range: TimeRange,
    bar_h: f32,
) {
    let total_w = ui.available_width();
    let gap = 2.5_f32;
    let n = slots.len().max(1);
    let slot_w = ((total_w - gap * (n as f32 - 1.0)) / n as f32).max(2.0);

    let (rect, _) = ui.allocate_exact_size(Vec2::new(total_w, bar_h), Sense::hover());
    let mut hovered_slot: Option<&TimelineSlot> = None;

    for (idx, slot) in slots.iter().enumerate() {
        let x0 = rect.left() + idx as f32 * (slot_w + gap);
        let cell = Rect::from_min_size(Pos2::new(x0, rect.top()), Vec2::new(slot_w, bar_h));
        let resp = ui.interact(cell, ui.id().with(("uptime_slot", idx)), Sense::hover());
        let base_color = match slot.status {
            SlotStatus::Online => match p.mode {
                ThemeMode::Light => Color32::from_rgb(0x10, 0xB9, 0x81),
                ThemeMode::Dark => Color32::from_rgb(0x10, 0xB9, 0x81),
            },
            SlotStatus::Degraded => p.warn,
            SlotStatus::Offline => p.danger,
            SlotStatus::NoData => with_alpha(p.border_strong, 95),
        };
        let fill = if resp.hovered() {
            hovered_slot = Some(slot);
            base_color
        } else if slot.status == SlotStatus::Online {
            with_alpha(base_color, 215)
        } else {
            base_color
        };
        ui.painter().rect_filled(cell, CornerRadius::same(3), fill);
        if resp.hovered() {
            ui.painter().rect_stroke(
                cell,
                CornerRadius::same(3),
                Stroke::new(1.2_f32, p.text),
                StrokeKind::Outside,
            );
        }
    }

    if let Some(slot) = hovered_slot {
        let from = format_hm(slot.start_ts);
        let to = format_hm(slot.end_ts);
        let status_str = match slot.status {
            SlotStatus::Online => "连接稳定",
            SlotStatus::Degraded => "曾发生断联并恢复",
            SlotStatus::Offline => "持续离线",
            SlotStatus::NoData => "暂无采样",
        };
        egui::show_tooltip(
            ui.ctx(),
            ui.layer_id(),
            ui.id().with("uptime_slot_tip"),
            |ui| {
                ui.set_max_width(220.0);
                ui.label(bold_text(format!("{from} – {to}"), 11.5, p.text));
                ui.label(body_text(status_str, 11.0, p.text_secondary));
                if slot.checks > 0 {
                    let mut detail = format!("在线 {}/{} 次", slot.online_checks, slot.checks);
                    if let Some(ms) = slot.avg_ms {
                        detail.push_str(&format!(" · 平均网关 {ms} ms"));
                    }
                    ui.label(body_text(detail, 10.5, p.text_muted));
                }
            },
        );
    }

    // 底部时间刻度（左 / 中 / 右）
    ui.add_space(3.0);
    let (left_lbl, mid_lbl) = match range {
        TimeRange::H1 => ("1 小时前", "30 分钟前"),
        TimeRange::H7 => ("7 小时前", "3.5 小时前"),
        TimeRange::H24 => ("24 小时前", "12 小时前"),
    };
    let (axis_rect, _) = ui.allocate_exact_size(Vec2::new(total_w, 12.0), Sense::hover());
    let font = egui::FontId::proportional(9.5);
    ui.painter().text(
        axis_rect.left_center(),
        egui::Align2::LEFT_CENTER,
        left_lbl,
        font.clone(),
        p.text_muted,
    );
    ui.painter().text(
        axis_rect.center(),
        egui::Align2::CENTER_CENTER,
        mid_lbl,
        font.clone(),
        p.text_muted,
    );
    ui.painter().text(
        axis_rect.right_center(),
        egui::Align2::RIGHT_CENTER,
        "现在",
        font,
        p.text_muted,
    );
}

fn draw_outage_event_row(ui: &mut egui::Ui, p: &Palette, ev: &OutageEvent<'_>) {
    let dot_color = if !ev.recovered
        || matches!(
            ev.start.reason.as_str(),
            "LINK_DOWN" | "NO_IP" | "NO_GATEWAY" | "GATEWAY_UNREACHABLE" | "LOGIN_REJECTED"
        ) {
        p.danger
    } else {
        p.warn
    };

    let frame_resp = egui::Frame::new()
        .fill(p.subtle)
        .stroke(Stroke::new(1.0_f32, p.divider))
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

    frame_resp
        .response
        .on_hover_text(reason_hint(&ev.start.reason));
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
