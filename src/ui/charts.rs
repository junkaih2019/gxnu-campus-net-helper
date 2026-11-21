//! 校园网延迟与断网原因监控看板：
//! - 顶部：通俗化指标（当前延迟 / 平均延迟 / 断网次数）与 1h/7h/24h 切换
//! - 左侧：校园网延迟曲线 + 紧贴正下方共用时间轴的「通断状态轨」+ 跟随鼠标详情浮窗
//! - 右侧：断网记录与可能原因清单（无断网时展示稳定状态）

use std::collections::BTreeMap;

use chrono::{Local, TimeZone};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::metrics_store::{
    compute_stats, filter_range, outages_in_range, uptime_timeline, OutageSummary, RangeStats,
    SlotStatus, TelemetryPoint, TimeRange, TimelineSlot,
};
use crate::ui::icons::Icon;
use crate::ui::theme::{
    body_text, bold_text, format_ms_compact, reason_hint, reason_label, with_alpha, Palette,
    ThemeMode,
};
use crate::ui::widgets::{card_padded, kpi_badge, section_heading, segmented_control};

pub fn draw_monitoring_card(
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

    let latest_check = points.iter().rev().find(|pt| pt.event == "check").copied();

    card_padded(ui, p, 12, |ui| {
        let inner_h = (target_height - 24.0).max(190.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);

        // ── 1. 顶部工具条（26px 统一中线对齐）───────────────────────────
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                section_heading(ui, Icon::Activity, p.accent, "校园网延迟", p.text, 26.0);

                // 标题右侧 ⓘ 说明图标：悬停展示延迟测量方式
                let (info_rect, info_resp) =
                    ui.allocate_exact_size(Vec2::new(18.0, 26.0), Sense::hover());
                let info_resp = info_resp.on_hover_cursor(egui::CursorIcon::Help);
                let info_color = if info_resp.hovered() { p.accent } else { p.text_muted };
                if info_resp.hovered() {
                    ui.painter().circle_filled(info_rect.center(), 9.0, p.accent_soft);
                }
                let ic_rect = Rect::from_center_size(info_rect.center(), Vec2::splat(13.5));
                crate::ui::icons::paint(ui.painter(), ic_rect, Icon::Info, info_color);
                info_resp.on_hover_ui(|ui| {
                    ui.set_max_width(270.0);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(bold_text("延迟测量说明", 11.5, p.text));
                    ui.label(body_text(
                        "每次联网检测正常后，程序会静默直连请求一次校园网认证网关（yc.gxnu.edu.cn/drcom/chkstatus），记录从发出请求到网关返回完整响应的往返耗时。",
                        11.0,
                        p.text_secondary,
                    ));
                    ui.label(body_text(
                        "采用完整 HTTP 响应而非纯 TCP 握手，可避免本机代理/TUN 虚拟网卡秒回导致假 1ms；首次建连略高，后续复用连接后会更低。",
                        10.5,
                        p.text_muted,
                    ));
                });

                ui.add_space(2.0);
                let mut idx = range.index();
                let labels = ["1h", "7h", "24h"];
                if segmented_control(ui, p, "chart_range_seg", &mut idx, &labels) {
                    *range = TimeRange::from_index(idx);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;

                    // 3) 断网次数（大白话，0 次显示绿色“无断网”）
                    if !outages.starts.is_empty() {
                        kpi_badge(
                            ui,
                            p.danger,
                            p.danger_soft,
                            p.danger_border,
                            &format!("断网 {}次", outages.starts.len()),
                        )
                        .on_hover_text(format!(
                            "过去 {} 内发生 {} 次断网，已自动恢复 {} 次",
                            range.label(),
                            outages.starts.len(),
                            outages.recoveries
                        ));
                    } else {
                        kpi_badge(ui, p.ok, p.ok_soft, p.ok_border, "无断网").on_hover_text(
                            format!("过去 {} 内校园网连接稳定，未发生断网", range.label()),
                        );
                    }

                    // 2) 平均延迟
                    if let Some(avg) = stats.avg_ms {
                        kpi_badge(
                            ui,
                            p.text_secondary,
                            p.subtle,
                            p.border,
                            &format!("平均 {}", format_ms_compact(avg)),
                        )
                        .on_hover_text(format!("过去 {} 正常在线时的平均检测延迟", range.label()));
                    }

                    // 1) 当前延迟
                    if let Some(last) = latest_check {
                        if last.online && last.ms == 0 {
                            kpi_badge(ui, p.text_muted, p.subtle, p.border, "当前延迟缺测");
                        } else if last.online {
                            kpi_badge(
                                ui,
                                p.accent,
                                p.accent_soft,
                                p.accent_border,
                                &format!("当前 {}", format_ms_compact(last.ms)),
                            )
                            .on_hover_text(format!(
                                "最近一次检测时间 {}，延迟 {} ms",
                                last.local_hms(),
                                last.ms
                            ));
                        } else {
                            kpi_badge(ui, p.danger, p.danger_soft, p.danger_border, "当前离线")
                                .on_hover_text(format!(
                                    "最近一次检测（{}）：{}",
                                    last.local_hms(),
                                    reason_label(&last.reason)
                                ));
                        }
                    }
                });
            },
        );

        ui.add_space(8.0);

        // ── 2. 主图表区：左（延迟曲线 + 通断轨）+ 右（断网记录与可能原因）───
        let charts_h = (inner_h - 26.0 - 8.0).max(150.0);
        let total_w = ui.available_width();
        let gap = 10.0;
        let show_side_panel = total_w >= 480.0;
        let left_w = if show_side_panel {
            ((total_w - gap) * 0.67).floor()
        } else {
            total_w
        };
        let right_w = (total_w - gap - left_w).max(185.0);

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, charts_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    draw_latency_trend_chart(ui, p, &points, &stats, now, *range, charts_h);
                },
            );
            if show_side_panel {
                ui.allocate_ui_with_layout(
                    Vec2::new(right_w, charts_h),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        draw_disconnect_log_panel(ui, p, &outages, *range, charts_h);
                    },
                );
            }
        });
    });
}

/// 校园网延迟曲线 + 紧贴正下方的 8px 共轴通断状态轨 + 跟随鼠标详情浮窗。
fn draw_latency_trend_chart(
    ui: &mut egui::Ui,
    p: &Palette,
    points: &[&TelemetryPoint],
    stats: &RangeStats,
    now: i64,
    range: TimeRange,
    height: f32,
) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();

    let (chart_bg, chart_border, grid_color, curve_color, grad_top, grad_bottom) = match p.mode {
        ThemeMode::Light => (
            // 小清新通透云白底 + 极淡天蓝细框 + 清爽晴空蓝折线 + 顶部淡蓝到底部全透明的垂直丝绸渐变
            Color32::from_rgb(0xFA, 0xFD, 0xFE),
            Color32::from_rgb(0xE4, 0xED, 0xF7),
            Color32::from_rgb(0xED, 0xF3, 0xFA),
            Color32::from_rgb(0x3B, 0x82, 0xF6),
            Color32::from_rgba_unmultiplied(0x60, 0xA5, 0xFA, 68),
            Color32::from_rgba_unmultiplied(0xDB, 0xEA, 0xFE, 0),
        ),
        ThemeMode::Dark => (
            p.subtle,
            p.divider,
            with_alpha(p.border, 120),
            Color32::from_rgb(0x60, 0xA5, 0xFA),
            Color32::from_rgba_unmultiplied(0x3B, 0x82, 0xF6, 62),
            Color32::from_rgba_unmultiplied(0x3B, 0x82, 0xF6, 0),
        ),
    };
    let offline_color = if p.mode == ThemeMode::Light {
        Color32::from_rgb(0xF4, 0x3F, 0x5E)
    } else {
        p.danger
    };

    painter.rect(
        rect,
        CornerRadius::same(8),
        chart_bg,
        Stroke::new(1.0_f32, chart_border),
        StrokeKind::Inside,
    );

    // 顶部简洁图例条（仅保留延迟与断网）
    let legend_y = rect.top() + 13.0;
    let mut lx = rect.left() + 12.0;
    lx = paint_legend_dot(painter, lx, legend_y, curve_color, "延迟", p.text_secondary);
    let _ = paint_legend_dot(
        painter,
        lx + 12.0,
        legend_y,
        offline_color,
        "断网",
        p.text_secondary,
    );

    if let (Some(min_ms), Some(max_ms)) = (stats.min_ms, stats.max_ms) {
        painter.text(
            Pos2::new(rect.right() - 10.0, legend_y),
            egui::Align2::RIGHT_CENTER,
            format!(
                "范围 {} – {}",
                format_ms_compact(min_ms),
                format_ms_compact(max_ms)
            ),
            egui::FontId::proportional(10.0),
            p.text_muted,
        );
    }

    // 坐标系布局
    let plot = Rect::from_min_max(
        Pos2::new(rect.left() + 42.0, rect.top() + 28.0),
        Pos2::new(rect.right() - 12.0, rect.bottom() - 36.0),
    );
    let track_rect = Rect::from_min_max(
        Pos2::new(plot.left(), plot.bottom() + 7.0),
        Pos2::new(plot.right(), plot.bottom() + 15.0),
    );
    let axis_y = rect.bottom() - 10.0;

    if plot.width() < 24.0 || plot.height() < 24.0 {
        return;
    }

    let check_points: Vec<&TelemetryPoint> = points
        .iter()
        .copied()
        .filter(|pt| pt.event == "check")
        .collect();

    if check_points.is_empty() {
        for frac in [0.0_f32, 0.5_f32, 1.0_f32] {
            let y = plot.top() + plot.height() * frac;
            painter.line_segment(
                [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
                Stroke::new(1.0_f32, with_alpha(p.border, 120)),
            );
        }
        painter.rect_filled(
            track_rect,
            CornerRadius::same(4),
            with_alpha(p.border_strong, 90),
        );
        painter.text(
            Pos2::new(plot.left() - 6.0, track_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "通断",
            egui::FontId::proportional(9.5),
            p.text_muted,
        );
        painter.text(
            plot.center(),
            egui::Align2::CENTER_CENTER,
            "当前时间段暂无检测记录，点击上方「立即检测」立即测一次",
            egui::FontId::proportional(11.5),
            p.text_muted,
        );
        return;
    }

    let max_raw = check_points
        .iter()
        .filter(|pt| pt.online)
        .map(|pt| pt.ms)
        .max()
        .unwrap_or(600);
    let y_max = nice_ceil_ms(max_raw.max(200)) as f32;

    // Y 轴 3 档刻度与柔和网格线
    for (idx, frac) in [0.0_f32, 0.5_f32, 1.0_f32].iter().enumerate() {
        let y = plot.top() + plot.height() * frac;
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(1.0_f32, grid_color),
        );
        let val = ((1.0 - frac) * y_max).round() as u64;
        let label = if idx == 2 {
            "0".to_string()
        } else {
            format_ms_compact(val)
        };
        painter.text(
            Pos2::new(plot.left() - 6.0, y),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::monospace(9.5),
            p.text_muted,
        );
    }

    // 统一时间范围（上方曲线与下方 8px 通断轨严格共用 t_start .. t_end）
    let t_end = now.max(check_points.last().map(|pt| pt.ts).unwrap_or(now));
    let t_start = t_end - range.secs();
    let t_span = (t_end - t_start).max(60) as f32;

    // 共用 X 轴时间刻度（4 个均分点）
    let x_ticks = [
        (0.0_f32, format_hm(t_start), egui::Align2::LEFT_CENTER),
        (
            0.333_f32,
            format_hm(t_start + (t_end - t_start) / 3),
            egui::Align2::CENTER_CENTER,
        ),
        (
            0.667_f32,
            format_hm(t_start + (t_end - t_start) * 2 / 3),
            egui::Align2::CENTER_CENTER,
        ),
        (1.0_f32, "现在".to_string(), egui::Align2::RIGHT_CENTER),
    ];
    for (frac, label, align) in x_ticks {
        let x = plot.left() + frac * plot.width();
        painter.text(
            Pos2::new(x, axis_y),
            align,
            label,
            egui::FontId::proportional(9.5),
            p.text_muted,
        );
    }

    // 均值虚线参考线（轻柔淡蓝虚线）
    if let Some(avg_ms) = stats.avg_ms {
        let avg_y = plot.bottom() - ((avg_ms as f32 / y_max).clamp(0.0, 1.0) * plot.height());
        paint_dashed_hline(
            painter,
            plot.left(),
            plot.right(),
            avg_y,
            Stroke::new(1.0_f32, with_alpha(curve_color, 85)),
        );
    }

    // 映射采样点坐标（>= 2.5px 水平聚合，避免密集点毛刺）
    let mut online_pts: Vec<(Pos2, &TelemetryPoint, usize)> = Vec::new();
    let mut segment = 0;
    let mut offline_pts: Vec<(Pos2, &TelemetryPoint)> = Vec::new();
    let mut hover_targets: Vec<(Pos2, &TelemetryPoint)> = Vec::new();

    let n = check_points.len();
    for pt in &check_points {
        if !pt.online || pt.ms == 0 {
            segment += 1;
        }
        if pt.online && pt.ms == 0 {
            continue;
        }
        let x = if n == 1 {
            plot.center().x
        } else {
            let frac = ((pt.ts - t_start) as f32 / t_span).clamp(0.0, 1.0);
            plot.left() + frac * plot.width()
        };
        let y = if pt.online {
            let norm = (pt.ms as f32 / y_max).clamp(0.02, 1.0);
            plot.bottom() - norm * plot.height()
        } else {
            plot.top() + 5.0
        };
        let pos = Pos2::new(x, y);
        if pt.online {
            if let Some(last) = online_pts.last_mut() {
                if last.2 == segment && (pos.x - last.0.x).abs() < 2.5 {
                    if pt.ms >= last.1.ms {
                        *last = (Pos2::new(last.0.x, pos.y), *pt, segment);
                    }
                    continue;
                }
            }
            online_pts.push((pos, *pt, segment));
        } else {
            if let Some(last) = offline_pts.last() {
                if (pos.x - last.0.x).abs() < 4.0 {
                    continue;
                }
            }
            offline_pts.push((pos, *pt));
        }
    }

    for (pos, pt, _) in &online_pts {
        hover_targets.push((*pos, *pt));
    }
    for (pos, pt) in &offline_pts {
        hover_targets.push((*pos, *pt));
    }

    let plot_painter = ui
        .painter()
        .with_clip_rect(plot.expand2(Vec2::new(4.0, 6.0)));

    // 1) 主延迟曲线 + 顶部淡蓝到底部全透明的柔和垂直渐变面积底
    if online_pts.len() >= 2 {
        let mut mesh = egui::epaint::Mesh::default();
        for pair in online_pts.windows(2) {
            if pair[0].2 == pair[1].2 && pair[1].0.x - pair[0].0.x >= 0.5 {
                let base = mesh.vertices.len() as u32;
                let uv = Pos2::ZERO;
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: pair[0].0,
                    uv,
                    color: grad_top,
                });
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: pair[1].0,
                    uv,
                    color: grad_top,
                });
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: Pos2::new(pair[1].0.x, plot.bottom()),
                    uv,
                    color: grad_bottom,
                });
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: Pos2::new(pair[0].0.x, plot.bottom()),
                    uv,
                    color: grad_bottom,
                });
                mesh.indices.extend_from_slice(&[
                    base,
                    base + 1,
                    base + 2,
                    base,
                    base + 2,
                    base + 3,
                ]);
            }
        }
        if !mesh.indices.is_empty() {
            plot_painter.add(egui::Shape::mesh(mesh));
        }
        for pair in online_pts.windows(2).filter(|pair| pair[0].2 == pair[1].2) {
            plot_painter.line_segment([pair[0].0, pair[1].0], Stroke::new(1.9_f32, curve_color));
        }
    }
    for (pos, _, _) in &online_pts {
        plot_painter.circle_filled(*pos, 1.3, curve_color);
    }

    // 2) 最新点精致白心圆点 + 轻柔断网标记
    if let Some((last_pos, _, _)) = online_pts.last() {
        plot_painter.circle_filled(*last_pos, 6.0, with_alpha(curve_color, 42));
        plot_painter.circle_filled(*last_pos, 3.6, curve_color);
        plot_painter.circle_filled(*last_pos, 1.8, chart_bg);
    }
    for (pos, _) in &offline_pts {
        plot_painter.line_segment(
            [
                Pos2::new(pos.x, plot.top()),
                Pos2::new(pos.x, plot.bottom()),
            ],
            Stroke::new(1.0_f32, with_alpha(offline_color, 52)),
        );
        plot_painter.circle_filled(*pos, 4.5, with_alpha(offline_color, 38));
        plot_painter.circle_filled(*pos, 2.5, offline_color);
    }

    // 3) 紧贴正下方的 8px 细长「通断状态轨」（浅色模式使用清爽马卡龙薄荷绿/暖杏黄/柔珊瑚红）
    painter.text(
        Pos2::new(plot.left() - 6.0, track_rect.center().y),
        egui::Align2::RIGHT_CENTER,
        "通断",
        egui::FontId::proportional(9.5),
        p.text_muted,
    );

    let slots = uptime_timeline(points, t_end, range, 48);
    let slot_count = slots.len().max(1);
    let seg_w = track_rect.width() / slot_count as f32;
    let track_empty = if p.mode == ThemeMode::Light {
        Color32::from_rgb(0xE8, 0xEE, 0xF5)
    } else {
        with_alpha(p.border_strong, 90)
    };
    painter.rect_filled(track_rect, CornerRadius::same(4), track_empty);
    for (idx, slot) in slots.iter().enumerate() {
        let color = match (slot.status, p.mode) {
            (SlotStatus::Online, ThemeMode::Light) => Color32::from_rgb(0x34, 0xD3, 0x99),
            (SlotStatus::Degraded, ThemeMode::Light) => Color32::from_rgb(0xFB, 0xBF, 0x24),
            (SlotStatus::Offline, ThemeMode::Light) => Color32::from_rgb(0xFB, 0x71, 0x85),
            (SlotStatus::Online, ThemeMode::Dark) => p.ok,
            (SlotStatus::Degraded, ThemeMode::Dark) => p.warn,
            (SlotStatus::Offline, ThemeMode::Dark) => p.danger,
            (SlotStatus::NoData, _) => continue,
        };
        let x0 = track_rect.left() + idx as f32 * seg_w;
        let x1 = if idx + 1 == slot_count {
            track_rect.right()
        } else {
            (x0 + seg_w - 0.8).max(x0 + 1.0)
        };
        let seg = Rect::from_min_max(
            Pos2::new(x0, track_rect.top()),
            Pos2::new(x1, track_rect.bottom()),
        );
        let rounding = if idx == 0 {
            CornerRadius {
                nw: 4,
                sw: 4,
                ne: 0,
                se: 0,
            }
        } else if idx + 1 == slot_count {
            CornerRadius {
                nw: 0,
                sw: 0,
                ne: 4,
                se: 4,
            }
        } else {
            CornerRadius::ZERO
        };
        painter.rect_filled(seg, rounding, color);
    }

    // 4) 跟随鼠标的统一风格详情浮窗
    let active_zone = Rect::from_min_max(plot.min, track_rect.max).expand2(Vec2::new(4.0, 4.0));
    let hover_pos = response
        .hover_pos()
        .filter(|pos| active_zone.contains(*pos));

    if let Some(hover) = hover_pos {
        if let Some((pos, pt)) = hover_targets.iter().min_by(|(a, _), (b, _)| {
            (a.x - hover.x)
                .abs()
                .partial_cmp(&(b.x - hover.x).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            let snap_dist = (pos.x - hover.x).abs();
            if snap_dist <= 28.0 {
                painter.line_segment(
                    [
                        Pos2::new(pos.x, plot.top()),
                        Pos2::new(pos.x, track_rect.bottom()),
                    ],
                    Stroke::new(1.0_f32, with_alpha(p.text_muted, 150)),
                );
                let ring_c = if pt.online { p.accent } else { p.danger };
                plot_painter.circle_filled(*pos, 6.0, with_alpha(ring_c, 55));
                plot_painter.circle_stroke(*pos, 4.2, Stroke::new(1.6_f32, ring_c));
                plot_painter.circle_filled(*pos, 2.2, p.card);

                let slot_idx = (((pos.x - track_rect.left()) / seg_w).floor() as isize)
                    .clamp(0, (slot_count - 1) as isize) as usize;
                let hl_slot = &slots[slot_idx];
                paint_hover_callout(ui, p, rect, *pos, pt, stats.avg_ms, hl_slot);
            } else {
                let slot_idx = (((hover.x - track_rect.left()) / seg_w).floor() as isize)
                    .clamp(0, (slot_count - 1) as isize) as usize;
                let hl_slot = &slots[slot_idx];
                painter.line_segment(
                    [
                        Pos2::new(hover.x, plot.top()),
                        Pos2::new(hover.x, track_rect.bottom()),
                    ],
                    Stroke::new(1.0_f32, with_alpha(p.text_muted, 110)),
                );
                paint_slot_callout(ui, p, rect, Pos2::new(hover.x, plot.center().y), hl_slot);
            }
        }
    }
}

/// 跟随鼠标的统一风格详情小窗（简洁大白话）。
fn paint_hover_callout(
    ui: &egui::Ui,
    p: &Palette,
    bounds: Rect,
    anchor: Pos2,
    pt: &TelemetryPoint,
    avg_ms: Option<u64>,
    slot: &TimelineSlot,
) {
    let fg = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("chart_hover_callout"),
    ));

    let card_w = if pt.online { 148.0_f32 } else { 172.0_f32 };
    let card_h = 68.0_f32;

    let place_left = anchor.x + 14.0 + card_w > bounds.right() - 4.0;
    let x0 = if place_left {
        (anchor.x - 14.0 - card_w).max(bounds.left() + 4.0)
    } else {
        anchor.x + 14.0
    };
    let y0 = (anchor.y - card_h * 0.5).clamp(bounds.top() + 6.0, bounds.bottom() - card_h - 6.0);
    let card_rect = Rect::from_min_size(Pos2::new(x0, y0), Vec2::new(card_w, card_h));

    let shadow_rect = card_rect.translate(Vec2::new(0.0, 2.0)).expand(1.5);
    fg.rect_filled(shadow_rect, CornerRadius::same(9), p.shadow);
    fg.rect(
        card_rect,
        CornerRadius::same(8),
        p.card,
        Stroke::new(1.0_f32, p.border_strong),
        StrokeKind::Inside,
    );

    let left = card_rect.left() + 10.0;
    let right = card_rect.right() - 10.0;

    let row1_y = card_rect.top() + 14.0;
    fg.text(
        Pos2::new(left, row1_y),
        egui::Align2::LEFT_CENTER,
        pt.local_hms(),
        egui::FontId::monospace(10.5),
        p.text,
    );
    let (st_color, st_text) = if pt.online {
        (p.ok, "在线")
    } else {
        (p.danger, "断网")
    };
    let g_st = fg.layout_no_wrap(
        st_text.to_string(),
        egui::FontId::new(10.5, crate::ui::theme::bold_family()),
        st_color,
    );
    fg.galley(
        Pos2::new(right - g_st.size().x, row1_y - g_st.size().y * 0.5),
        g_st.clone(),
        st_color,
    );
    fg.circle_filled(
        Pos2::new(right - g_st.size().x - 7.0, row1_y),
        3.0,
        st_color,
    );

    let div_y = card_rect.top() + 26.0;
    fg.line_segment(
        [Pos2::new(left, div_y), Pos2::new(right, div_y)],
        Stroke::new(1.0_f32, p.divider),
    );

    if pt.online {
        paint_kv_row(
            &fg,
            left,
            right,
            div_y + 13.0,
            "延迟",
            &format!("{} ms", pt.ms),
            p.text_muted,
            p.accent,
            true,
        );
        let avg_s = avg_ms
            .map(|v| format!("{v} ms"))
            .unwrap_or_else(|| "—".into());
        paint_kv_row(
            &fg,
            left,
            right,
            div_y + 30.0,
            "平均",
            &avg_s,
            p.text_muted,
            p.text_secondary,
            false,
        );
    } else {
        paint_kv_row(
            &fg,
            left,
            right,
            div_y + 13.0,
            "可能原因",
            reason_label(&pt.reason),
            p.text_muted,
            p.danger,
            true,
        );
        let slot_desc = format!("在线 {}/{} 次", slot.online_checks, slot.checks);
        paint_kv_row(
            &fg,
            left,
            right,
            div_y + 30.0,
            "该时段",
            &slot_desc,
            p.text_muted,
            p.text_secondary,
            false,
        );
    }
}

fn paint_slot_callout(ui: &egui::Ui, p: &Palette, bounds: Rect, anchor: Pos2, slot: &TimelineSlot) {
    let fg = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("chart_slot_callout"),
    ));
    let card_w = 148.0_f32;
    let card_h = 48.0_f32;
    let x0 = if anchor.x + 12.0 + card_w > bounds.right() - 4.0 {
        (anchor.x - 12.0 - card_w).max(bounds.left() + 4.0)
    } else {
        anchor.x + 12.0
    };
    let y0 = (anchor.y - card_h * 0.5).clamp(bounds.top() + 8.0, bounds.bottom() - card_h - 8.0);
    let card_rect = Rect::from_min_size(Pos2::new(x0, y0), Vec2::new(card_w, card_h));

    fg.rect_filled(
        card_rect.translate(Vec2::new(0.0, 2.0)).expand(1.0),
        CornerRadius::same(8),
        p.shadow,
    );
    fg.rect(
        card_rect,
        CornerRadius::same(8),
        p.card,
        Stroke::new(1.0_f32, p.border_strong),
        StrokeKind::Inside,
    );
    let left = card_rect.left() + 10.0;
    fg.text(
        Pos2::new(left, card_rect.top() + 15.0),
        egui::Align2::LEFT_CENTER,
        format!("{} – {}", format_hm(slot.start_ts), format_hm(slot.end_ts)),
        egui::FontId::monospace(10.0),
        p.text,
    );
    let desc = match slot.status {
        SlotStatus::NoData => "该时段暂无检测记录".to_string(),
        _ => format!("在线 {}/{} 次", slot.online_checks, slot.checks),
    };
    fg.text(
        Pos2::new(left, card_rect.top() + 33.0),
        egui::Align2::LEFT_CENTER,
        desc,
        egui::FontId::proportional(10.5),
        p.text_muted,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_kv_row(
    painter: &egui::Painter,
    left: f32,
    right: f32,
    cy: f32,
    key: &str,
    value: &str,
    key_color: Color32,
    val_color: Color32,
    bold_val: bool,
) {
    painter.text(
        Pos2::new(left, cy),
        egui::Align2::LEFT_CENTER,
        key,
        egui::FontId::proportional(10.5),
        key_color,
    );
    let v_font = if bold_val {
        egui::FontId::new(10.5, crate::ui::theme::bold_family())
    } else {
        egui::FontId::proportional(10.5)
    };
    painter.text(
        Pos2::new(right, cy),
        egui::Align2::RIGHT_CENTER,
        value,
        v_font,
        val_color,
    );
}

/// 右侧「断网记录与可能原因」面板：只展示用户真正关心的时间 + 通俗原因。
fn draw_disconnect_log_panel(
    ui: &mut egui::Ui,
    p: &Palette,
    outages: &OutageSummary,
    range: TimeRange,
    height: f32,
) {
    // 按时间正序排列（最早在上、最新在下），最多保留最近 40 条
    let disconnects = &outages.starts[outages.starts.len().saturating_sub(40)..];

    // 统计主要原因：按「出现次数降序 → 最近发生时间降序 → 原因码」确定性排序，
    // 避免 HashMap 随机迭代顺序在平票时随鼠标重绘频繁抖动切换。
    let mut cause_counts: BTreeMap<&str, (usize, i64)> = BTreeMap::new();
    for pt in disconnects {
        let entry = cause_counts.entry(pt.reason.as_str()).or_insert((0, pt.ts));
        entry.0 += 1;
        entry.1 = entry.1.max(pt.ts);
    }
    let top_cause = cause_counts
        .into_iter()
        .max_by_key(|(code, (count, latest_ts))| (*count, *latest_ts, *code))
        .map(|(code, (count, _))| (reason_label(code), count));

    egui::Frame::new()
        .fill(p.subtle)
        .stroke(Stroke::new(1.0_f32, p.divider))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin {
            left: 11,
            right: 11,
            top: 8,
            bottom: 8,
        })
        .show(ui, |ui| {
            let inner_h = (height - 16.0).max(120.0);
            ui.set_min_height(inner_h);
            ui.set_max_height(inner_h);

            // 标题行
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(bold_text("断网记录与原因", 11.5, p.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if disconnects.is_empty() {
                            ui.label(bold_text("网络稳定", 10.5, p.ok));
                        } else {
                            ui.label(body_text(
                                format!("共 {} 次", outages.starts.len()),
                                10.5,
                                p.text_muted,
                            ));
                        }
                    });
                },
            );

            ui.add_space(4.0);

            if disconnects.is_empty() {
                let body_h = (inner_h - 24.0).max(60.0);
                let (empty_rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), body_h), Sense::hover());
                let c = empty_rect.center();
                ui.painter()
                    .circle_filled(Pos2::new(c.x, c.y - 16.0), 14.0, p.ok_soft);
                let icon_rect =
                    Rect::from_center_size(Pos2::new(c.x, c.y - 16.0), Vec2::splat(13.0));
                crate::ui::icons::paint(ui.painter(), icon_rect, Icon::Check, p.ok);
                ui.painter().text(
                    Pos2::new(c.x, c.y + 8.0),
                    egui::Align2::CENTER_CENTER,
                    "连接稳定，暂无断网",
                    egui::FontId::new(11.5, crate::ui::theme::bold_family()),
                    p.text,
                );
                ui.painter().text(
                    Pos2::new(c.x, c.y + 25.0),
                    egui::Align2::CENTER_CENTER,
                    format!("过去 {} 未检测到异常中断", range.label()),
                    egui::FontId::proportional(10.5),
                    p.text_muted,
                );
                return;
            }

            // 中间滚动列表（默认贴底显示最新记录）+ 底部 22px 常见原因摘要
            let footer_h = 22.0;
            let list_h = (inner_h - 24.0 - footer_h - 6.0).max(52.0);
            egui::ScrollArea::vertical()
                .id_salt("disconnect_reasons_scroll")
                .max_height(list_h)
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    for pt in disconnects {
                        let row_w = ui.available_width();
                        let (row_rect, resp) =
                            ui.allocate_exact_size(Vec2::new(row_w, 22.0), Sense::hover());
                        if resp.hovered() {
                            ui.painter().rect_filled(
                                row_rect,
                                CornerRadius::same(5),
                                with_alpha(p.card, 200),
                            );
                        }
                        let cy = row_rect.center().y;
                        let x = row_rect.left() + 5.0;
                        ui.painter()
                            .circle_filled(Pos2::new(x + 2.5, cy), 3.0, p.danger);
                        ui.painter().text(
                            Pos2::new(x + 10.0, cy),
                            egui::Align2::LEFT_CENTER,
                            pt.local_md_hm(),
                            egui::FontId::monospace(10.0),
                            p.text_muted,
                        );
                        ui.painter().text(
                            Pos2::new(x + 84.0, cy),
                            egui::Align2::LEFT_CENTER,
                            reason_label(&pt.reason),
                            egui::FontId::proportional(11.0),
                            p.text,
                        );
                        resp.on_hover_text(format!(
                            "时间：{} ({})\n可能原因：{}\n说明：{}",
                            pt.local_md_hm(),
                            pt.local_hms(),
                            reason_label(&pt.reason),
                            reason_hint(&pt.reason)
                        ));
                    }
                });

            // 底部摘要（纯 Painter 统一水平中线，消除左右高低错位）
            ui.add_space(4.0);
            let sep = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
            ui.painter().line_segment(
                [sep.left_center(), sep.right_center()],
                Stroke::new(1.0_f32, p.divider),
            );
            let (footer_rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), footer_h), Sense::hover());
            let fcy = footer_rect.center().y;
            if let Some((label, count)) = top_cause {
                ui.painter().text(
                    Pos2::new(footer_rect.left(), fcy),
                    egui::Align2::LEFT_CENTER,
                    format!("主因：{label}({count})"),
                    egui::FontId::proportional(10.5),
                    p.text_secondary,
                );
            }
            let rec_color = if outages.recoveries > 0 {
                p.ok
            } else {
                p.text_muted
            };
            ui.painter().text(
                Pos2::new(footer_rect.right(), fcy),
                egui::Align2::RIGHT_CENTER,
                format!("已恢复 {}次", outages.recoveries),
                egui::FontId::proportional(10.5),
                rec_color,
            );
        });
}

fn paint_legend_dot(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    color: Color32,
    label: &str,
    text_color: Color32,
) -> f32 {
    painter.circle_filled(Pos2::new(x + 3.0, y), 3.0, color);
    let galley = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(10.0),
        text_color,
    );
    let w = galley.size().x;
    painter.galley(
        Pos2::new(x + 9.0, y - galley.size().y * 0.5),
        galley,
        text_color,
    );
    x + 9.0 + w
}

fn paint_dashed_hline(painter: &egui::Painter, x0: f32, x1: f32, y: f32, stroke: Stroke) {
    let mut x = x0;
    while x < x1 {
        let end = (x + 4.0).min(x1);
        painter.line_segment([Pos2::new(x, y), Pos2::new(end, y)], stroke);
        x += 8.0;
    }
}

fn nice_ceil_ms(ms: u64) -> u64 {
    if ms <= 200 {
        200
    } else if ms <= 400 {
        400
    } else if ms <= 800 {
        800
    } else if ms <= 1000 {
        1000
    } else if ms <= 2000 {
        2000
    } else {
        ms.div_ceil(1000).saturating_mul(1000)
    }
}

fn format_hm(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".to_string())
}
