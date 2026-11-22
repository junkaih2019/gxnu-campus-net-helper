//! 主页核心可视化模块（v0.2.1）：
//! - Tier 2：均衡双栏自助服务卡片（左「账号流量」单圆环 + 运营商/校园网切换 + 口径说明 | 右「在线设备」并发配额与终端列表）
//! - Tier 3：通栏纤薄「连接稳定性」胶囊时间轴（位于双卡之下、运行日志之上，悬停黄格即看完整断联卡片）

use std::f32::consts::{FRAC_PI_2, TAU};

use chrono::{Local, TimeZone, Timelike};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::metrics_store::{
    build_health_track, compute_stats, filter_range, outages_in_range, OutageEvent, TelemetryPoint,
    TimeRange, TrackSegment, TrackState,
};
use crate::models::{
    OnlineDevice, SelfServiceData, SelfServiceState, SelfServiceStatus, TrafficSummary,
};
use crate::ui::icons::Icon;
use crate::ui::theme::{
    body_text, bold_text, reason_hint, reason_label, with_alpha, Palette, ThemeMode,
    DEFAULT_TRAFFIC_SPLIT, RADIUS_MD,
};
use crate::ui::widgets::{
    badge, card_padded, icon_button, section_heading, segmented_control, vertical_splitter,
};

/// 账号流量卡展示视角（默认跟随当前活跃账号运营商，支持点击按钮随时切换）：
/// - `Carrier`：运营商视角（蓝/绿双色圆环展示全账号本月实际下行/上行与吞吐说明）
/// - `Campus`：校园网视角（余量进度圆环展示校园网套餐剩余/已用与计费说明）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrafficViewMode {
    #[default]
    Carrier,
    Campus,
}

impl TrafficViewMode {
    pub fn default_for_carrier(carrier: &str) -> Self {
        if carrier == "0" {
            Self::Campus
        } else {
            Self::Carrier
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Carrier => 0,
            Self::Campus => 1,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        match idx {
            1 => Self::Campus,
            _ => Self::Carrier,
        }
    }
}

#[derive(Default)]
pub struct HomeVisualActions {
    pub split_changed: bool,
    pub refresh_self_service: bool,
    pub traffic_view_changed: bool,
}

/// Tier 2：账号流量（左） + 在线设备（右）双栏卡片（可左右拖拽调整比例）。
#[allow(clippy::too_many_arguments)]
pub fn draw_self_service_row(
    ui: &mut egui::Ui,
    p: &Palette,
    self_service: &SelfServiceState,
    local_traffic: &TrafficSummary,
    view_mode: &mut TrafficViewMode,
    local_campus_ip: &str,
    is_monitor_online: bool,
    login_rejected: bool,
    split_ratio: &mut f32,
    target_height: f32,
) -> HomeVisualActions {
    let mut actions = HomeVisualActions::default();
    let total_w = ui.available_width();
    let splitter_w = 10.0_f32;
    let usable_w = (total_w - splitter_w).max(460.0);
    let min_left = 280.0_f32.min(usable_w * 0.44);
    let min_right = 260.0_f32.min(usable_w * 0.44);
    let left_w = (usable_w * (*split_ratio).clamp(0.38, 0.64))
        .clamp(min_left, (usable_w - min_right).max(min_left))
        .floor();
    let right_w = (usable_w - left_w).max(min_right);

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;

        ui.allocate_ui_with_layout(
            Vec2::new(left_w, target_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                let (refresh, changed) = draw_account_traffic_card(
                    ui,
                    p,
                    self_service,
                    local_traffic,
                    view_mode,
                    target_height,
                );
                if refresh {
                    actions.refresh_self_service = true;
                }
                if changed {
                    actions.traffic_view_changed = true;
                }
            },
        );

        let (dx, dbl) = vertical_splitter(ui, p, target_height);
        if dbl {
            *split_ratio = DEFAULT_TRAFFIC_SPLIT;
            actions.split_changed = true;
        } else if dx.abs() > 0.01 && usable_w > 1.0 {
            *split_ratio = ((left_w + dx) / usable_w).clamp(0.38, 0.64);
            actions.split_changed = true;
        }

        ui.allocate_ui_with_layout(
            Vec2::new(right_w, target_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                if draw_online_devices_card(
                    ui,
                    p,
                    self_service,
                    local_campus_ip,
                    is_monitor_online,
                    login_rejected,
                    target_height,
                ) {
                    actions.refresh_self_service = true;
                }
            },
        );
    });

    actions
}

/// 左卡：账号流量（单圆环 + 清晰数据与口径说明，支持「运营商 / 校园网」一键切换）。
fn draw_account_traffic_card(
    ui: &mut egui::Ui,
    p: &Palette,
    self_service: &SelfServiceState,
    local_traffic: &TrafficSummary,
    view_mode: &mut TrafficViewMode,
    target_height: f32,
) -> (bool, bool) {
    let mut refresh_clicked = false;
    let mut view_changed = false;

    card_padded(ui, p, 12, |ui| {
        let inner_h = (target_height - 24.0).max(112.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);
        ui.spacing_mut().item_spacing.y = 0.0;

        let card_w = ui.available_width();
        let has_data = self_service.data.is_some();

        // 1. 顶部标题行（26px）：标题 + 状态小徽章 + 「运营商 | 校园网」切换 + 刷新按钮
        ui.allocate_ui_with_layout(
            Vec2::new(card_w, 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                section_heading(ui, Icon::ArrowUpDown, p.accent, "账号流量", p.text, 26.0);

                if card_w >= 318.0 {
                    let campus_depleted = self_service
                        .data
                        .as_ref()
                        .is_some_and(|d| d.traffic.campus_remaining_mb <= 0.1);
                    let (lbl, fg, bg, bd, tip) = match (self_service.status, has_data) {
                        (SelfServiceStatus::Refreshing, _) => (
                            "同步中",
                            p.accent,
                            p.accent_soft,
                            p.accent_border,
                            "正在向校园网自助服务同步最新数据",
                        ),
                        (_, true) if *view_mode == TrafficViewMode::Carrier => (
                            "不限流量",
                            p.ok,
                            p.ok_soft,
                            p.ok_border,
                            "电信/联通/移动宽带出口不限流量，圆环展示本月全账号实际上下行总流量",
                        ),
                        (_, true) if campus_depleted => (
                            "额度用尽",
                            p.danger,
                            p.danger_soft,
                            p.danger_border,
                            "校园网套餐剩余流量已用尽，仅校园网出口受限",
                        ),
                        (_, true) => (
                            "套餐计费",
                            p.accent,
                            p.accent_soft,
                            p.accent_border,
                            "仅校园网出口计入套餐额度，圆环展示自助服务官方剩余可用比例",
                        ),
                        (SelfServiceStatus::CaptchaCooldown, false) => (
                            "需验证码",
                            p.warn,
                            p.warn_soft,
                            p.warn_border,
                            self_service.message.as_str(),
                        ),
                        (SelfServiceStatus::NoCredentials, false) => (
                            "未配置",
                            p.warn,
                            p.warn_soft,
                            p.warn_border,
                            "请先在设置中保存学号与密码",
                        ),
                        (SelfServiceStatus::AuthFailed, false) => (
                            "登录未过",
                            p.warn,
                            p.warn_soft,
                            p.warn_border,
                            self_service.message.as_str(),
                        ),
                        (
                            SelfServiceStatus::Unreachable | SelfServiceStatus::UnsupportedPassword,
                            false,
                        ) => (
                            "未同步",
                            p.neutral,
                            p.neutral_soft,
                            p.neutral_border,
                            self_service.message.as_str(),
                        ),
                        _ => (
                            "网卡参考",
                            p.neutral,
                            p.neutral_soft,
                            p.neutral_border,
                            "等待自助服务同步，当前展示本机物理网卡今日流量参考",
                        ),
                    };
                    let resp = badge(ui, fg, bg, bd, lbl);
                    if !tip.is_empty() {
                        resp.on_hover_text(tip);
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    if icon_button(
                        ui,
                        p,
                        Icon::Refresh,
                        Vec2::splat(24.0),
                        12.0,
                        false,
                        "手动刷新校园网自助服务数据（15 秒防抖）",
                    )
                    .clicked()
                    {
                        refresh_clicked = true;
                    }
                    if has_data && card_w >= 265.0 {
                        let mut idx = view_mode.index();
                        if segmented_control(
                            ui,
                            p,
                            "traffic_view_seg",
                            &mut idx,
                            &["运营商", "校园网"],
                        ) {
                            *view_mode = TrafficViewMode::from_index(idx);
                            view_changed = true;
                        }
                    }
                });
            },
        );

        ui.add_space(8.0);

        // 2. 主体区（默认 80px）：左 76×76 抗锯齿圆环 + 右侧指标与口径说明
        let body_h = (inner_h - 34.0).max(76.0);
        let ring_size = 76.0_f32;
        let ring_gap = 14.0_f32;
        let info_w = (card_w - ring_size - ring_gap).max(120.0);

        ui.allocate_ui_with_layout(
            Vec2::new(card_w, body_h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = ring_gap;

                if let Some(data) = &self_service.data {
                    let t = &data.traffic;
                    match *view_mode {
                        TrafficViewMode::Carrier => {
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

                            let ring_resp = draw_dual_donut_ring(
                                ui,
                                p,
                                ring_size,
                                dl_frac,
                                down > 0.0,
                                up > 0.0,
                                &format_mb_ring(total),
                                "本月实际",
                                p.text,
                            );
                            ring_resp.on_hover_ui(|ui| {
                                ui.set_min_width(226.0);
                                ui.spacing_mut().item_spacing.y = 4.0;
                                ui.label(bold_text("本月全账号实际流量", 11.5, p.text));
                                detail_kv_row(ui, p, "实际总流量", &format_mb_compact(total));
                                detail_kv_row(
                                    ui,
                                    p,
                                    "下行流量",
                                    &format!("{} ({dl_pct}%)", format_mb_compact(down)),
                                );
                                detail_kv_row(
                                    ui,
                                    p,
                                    "上行流量",
                                    &format!("{} ({ul_pct}%)", format_mb_compact(up)),
                                );
                                if !t.user_group_name.is_empty() {
                                    detail_kv_row(ui, p, "用户身份", &t.user_group_name);
                                }
                                if let Some(n) = t.month_session_count {
                                    detail_kv_row(ui, p, "本月认证次数", &format!("{n} 次"));
                                }
                                detail_kv_row(
                                    ui,
                                    p,
                                    "校园网余量",
                                    &format_mb_compact(t.campus_remaining_mb),
                                );
                            });

                            let carrier_sub = match (
                                !t.user_group_name.is_empty(),
                                t.month_session_count,
                            ) {
                                (true, Some(n)) => {
                                    format!("身份：{} · 本月认证 {n} 次", t.user_group_name)
                                }
                                (true, None) => {
                                    format!("身份：{} · 统计同一学号全部终端", t.user_group_name)
                                }
                                (false, Some(n)) => {
                                    format!("本月认证 {n} 次 · 统计同一学号全部终端")
                                }
                                (false, None) => "统计同一学号全部终端的实际吞吐".to_string(),
                            };

                            draw_traffic_info_column(
                                ui,
                                p,
                                info_w,
                                (
                                    p.accent,
                                    "下行流量",
                                    &format_mb_compact(down),
                                    if total > 0.001 {
                                        Some(format!("{dl_pct}%"))
                                    } else {
                                        None
                                    },
                                ),
                                (
                                    p.ok,
                                    "上行流量",
                                    &format_mb_compact(up),
                                    if total > 0.001 {
                                        Some(format!("{ul_pct}%"))
                                    } else {
                                        None
                                    },
                                ),
                                "运营商出口不限流量 · 统计全账号实际吞吐",
                                &carrier_sub,
                                Some(
                                    "数据源自校园网自助服务上网明细，汇总同一学号下全部终端与运营商出口的实际流量",
                                ),
                            );
                        }
                        TrafficViewMode::Campus => {
                            let rem = t.campus_remaining_mb.max(0.0);
                            let used = t.campus_used_mb.max(0.0);
                            let total = t.campus_total_mb().max(0.001);
                            let rem_ratio = (rem / total).clamp(0.0, 1.0) as f32;
                            let used_pct = ((used / total) * 100.0).clamp(0.0, 100.0);
                            let depleted = rem <= 0.1;

                            let ring_color = if depleted || rem_ratio <= 0.05 {
                                p.danger
                            } else if rem_ratio <= 0.20 {
                                p.warn
                            } else {
                                p.ok
                            };
                            let center_val = if depleted {
                                "已用尽".to_string()
                            } else if used <= 0.01 {
                                "100%".to_string()
                            } else {
                                format!("{:.1}%", (rem_ratio * 100.0).min(99.9))
                            };

                            let ring_resp = draw_single_donut_ring(
                                ui,
                                p,
                                ring_size,
                                if depleted { 0.0 } else { rem_ratio },
                                ring_color,
                                &center_val,
                                "剩余可用",
                                if depleted { p.danger } else { p.text },
                            );
                            ring_resp.on_hover_ui(|ui| {
                                ui.set_min_width(226.0);
                                ui.spacing_mut().item_spacing.y = 4.0;
                                ui.label(bold_text("校园网出口套餐明细", 11.5, p.text));
                                detail_kv_row(ui, p, "剩余可用", &format_mb_compact(rem));
                                detail_kv_row(
                                    ui,
                                    p,
                                    "本月已用",
                                    &format!("{} ({used_pct:.1}%)", format_mb_compact(used)),
                                );
                                detail_kv_row(ui, p, "账户总额", &format_mb_compact(total));
                                if !t.user_group_name.is_empty() {
                                    detail_kv_row(ui, p, "用户身份", &t.user_group_name);
                                }
                                if !t.plan_description.is_empty() {
                                    detail_kv_row(ui, p, "免费额度", &t.plan_description);
                                }
                                detail_kv_row(
                                    ui,
                                    p,
                                    "全账号实际",
                                    &format_mb_compact(t.month_actual_total_mb()),
                                );
                            });

                            let identity_line = match (
                                !t.user_group_name.is_empty(),
                                !t.plan_description.is_empty(),
                            ) {
                                (true, true) => {
                                    format!("身份：{} · {}", t.user_group_name, t.plan_description)
                                }
                                (true, false) => format!("身份：{}", t.user_group_name),
                                (false, true) => format!("校园网免费额度 · {}", t.plan_description),
                                (false, false) => "校园网出口计费额度".to_string(),
                            };
                            let billing_note = "仅校园网出口计费 · 剩余免费额度可结转";
                            let rem_str = if depleted {
                                "已用尽".to_string()
                            } else {
                                format_mb_compact(rem)
                            };

                            let rem_pct = if depleted {
                                "0%".to_string()
                            } else if used <= 0.01 {
                                "100%".to_string()
                            } else {
                                format!("{:.1}%", (rem_ratio * 100.0).min(99.9))
                            };

                            draw_traffic_info_column(
                                ui,
                                p,
                                info_w,
                                (ring_color, "剩余可用", &rem_str, Some(rem_pct)),
                                (
                                    p.accent,
                                    "本月已用",
                                    &format_mb_compact(used),
                                    Some(format!("{used_pct:.1}%")),
                                ),
                                &identity_line,
                                billing_note,
                                Some(
                                    "校园网出口独立计费；若同时办理电信/联通/移动宽带，该宽带流量不扣此额度",
                                ),
                            );
                        }
                    }
                } else {
                    // 无门户缓存时优雅降级：展示本机物理网卡今日流量圆环与原因说明
                    let rx = local_traffic.today_rx_bytes;
                    let tx = local_traffic.today_tx_bytes;
                    let total = rx.saturating_add(tx);
                    let dl_frac = if total > 0 {
                        rx as f32 / total as f32
                    } else {
                        0.0
                    };
                    let _ = draw_dual_donut_ring(
                        ui,
                        p,
                        ring_size,
                        dl_frac,
                        rx > 0,
                        tx > 0,
                        &format_bytes_ring(total),
                        "本机今日",
                        p.text,
                    );
                    let hint = if self_service.message.is_empty() {
                        "连入校园网后自动拉取官方流量与在线设备"
                    } else {
                        self_service.message.as_str()
                    };
                    draw_traffic_info_column(
                        ui,
                        p,
                        info_w,
                        (p.accent, "本机下载", &format_bytes_compact(rx), None),
                        (p.ok, "本机上传", &format_bytes_compact(tx), None),
                        "暂未同步自助服务 · 显示本机物理网卡参考",
                        hint,
                        Some(hint),
                    );
                }
            },
        );
    });

    (refresh_clicked, view_changed)
}

#[allow(clippy::too_many_arguments)]
fn draw_traffic_info_column(
    ui: &mut egui::Ui,
    p: &Palette,
    width: f32,
    row1: (Color32, &str, &str, Option<String>),
    row2: (Color32, &str, &str, Option<String>),
    note_primary: &str,
    note_secondary: &str,
    note_tooltip: Option<&str>,
) {
    // 严格 78px 高度（18 + 2 + 18 + 4 + 1 + 4 + 15.5 + 15.5 = 78.0），杜绝字体行高溢出撑高左卡
    ui.allocate_ui_with_layout(
        Vec2::new(width, 78.0),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;

            draw_ring_metric_row(ui, p, width, row1.0, row1.1, row1.2, row1.3.as_deref());
            ui.add_space(2.0);
            draw_ring_metric_row(ui, p, width, row2.0, row2.1, row2.2, row2.3.as_deref());
            ui.add_space(4.0);

            let sep = ui.allocate_space(Vec2::new(width, 1.0)).1;
            ui.painter().line_segment(
                [sep.left_center(), sep.right_center()],
                Stroke::new(1.0_f32, p.divider),
            );
            ui.add_space(4.0);

            let resp1 =
                paint_clipped_note_line(ui, width, 15.5, note_primary, 10.2, p.text_secondary);
            let resp2 = paint_clipped_note_line(ui, width, 15.5, note_secondary, 9.8, p.text_muted);
            if let Some(tip) = note_tooltip {
                let full_tip = format!("{note_primary}\n{note_secondary}\n{tip}");
                resp1.on_hover_text(&full_tip);
                resp2.on_hover_text(&full_tip);
            }
        },
    );
}

fn paint_clipped_note_line(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    text: &str,
    size: f32,
    color: Color32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), egui::FontId::proportional(size), color);
    let painter = ui.painter().with_clip_rect(rect);
    painter.galley(
        Pos2::new(rect.left(), rect.center().y - g.size().y * 0.5),
        g,
        color,
    );
    resp
}

fn draw_ring_metric_row(
    ui: &mut egui::Ui,
    p: &Palette,
    width: f32,
    dot_color: Color32,
    label: &str,
    value: &str,
    suffix: Option<&str>,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 18.0), Sense::hover());
    let cy = rect.center().y;
    let painter = ui.painter();

    painter.circle_filled(Pos2::new(rect.left() + 3.5, cy), 3.2, dot_color);
    let lbl_g = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(11.0),
        p.text_secondary,
    );
    painter.galley(
        Pos2::new(rect.left() + 12.0, cy - lbl_g.size().y * 0.5),
        lbl_g,
        p.text_secondary,
    );

    let mut right_x = rect.right();
    if let Some(suf) = suffix {
        let suf_g = painter.layout_no_wrap(
            suf.to_string(),
            egui::FontId::proportional(10.0),
            p.text_muted,
        );
        painter.galley(
            Pos2::new(right_x - suf_g.size().x, cy - suf_g.size().y * 0.5),
            suf_g,
            p.text_muted,
        );
        // 固定 44px 百分比列宽，使流量数值列与百分比列各自右对齐且中间保持清爽列间距
        right_x -= 44.0;
    }
    let val_g = painter.layout_no_wrap(
        value.to_string(),
        egui::FontId::new(12.0, crate::ui::theme::bold_family()),
        p.text,
    );
    painter.galley(
        Pos2::new(right_x - val_g.size().x, cy - val_g.size().y * 0.5),
        val_g,
        p.text,
    );
}

/// 抗锯齿双色环形图（蓝=下行，绿=上行），带柔和圆头与清晰间隙。
#[allow(clippy::too_many_arguments)]
fn draw_dual_donut_ring(
    ui: &mut egui::Ui,
    p: &Palette,
    size: f32,
    left_frac: f32,
    has_left: bool,
    has_right: bool,
    center_value: &str,
    center_caption: &str,
    value_color: Color32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let thickness = 7.0_f32;
    let radius = (size * 0.5 - thickness * 0.5 - 1.0).max(16.0);
    let track = ring_track_color(p);
    painter.circle_stroke(center, radius, Stroke::new(thickness, track));

    if has_left && has_right {
        let gap = 0.12_f32;
        let usable = TAU - 2.0 * gap;
        let f = left_frac.clamp(0.08, 0.92);
        let sweep1 = usable * f;
        let sweep2 = usable * (1.0 - f);
        let start1 = -FRAC_PI_2 + gap * 0.5;
        let start2 = start1 + sweep1 + gap;
        paint_ring_arc(painter, center, radius, thickness, start1, sweep1, p.accent);
        paint_ring_arc(painter, center, radius, thickness, start2, sweep2, p.ok);
    } else if has_left {
        painter.circle_stroke(center, radius, Stroke::new(thickness, p.accent));
    } else if has_right {
        painter.circle_stroke(center, radius, Stroke::new(thickness, p.ok));
    }

    paint_ring_center_text(
        painter,
        center,
        center_value,
        center_caption,
        value_color,
        p.text_muted,
    );
    resp
}

/// 抗锯齿单色进度环形图（校园网套餐余量）。
#[allow(clippy::too_many_arguments)]
fn draw_single_donut_ring(
    ui: &mut egui::Ui,
    p: &Palette,
    size: f32,
    ratio: f32,
    color: Color32,
    center_value: &str,
    center_caption: &str,
    value_color: Color32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let thickness = 7.0_f32;
    let radius = (size * 0.5 - thickness * 0.5 - 1.0).max(16.0);
    let track = ring_track_color(p);
    painter.circle_stroke(center, radius, Stroke::new(thickness, track));

    let clamped = ratio.clamp(0.0, 1.0);
    if clamped >= 0.996 {
        painter.circle_stroke(center, radius, Stroke::new(thickness, color));
    } else if clamped > 0.005 {
        let sweep = (clamped * TAU).max(0.18);
        paint_ring_arc(painter, center, radius, thickness, -FRAC_PI_2, sweep, color);
    }

    paint_ring_center_text(
        painter,
        center,
        center_value,
        center_caption,
        value_color,
        p.text_muted,
    );
    resp
}

fn ring_track_color(p: &Palette) -> Color32 {
    match p.mode {
        ThemeMode::Light => Color32::from_rgb(0xE5, 0xEA, 0xF2),
        ThemeMode::Dark => with_alpha(p.border_strong, 105),
    }
}

fn paint_ring_arc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    thickness: f32,
    start_angle: f32,
    sweep: f32,
    color: Color32,
) {
    if sweep <= 0.01 {
        return;
    }
    let cap_r = thickness * 0.5;
    let cap_angle = (cap_r / radius).min(sweep * 0.42);
    let a0 = start_angle + cap_angle;
    let a1 = start_angle + sweep - cap_angle;
    if a1 <= a0 {
        let mid = start_angle + sweep * 0.5;
        let pt = Pos2::new(center.x + radius * mid.cos(), center.y + radius * mid.sin());
        painter.circle_filled(pt, cap_r, color);
        return;
    }
    let span = a1 - a0;
    let steps = ((span / TAU) * 72.0).ceil().clamp(10.0, 72.0) as usize;
    let mut pts = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let ang = a0 + span * (i as f32 / steps as f32);
        pts.push(Pos2::new(
            center.x + radius * ang.cos(),
            center.y + radius * ang.sin(),
        ));
    }
    if let (Some(&first), Some(&last)) = (pts.first(), pts.last()) {
        painter.add(egui::Shape::line(pts, Stroke::new(thickness, color)));
        painter.circle_filled(first, cap_r, color);
        painter.circle_filled(last, cap_r, color);
    }
}

fn paint_ring_center_text(
    painter: &egui::Painter,
    center: Pos2,
    value: &str,
    caption: &str,
    value_color: Color32,
    caption_color: Color32,
) {
    let val_size = if value.chars().count() >= 6 {
        11.8
    } else {
        13.0
    };
    painter.text(
        Pos2::new(center.x, center.y - 6.0),
        egui::Align2::CENTER_CENTER,
        value,
        egui::FontId::new(val_size, crate::ui::theme::bold_family()),
        value_color,
    );
    painter.text(
        Pos2::new(center.x, center.y + 10.5),
        egui::Align2::CENTER_CENTER,
        caption,
        egui::FontId::proportional(9.4),
        caption_color,
    );
}

/// 右卡：在线设备（聚焦当前在线终端、并发配额与清晰状态说明，悬停页脚入口可按需查看近期上网记录）。
fn draw_online_devices_card(
    ui: &mut egui::Ui,
    p: &Palette,
    self_service: &SelfServiceState,
    local_campus_ip: &str,
    is_monitor_online: bool,
    login_rejected: bool,
    target_height: f32,
) -> bool {
    let mut refresh_clicked = false;
    let now_unix = Local::now().timestamp();

    card_padded(ui, p, 12, |ui| {
        let inner_h = (target_height - 24.0).max(112.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);
        ui.spacing_mut().item_spacing.y = 0.0;

        let card_w = ui.available_width();

        // 1. 顶部标题行（26px）：在线设备 + 并发配额徽章 + 同步时间 + 刷新按钮
        ui.allocate_ui_with_layout(
            Vec2::new(card_w, 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                section_heading(ui, Icon::Monitor, p.ok, "在线设备", p.text, 26.0);

                if let Some(data) = &self_service.data {
                    let count = data.online_devices.len();
                    let at_limit = data.max_online_devices.is_some_and(|m| count >= m as usize);
                    let pill_txt = match data.max_online_devices {
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
                    let tip = if login_rejected && count > 0 {
                        "认证被拒且检测到当前账号存在占线终端，请确认是否达到并发上限"
                    } else if at_limit {
                        "已达到该学号允许同时在线的终端上限，再登录新设备可能触发挤线"
                    } else {
                        "同一学号可在多终端或不同运营商出口并发登录"
                    };
                    badge(ui, fg, bg, bd, &pill_txt).on_hover_text(tip);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    if icon_button(
                        ui,
                        p,
                        Icon::Refresh,
                        Vec2::splat(24.0),
                        12.0,
                        false,
                        "刷新在线终端列表",
                    )
                    .clicked()
                    {
                        refresh_clicked = true;
                    }
                    let sync_text = match self_service.status {
                        SelfServiceStatus::Refreshing => "同步中".to_string(),
                        _ => match &self_service.data {
                            Some(d) => format_sync_age(now_unix - d.fetched_at_unix),
                            None => String::new(),
                        },
                    };
                    if !sync_text.is_empty() {
                        let lbl = ui.label(body_text(&sync_text, 10.2, p.text_muted));
                        if let Some(d) = &self_service.data {
                            let mut tip = format!("上次同步：{}", format_md_hms(d.fetched_at_unix));
                            if !self_service.message.is_empty() {
                                tip.push_str(&format!("\n状态：{}", self_service.message));
                            }
                            lbl.on_hover_text(tip);
                        }
                    }
                });
            },
        );

        ui.add_space(8.0);

        let body_h = (inner_h - 34.0).max(76.0);

        let Some(data) = &self_service.data else {
            draw_devices_placeholder(ui, p, self_service, local_campus_ip, card_w, body_h);
            return;
        };

        let mut devices: Vec<(&OnlineDevice, bool)> = data
            .online_devices
            .iter()
            .map(|dev| {
                let is_local = (!local_campus_ip.is_empty() && dev.ip == local_campus_ip)
                    || (local_campus_ip.is_empty()
                        && is_monitor_online
                        && data.online_devices.len() == 1);
                (dev, is_local)
            })
            .collect();
        devices.sort_by_key(|(_, is_local)| !*is_local);

        let count = devices.len();
        let local_present = devices.iter().any(|(_, is_local)| *is_local);
        let other_count = devices.iter().filter(|(_, is_local)| !*is_local).count();
        let at_limit = data.max_online_devices.is_some_and(|m| count >= m as usize);

        // 2. 在线终端列表区（默认 58px：1 台舒展双行卡片 / 2 台两行齐平直出 / 3+ 台平滑滚动）
        let footer_total_h = 24.0_f32;
        let dev_area_h = (body_h - footer_total_h).max(52.0);

        ui.allocate_ui_with_layout(
            Vec2::new(card_w, dev_area_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                if devices.is_empty() {
                    draw_empty_devices_box(ui, p, card_w, dev_area_h.min(54.0));
                } else if devices.len() == 1 {
                    let (dev, is_local) = devices[0];
                    draw_featured_device_card(
                        ui,
                        p,
                        dev,
                        is_local,
                        at_limit || login_rejected,
                        data.fetched_at_unix,
                        now_unix,
                        card_w,
                        dev_area_h.min(56.0),
                    );
                } else if devices.len() == 2 && dev_area_h >= 56.0 {
                    for (dev, is_local) in &devices {
                        draw_online_device_row(
                            ui,
                            p,
                            dev,
                            *is_local,
                            at_limit || login_rejected,
                            data.fetched_at_unix,
                            now_unix,
                        );
                    }
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("online_devices_scroll")
                        .min_scrolled_height(0.0)
                        .max_height(dev_area_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(card_w);
                            ui.spacing_mut().item_spacing.y = 4.0;
                            for (dev, is_local) in &devices {
                                draw_online_device_row(
                                    ui,
                                    p,
                                    dev,
                                    *is_local,
                                    at_limit || login_rejected,
                                    data.fetched_at_unix,
                                    now_unix,
                                );
                            }
                        });
                }
            },
        );

        // 3. 底部固定 22px 并发余量说明 + 悬停查看近期记录轻入口（保证与左卡底边 0.0px 齐平）
        ui.add_space(4.0);
        let sep = ui.allocate_space(Vec2::new(card_w, 1.0)).1;
        ui.painter().line_segment(
            [sep.left_center(), sep.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );
        ui.add_space(3.0);

        let (summary_text, summary_color) = if login_rejected && count > 0 {
            (
                "认证被拒 · 检测到占线设备，请检查并发上限".to_string(),
                p.danger,
            )
        } else if let Some(max) = data.max_online_devices {
            let max_u = max as usize;
            let rem = max_u.saturating_sub(count);
            if count >= max_u {
                (
                    format!("已达并发上限（{count}/{max}），再登新设备可能挤线"),
                    p.warn,
                )
            } else if other_count > 0 {
                (
                    format!("含 {other_count} 台其他终端并发 · 还可再连 {rem} 台"),
                    p.text_secondary,
                )
            } else if count == 1 && local_present {
                (
                    format!("仅本机在线 · 同账号还可同时连 {rem} 台终端"),
                    p.text_muted,
                )
            } else if count == 0 {
                (
                    format!("当前无在线终端 · 最多支持 {max} 台并发登录"),
                    p.text_muted,
                )
            } else {
                (
                    format!("当前 {count} 台在线 · 还可再连 {rem} 台"),
                    p.text_muted,
                )
            }
        } else if other_count > 0 {
            (
                format!("含 {other_count} 台其他终端并发在线"),
                p.text_secondary,
            )
        } else if count == 1 && local_present {
            (
                "仅本机在线 · 同一学号支持多运营商并发".to_string(),
                p.text_muted,
            )
        } else {
            ("认证成功后自动同步当前在线终端".to_string(), p.text_muted)
        };

        // 纯 Painter 精确 16px 页脚，杜绝 egui::Label 行高在 100%/125% DPI 下撑高右卡
        let (foot_rect, foot_resp) =
            ui.allocate_exact_size(Vec2::new(card_w, 16.0), Sense::hover());
        let foot_cy = foot_rect.center().y;
        let mut right_limit = foot_rect.right();
        if !data.recent_sessions.is_empty() {
            let link_g = ui.painter().layout_no_wrap(
                "近期记录".to_string(),
                egui::FontId::proportional(10.0),
                p.accent,
            );
            let link_rect = Rect::from_min_max(
                Pos2::new(foot_rect.right() - link_g.size().x - 2.0, foot_rect.top()),
                foot_rect.max,
            );
            let link_resp = ui.interact(
                link_rect,
                ui.id().with("recent_sessions_hover"),
                Sense::hover(),
            );
            ui.painter().galley(
                Pos2::new(
                    foot_rect.right() - link_g.size().x,
                    foot_cy - link_g.size().y * 0.5,
                ),
                link_g,
                p.accent,
            );
            link_resp.on_hover_ui(|ui| {
                draw_recent_sessions_full_tooltip(ui, p, data);
            });
            right_limit = link_rect.left() - 8.0;
        }
        let sum_clip = Rect::from_min_max(
            foot_rect.min,
            Pos2::new(right_limit.max(foot_rect.left()), foot_rect.bottom()),
        );
        let sum_g = ui.painter().layout_no_wrap(
            summary_text.clone(),
            egui::FontId::proportional(10.0),
            summary_color,
        );
        ui.painter().with_clip_rect(sum_clip).galley(
            Pos2::new(foot_rect.left(), foot_cy - sum_g.size().y * 0.5),
            sum_g,
            summary_color,
        );
        if foot_resp.hovered()
            && !ui.rect_contains_pointer(Rect::from_min_max(
                Pos2::new(right_limit, foot_rect.top()),
                foot_rect.max,
            ))
        {
            foot_resp.on_hover_text(&summary_text);
        }
    });

    refresh_clicked
}

fn draw_empty_devices_box(ui: &mut egui::Ui, p: &Palette, width: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(7),
        p.subtle,
        Stroke::new(1.0_f32, p.divider),
        StrokeKind::Inside,
    );
    let cy = rect.center().y;
    ui.painter()
        .circle_filled(Pos2::new(rect.left() + 14.0, cy - 7.5), 3.0, p.neutral);
    ui.painter().text(
        Pos2::new(rect.left() + 24.0, cy - 7.5),
        egui::Align2::LEFT_CENTER,
        "当前账号暂无在线终端会话",
        egui::FontId::new(11.2, crate::ui::theme::bold_family()),
        p.text_secondary,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 24.0, cy + 9.0),
        egui::Align2::LEFT_CENTER,
        "连入并认证校园网后，将自动展示各终端 IP 与上线时长",
        egui::FontId::proportional(10.0),
        p.text_muted,
    );
}

/// 仅 1 台终端在线时的舒展双行卡片（充分利用空间，杜绝空洞感）。
#[allow(clippy::too_many_arguments)]
fn draw_featured_device_card(
    ui: &mut egui::Ui,
    p: &Palette,
    dev: &OnlineDevice,
    is_local: bool,
    highlight_conflict: bool,
    fetched_at_unix: i64,
    now_unix: i64,
    width: f32,
    height: f32,
) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let (fill, stroke) = if resp.hovered() {
        (p.card_hover, Stroke::new(1.0_f32, p.border_strong))
    } else if !is_local && highlight_conflict {
        (p.warn_soft, Stroke::new(1.0_f32, p.warn_border))
    } else {
        (p.subtle, Stroke::new(1.0_f32, p.divider))
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(7),
        fill,
        stroke,
        StrokeKind::Inside,
    );

    let painter = ui.painter();
    let top_y = rect.top() + height * 0.34;
    let bot_y = rect.top() + height * 0.72;
    let mut x = rect.left() + 11.0;

    let dot_c = if is_local {
        p.ok
    } else if highlight_conflict {
        p.warn
    } else {
        p.accent
    };
    painter.circle_filled(Pos2::new(x + 3.0, top_y), 3.3, dot_c);
    x += 12.0;

    if !dev.terminal_type.is_empty() {
        x = paint_mini_pill(
            painter,
            x,
            top_y,
            34.0,
            &dev.terminal_type,
            p.text_secondary,
            p.card,
            p.border,
        ) + 7.0;
    }

    // 统一使用微软雅黑字族绘制 IP 与中文，确保数字、图标与汉字基线严格对齐
    let ip_galley = painter.layout_no_wrap(
        dev.ip.clone(),
        egui::FontId::new(11.8, crate::ui::theme::bold_family()),
        p.text,
    );
    painter.galley(
        Pos2::new(x, top_y - ip_galley.size().y * 0.5),
        ip_galley.clone(),
        p.text,
    );
    x += ip_galley.size().x + 7.0;

    let (id_lbl, id_fg, id_bg, id_bd) = if is_local {
        ("本机", p.ok, p.ok_soft, p.ok_border)
    } else if highlight_conflict {
        ("占线终端", p.warn, p.warn_soft, p.warn_border)
    } else {
        (
            "其他终端",
            p.text_secondary,
            p.neutral_soft,
            p.neutral_border,
        )
    };
    let _ = paint_mini_pill(painter, x, top_y, 34.0, id_lbl, id_fg, id_bg, id_bd);

    let elapsed_secs = device_elapsed_secs(dev, fetched_at_unix, now_unix);
    if elapsed_secs > 0 {
        let dur_g = painter.layout_no_wrap(
            format!("已在线 {}", format_duration_short(elapsed_secs)),
            egui::FontId::new(10.8, crate::ui::theme::bold_family()),
            p.text_secondary,
        );
        painter.galley(
            Pos2::new(
                rect.right() - 11.0 - dur_g.size().x,
                top_y - dur_g.size().y * 0.5,
            ),
            dur_g,
            p.text_secondary,
        );
    }

    let start_hm = device_start_hm(dev);
    let sub_text = if is_local {
        format!("{start_hm} 上线 · 自助服务已核验本机活跃会话")
    } else if highlight_conflict {
        format!("{start_hm} 上线 · 其他终端占线，若登录被拒请排查并发名额")
    } else {
        format!("{start_hm} 上线 · 同账号另一终端当前在线")
    };
    let sub_g = painter.layout_no_wrap(sub_text, egui::FontId::proportional(10.0), p.text_muted);
    painter.galley(
        Pos2::new(rect.left() + 23.0, bot_y - sub_g.size().y * 0.5),
        sub_g,
        p.text_muted,
    );

    if resp.hovered() {
        resp.on_hover_ui(|ui| {
            draw_device_detail_tooltip(ui, p, dev, is_local, elapsed_secs);
        });
    }
}

fn draw_devices_placeholder(
    ui: &mut egui::Ui,
    p: &Palette,
    self_service: &SelfServiceState,
    local_campus_ip: &str,
    width: f32,
    height: f32,
) {
    egui::Frame::new()
        .fill(p.subtle)
        .stroke(Stroke::new(1.0_f32, p.divider))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            let iw = (width - 26.0).max(120.0);
            let ih = (height - 20.0).max(52.0);
            ui.set_min_size(Vec2::new(iw, ih));
            ui.set_max_size(Vec2::new(iw, ih));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                if !local_campus_ip.is_empty() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (dr, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                        ui.painter().circle_filled(dr.center(), 3.2, p.ok);
                        ui.label(body_text("本机校园网 IP", 10.8, p.text_secondary));
                        ui.label(bold_text(local_campus_ip, 11.2, p.text));
                    });
                }
                let hint = match self_service.status {
                    SelfServiceStatus::Refreshing => "正在向校园网自助服务拉取在线终端列表…",
                    SelfServiceStatus::CaptchaCooldown => self_service.message.as_str(),
                    SelfServiceStatus::NoCredentials => "请先在设置中保存校园网学号与密码",
                    _ if !self_service.message.is_empty() => self_service.message.as_str(),
                    _ => "连入校园网内网后，将自动同步当前在线终端与并发配额",
                };
                ui.add(egui::Label::new(body_text(hint, 10.4, p.text_muted)).wrap());
            });
        });
}

fn draw_online_device_row(
    ui: &mut egui::Ui,
    p: &Palette,
    dev: &OnlineDevice,
    is_local: bool,
    highlight_conflict: bool,
    fetched_at_unix: i64,
    now_unix: i64,
) {
    let row_w = ui.available_width();
    let row_h = 27.0_f32;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(row_w, row_h), Sense::hover());
    let (fill, stroke) = if resp.hovered() {
        (p.card_hover, Stroke::new(1.0_f32, p.border_strong))
    } else if !is_local && highlight_conflict {
        (p.warn_soft, Stroke::new(1.0_f32, p.warn_border))
    } else {
        (p.subtle, Stroke::new(1.0_f32, p.divider))
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        fill,
        stroke,
        StrokeKind::Inside,
    );

    let cy = rect.center().y;
    let painter = ui.painter();

    // 固定栅格列坐标：保证多行间圆点、终端标签、IP 数字、身份徽章与右侧时刻/时长纵向严格对齐
    let dot_c = if is_local {
        p.ok
    } else if highlight_conflict {
        p.warn
    } else {
        p.accent
    };
    painter.circle_filled(Pos2::new(rect.left() + 12.0, cy), 3.0, dot_c);

    let term_x = rect.left() + 21.0;
    let term_col_w = 34.0_f32;
    let term_label = if dev.terminal_type.is_empty() {
        "终端"
    } else {
        dev.terminal_type.as_str()
    };
    let _ = paint_mini_pill(
        painter,
        term_x,
        cy,
        term_col_w,
        term_label,
        p.text_secondary,
        p.card,
        p.border,
    );

    let ip_x = term_x + term_col_w + 7.0;
    let ip_galley = painter.layout_no_wrap(
        dev.ip.clone(),
        egui::FontId::new(11.2, crate::ui::theme::bold_family()),
        p.text,
    );
    painter.galley(
        Pos2::new(ip_x, cy - ip_galley.size().y * 0.5),
        ip_galley.clone(),
        p.text,
    );

    // 固定 IP 列宽（90px，足以容纳最长 IPv4），保证每行「本机 / 其他」徽章严格竖直齐平
    let badge_x = ip_x + ip_galley.size().x.max(90.0) + 6.0;
    let (id_lbl, id_fg, id_bg, id_bd) = if is_local {
        ("本机", p.ok, p.ok_soft, p.ok_border)
    } else if highlight_conflict {
        ("占线", p.warn, p.warn_soft, p.warn_border)
    } else {
        ("其他", p.text_secondary, p.neutral_soft, p.neutral_border)
    };
    let _ = paint_mini_pill(painter, badge_x, cy, 34.0, id_lbl, id_fg, id_bg, id_bd);

    // 右侧分列对齐：「06:29 上线」 + 固定居中「·」 + 固定右列「42m46s / 1h30m」
    let start_hm = device_start_hm(dev);
    let elapsed_secs = device_elapsed_secs(dev, fetched_at_unix, now_unix);
    let meta_font = egui::FontId::proportional(10.2);
    let right_edge = rect.right() - 10.0;

    if elapsed_secs > 0 {
        let dur_col_w = 40.0_f32;
        let dur_x = right_edge - dur_col_w;
        let dur_g = painter.layout_no_wrap(
            format_duration_short(elapsed_secs),
            meta_font.clone(),
            p.text_muted,
        );
        painter.galley(
            Pos2::new(dur_x, cy - dur_g.size().y * 0.5),
            dur_g,
            p.text_muted,
        );

        let dot_cx = dur_x - 5.0;
        let dot_g = painter.layout_no_wrap("·".to_string(), meta_font.clone(), p.text_muted);
        painter.galley(
            Pos2::new(dot_cx - dot_g.size().x * 0.5, cy - dot_g.size().y * 0.5),
            dot_g,
            p.text_muted,
        );

        let time_txt = if row_w >= 316.0 {
            format!("{start_hm} 上线")
        } else {
            start_hm.to_string()
        };
        let time_g = painter.layout_no_wrap(time_txt, meta_font, p.text_muted);
        painter.galley(
            Pos2::new(dot_cx - 5.0 - time_g.size().x, cy - time_g.size().y * 0.5),
            time_g,
            p.text_muted,
        );
    } else {
        let time_g = painter.layout_no_wrap(format!("{start_hm} 上线"), meta_font, p.text_muted);
        painter.galley(
            Pos2::new(right_edge - time_g.size().x, cy - time_g.size().y * 0.5),
            time_g,
            p.text_muted,
        );
    }

    if resp.hovered() {
        resp.on_hover_ui(|ui| {
            draw_device_detail_tooltip(ui, p, dev, is_local, elapsed_secs);
        });
    }
}

fn device_start_hm(dev: &OnlineDevice) -> &str {
    dev.login_time
        .split_whitespace()
        .nth(1)
        .and_then(|t| t.get(..5))
        .unwrap_or("--:--")
}

fn device_elapsed_secs(dev: &OnlineDevice, fetched_at_unix: i64, now_unix: i64) -> u64 {
    if let Some(use_secs) = dev.use_time_secs {
        use_secs.saturating_add((now_unix - fetched_at_unix).max(0) as u64)
    } else if let Some(ts) = dev.login_unix {
        (now_unix - ts).max(0) as u64
    } else {
        0
    }
}

fn draw_device_detail_tooltip(
    ui: &mut egui::Ui,
    p: &Palette,
    dev: &OnlineDevice,
    is_local: bool,
    elapsed_secs: u64,
) {
    ui.set_min_width(214.0);
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.label(bold_text(
        if is_local {
            "当前本机在线会话"
        } else {
            "同账号其他在线终端"
        },
        11.5,
        p.text,
    ));
    detail_kv_row(ui, p, "校园网 IP", &dev.ip);
    if !dev.terminal_type.is_empty() {
        detail_kv_row(ui, p, "终端类型", &dev.terminal_type);
    }
    detail_kv_row(ui, p, "上线时间", &dev.login_time);
    if elapsed_secs > 0 {
        detail_kv_row(ui, p, "已在线时长", &format_duration_short(elapsed_secs));
    }
}

fn draw_recent_sessions_full_tooltip(ui: &mut egui::Ui, p: &Palette, data: &SelfServiceData) {
    ui.set_min_width(276.0);
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.label(bold_text(
        "门户近期上网记录（含运营商实际流量）",
        11.5,
        p.text,
    ));
    for rec in &data.recent_sessions {
        let start = format_hm(rec.login_unix_ms / 1000);
        let end = rec
            .logout_unix_ms
            .map(|ms| format_hm(ms / 1000))
            .unwrap_or_else(|| "在线".into());
        detail_kv_row(
            ui,
            p,
            &format!("{start}–{end} · {}", rec.ip),
            &format!(
                "{} · {}",
                format_duration_short(rec.duration_secs),
                format_mb_compact(rec.actual_total_mb())
            ),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_mini_pill(
    painter: &egui::Painter,
    left: f32,
    cy: f32,
    min_width: f32,
    text: &str,
    fg: Color32,
    bg: Color32,
    border: Color32,
) -> f32 {
    let font = egui::FontId::new(9.5, crate::ui::theme::bold_family());
    let g = painter.layout_no_wrap(text.to_string(), font, fg);
    let w = (g.size().x + 10.0).max(min_width);
    let rect = Rect::from_min_size(Pos2::new(left, cy - 8.5), Vec2::new(w, 17.0));
    painter.rect(
        rect,
        CornerRadius::same(4),
        bg,
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );
    painter.galley(
        Pos2::new(
            rect.center().x - g.size().x * 0.5,
            rect.center().y - g.size().y * 0.5,
        ),
        g,
        fg,
    );
    rect.right()
}

// ── Tier 3：通栏纤薄「连接稳定性」胶囊时间轴 ────────────────────────────

/// 位于双卡之下、运行日志之上的通栏纤薄稳定性条（~78px 高，悬停黄格即看完整断联卡片）。
pub fn draw_stability_strip(
    ui: &mut egui::Ui,
    p: &Palette,
    all_points: &[TelemetryPoint],
    range: &mut TimeRange,
) {
    let now = Local::now().timestamp();
    let points = filter_range(all_points, now, *range);
    let stats = compute_stats(&points);
    let outages = outages_in_range(all_points, now, *range);
    let track = build_health_track(all_points, now, *range);
    let has_outages = !outages.events.is_empty();

    let shadow = match p.mode {
        ThemeMode::Light => egui::Shadow {
            offset: [0, 1],
            blur: 4,
            spread: 0,
            color: p.shadow,
        },
        ThemeMode::Dark => egui::Shadow::NONE,
    };

    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(RADIUS_MD))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .shadow(shadow)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let card_w = ui.available_width();

            // 1. 单行紧凑标题 + 在线率 + 主因摘要 + 图例 + 1h/7h/24h（26px 与分段器等高）
            ui.allocate_ui_with_layout(
                Vec2::new(card_w, 26.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    section_heading(ui, Icon::Shield, p.ok, "连接稳定性", p.text, 26.0);

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
                            14.5,
                            rate_color,
                        ));
                    } else {
                        ui.label(bold_text("—", 14.5, p.text_muted));
                    }

                    let summary = if let Some(primary) = outages.primary_reason() {
                        format!(
                            "在线率 · {} 次断联（主因：{}，悬停黄格看详情）",
                            outages.events.len(),
                            reason_label(primary)
                        )
                    } else if stats.check_count > 0 {
                        format!("在线率 · 过去 {} 全程稳定", range.label())
                    } else {
                        "等待巡检采样".to_string()
                    };

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let mut idx = range.index();
                        let labels = ["1h", "7h", "24h"];
                        if segmented_control(ui, p, "stab_strip_range_seg", &mut idx, &labels) {
                            *range = TimeRange::from_index(idx);
                        }
                        if card_w >= 620.0 {
                            // right_to_left 容器中倒序放置，最终从左到右呈现：「● 在线  ● 断联  ○ 未运行」
                            legend_dot_item(ui, p, None, "未运行");
                            legend_dot_item(ui, p, Some(p.warn), "断联");
                            legend_dot_item(
                                ui,
                                p,
                                Some(Color32::from_rgb(0x10, 0xB9, 0x81)),
                                "在线",
                            );
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(body_text(&summary, 10.3, p.text_muted))
                                    .truncate(),
                            )
                            .on_hover_text(&summary);
                        });
                    });
                },
            );

            ui.add_space(4.0);

            // 2. 通栏纤薄胶囊时间轴 + 真实时刻标尺（30px）
            draw_capsule_timeline(ui, p, &track, &points, &outages.events, now, *range);
        });
}

fn format_uptime_pct(pct: f64, has_outages: bool) -> String {
    if !has_outages && pct >= 99.95 {
        "100%".to_string()
    } else {
        let capped = if has_outages { pct.min(99.9) } else { pct };
        format!("{capped:.1}%")
    }
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

/// 精致纤细胶囊时间轴：
/// - 24h = 96 枚细胶囊（15m/枚）、7h = 84 枚（5m/枚）、1h = 90 枚（40s/枚）
/// - 竖向修长胶囊比例（约 5.5–6.5px 宽 × 14px 高），悬停黄色/红色胶囊即弹出完整断联详情小卡
fn draw_capsule_timeline(
    ui: &mut egui::Ui,
    p: &Palette,
    segments: &[TrackSegment],
    points: &[&TelemetryPoint],
    outages: &[OutageEvent<'_>],
    now: i64,
    range: TimeRange,
) {
    let total_w = ui.available_width();
    let window_secs = range.secs().max(60);
    let window_start = now - window_secs;

    let slot_count: usize = match range {
        TimeRange::H24 => 96,
        TimeRange::H7 => 84,
        TimeRange::H1 => 90,
    };
    let slot_secs = window_secs as f64 / slot_count as f64;

    let top_lift = 2.0_f32;
    let bar_h = 14.0_f32;
    let ruler_gap = 3.0_f32;
    let ruler_h = 13.0_f32;
    let total_h = top_lift + bar_h + ruler_gap + ruler_h;

    let (full_rect, _) = ui.allocate_exact_size(Vec2::new(total_w, total_h), Sense::hover());
    let bar_rect = Rect::from_min_size(
        Pos2::new(full_rect.left(), full_rect.top() + top_lift),
        Vec2::new(total_w, bar_h),
    );
    let ruler_top = bar_rect.bottom() + ruler_gap;
    let painter = ui.painter();

    let gap = if total_w < 540.0 { 2.0_f32 } else { 2.8_f32 };
    let slot_w = ((total_w - gap * (slot_count as f32 - 1.0)) / slot_count as f32).max(2.0);
    let pill_r = CornerRadius::same((slot_w * 0.45).clamp(2.0, 3.0).round() as u8);
    let online_green = Color32::from_rgb(0x10, 0xB9, 0x81);

    struct HoveredSlotInfo {
        start_ts: i64,
        end_ts: i64,
        outage_indices: Vec<usize>,
        is_online: bool,
        checks: usize,
        online_checks: usize,
        avg_ms: Option<u64>,
    }

    let mut hovered_slot_info: Option<HoveredSlotInfo> = None;

    for idx in 0..slot_count {
        let s_start = window_start + (idx as f64 * slot_secs).round() as i64;
        let s_end = window_start + ((idx + 1) as f64 * slot_secs).round() as i64;

        let mut slot_outages: Vec<usize> = Vec::new();
        let mut has_unrecovered = false;
        for (ev_idx, ev) in outages.iter().enumerate() {
            let ev_start = ev.start.ts;
            let ev_end = ev.end_ts.unwrap_or(now).max(ev_start + 1);
            if ev_start < s_end && ev_end > s_start {
                slot_outages.push(ev_idx);
                if !ev.recovered {
                    has_unrecovered = true;
                }
            }
        }

        let has_online_track = segments.iter().any(|seg| {
            seg.state == TrackState::Online && seg.start_ts < s_end && seg.end_ts > s_start
        });

        let mut checks = 0usize;
        let mut online_checks = 0usize;
        let mut ms_sum = 0u64;
        let mut ms_cnt = 0u64;
        for pt in points {
            if pt.event == "check" && pt.ts >= s_start && pt.ts < s_end {
                checks += 1;
                if pt.online {
                    online_checks += 1;
                    if pt.ms > 0 {
                        ms_sum += pt.ms;
                        ms_cnt += 1;
                    }
                }
            }
        }
        let avg_ms = (ms_cnt > 0).then(|| ms_sum / ms_cnt);
        let is_online = has_online_track || online_checks > 0;
        let has_outage = !slot_outages.is_empty();

        let x0 = bar_rect.left() + idx as f32 * (slot_w + gap);
        let hit_rect = Rect::from_min_size(
            Pos2::new(x0 - gap * 0.5, full_rect.top()),
            Vec2::new(slot_w + gap, bar_h + top_lift + 2.0),
        );
        let is_hovered = ui.rect_contains_pointer(hit_rect);

        if is_hovered {
            hovered_slot_info = Some(HoveredSlotInfo {
                start_ts: s_start,
                end_ts: s_end,
                outage_indices: slot_outages.clone(),
                is_online,
                checks,
                online_checks,
                avg_ms,
            });
        }

        let (cell_rect, fill) = if has_outage {
            let color = if has_unrecovered { p.danger } else { p.warn };
            let lift = if is_hovered { top_lift } else { 0.0 };
            let r = Rect::from_min_max(
                Pos2::new(x0, bar_rect.top() - lift),
                Pos2::new(x0 + slot_w, bar_rect.bottom()),
            );
            (r, color)
        } else if is_online {
            let lift = if is_hovered { 1.5 } else { 0.0 };
            let r = Rect::from_min_max(
                Pos2::new(x0, bar_rect.top() + 0.5 - lift),
                Pos2::new(x0 + slot_w, bar_rect.bottom()),
            );
            let fill = if is_hovered {
                online_green
            } else {
                with_alpha(online_green, 205)
            };
            (r, fill)
        } else {
            let idle_h = (bar_h * 0.45).max(5.0);
            let r = Rect::from_min_max(
                Pos2::new(x0, bar_rect.bottom() - idle_h),
                Pos2::new(x0 + slot_w, bar_rect.bottom()),
            );
            let fill = match p.mode {
                ThemeMode::Light => {
                    if is_hovered {
                        Color32::from_rgb(0xB8, 0xC2, 0xD1)
                    } else {
                        Color32::from_rgb(0xDC, 0xE2, 0xEB)
                    }
                }
                ThemeMode::Dark => {
                    if is_hovered {
                        with_alpha(p.border_strong, 175)
                    } else {
                        with_alpha(p.border_strong, 85)
                    }
                }
            };
            (r, fill)
        };

        painter.rect_filled(cell_rect, pill_r, fill);
        if is_hovered {
            let outline = if has_outage {
                if has_unrecovered {
                    p.danger
                } else {
                    p.warn
                }
            } else {
                p.text
            };
            painter.rect_stroke(
                cell_rect.expand(0.8),
                pill_r,
                Stroke::new(1.2_f32, outline),
                StrokeKind::Outside,
            );
        }
    }

    if let Some(info) = hovered_slot_info {
        if !info.outage_indices.is_empty() {
            let matched: Vec<&OutageEvent<'_>> = info
                .outage_indices
                .iter()
                .rev()
                .filter_map(|&idx| outages.get(idx))
                .collect();
            if !matched.is_empty() {
                show_slot_outages_tooltip(ui, p, ui.id().with("capsule_outage_tip"), &matched);
            }
        } else {
            egui::show_tooltip(
                ui.ctx(),
                ui.layer_id(),
                ui.id().with("capsule_slot_tip"),
                |ui| {
                    ui.set_max_width(220.0);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (dr, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                        if info.is_online {
                            ui.painter().circle_filled(dr.center(), 3.2, online_green);
                            ui.label(bold_text("校园网在线", 11.5, p.text));
                        } else {
                            ui.painter().circle_stroke(
                                dr.center(),
                                3.0,
                                Stroke::new(1.2_f32, p.text_muted),
                            );
                            ui.label(bold_text("守护未运行 / 休眠", 11.5, p.text_secondary));
                        }
                    });
                    ui.label(body_text(
                        format!(
                            "时段：{} – {}",
                            format_hm(info.start_ts),
                            format_hm(info.end_ts)
                        ),
                        10.8,
                        p.text_secondary,
                    ));
                    if info.checks > 0 {
                        let mut detail =
                            format!("巡检正常 {}/{} 次", info.online_checks, info.checks);
                        if let Some(ms) = info.avg_ms {
                            detail.push_str(&format!(" · 平均网关 {ms} ms"));
                        }
                        ui.label(body_text(detail, 10.3, p.text_muted));
                    }
                },
            );
        }
    }

    // 真实时钟标尺
    let ts_to_x = |ts: i64| -> f32 {
        let frac = ((ts - window_start) as f32 / window_secs as f32).clamp(0.0, 1.0);
        bar_rect.left() + frac * bar_rect.width()
    };
    painter.line_segment(
        [
            Pos2::new(bar_rect.left(), ruler_top),
            Pos2::new(bar_rect.right(), ruler_top),
        ],
        Stroke::new(1.0_f32, p.divider),
    );
    let tick_stroke = Stroke::new(1.0_f32, with_alpha(p.text_muted, 130));
    let tick_font = egui::FontId::proportional(9.2);
    let label_y = ruler_top + 9.0;

    painter.line_segment(
        [
            Pos2::new(bar_rect.left(), ruler_top),
            Pos2::new(bar_rect.left(), ruler_top + 3.0),
        ],
        tick_stroke,
    );
    painter.text(
        Pos2::new(bar_rect.left(), label_y),
        egui::Align2::LEFT_CENTER,
        format_hm(window_start),
        tick_font.clone(),
        p.text_muted,
    );

    painter.line_segment(
        [
            Pos2::new(bar_rect.right(), ruler_top),
            Pos2::new(bar_rect.right(), ruler_top + 3.0),
        ],
        tick_stroke,
    );
    painter.text(
        Pos2::new(bar_rect.right(), label_y),
        egui::Align2::RIGHT_CENTER,
        "现在",
        egui::FontId::new(9.2, crate::ui::theme::bold_family()),
        p.text_secondary,
    );

    let step_secs: i64 = match range {
        TimeRange::H1 => 15 * 60,
        TimeRange::H7 => 2 * 3600,
        TimeRange::H24 => 6 * 3600,
    };
    let mut last_tick_x = bar_rect.left() + 44.0;
    let right_limit = bar_rect.right() - 42.0;
    for tick_ts in aligned_clock_ticks(window_start, now, step_secs) {
        let tx = ts_to_x(tick_ts);
        if tx < last_tick_x || tx > right_limit {
            continue;
        }
        painter.line_segment(
            [Pos2::new(tx, ruler_top), Pos2::new(tx, ruler_top + 3.0)],
            tick_stroke,
        );
        painter.text(
            Pos2::new(tx, label_y),
            egui::Align2::CENTER_CENTER,
            format_hm(tick_ts),
            tick_font.clone(),
            p.text_muted,
        );
        last_tick_x = tx + 46.0;
    }
}

/// 悬停断联胶囊时弹出的结构化详情卡片（若同一时间槽内有多起断联，在浮层中全部列出）。
fn show_slot_outages_tooltip(
    ui: &egui::Ui,
    p: &Palette,
    id: egui::Id,
    events: &[&OutageEvent<'_>],
) {
    let first = events[0];
    let dot_color = if !first.recovered {
        p.danger
    } else {
        reason_severity_color(p, &first.start.reason)
    };
    egui::show_tooltip(ui.ctx(), ui.layer_id(), id, |ui| {
        ui.set_min_width(242.0);
        ui.set_max_width(278.0);
        ui.spacing_mut().item_spacing.y = 5.0;

        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (dr, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                ui.painter().circle_filled(dr.center(), 3.3, dot_color);
                ui.label(bold_text(reason_label(&first.start.reason), 12.0, p.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if first.recovered {
                        let tag = if first.auto_reconnected {
                            "自动重连"
                        } else {
                            "已恢复"
                        };
                        badge(ui, p.ok, p.ok_soft, p.ok_border, tag);
                    } else {
                        badge(ui, p.danger, p.danger_soft, p.danger_border, "离线中");
                    }
                });
            },
        );

        let sep1 = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
        ui.painter().line_segment(
            [sep1.left_center(), sep1.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );

        detail_kv_row(ui, p, "断联时间", &format_md_hms(first.start.ts));
        let recover_str = match first.end_ts {
            Some(ts) => format_md_hms(ts),
            None => "尚未恢复".to_string(),
        };
        detail_kv_row(ui, p, "恢复时间", &recover_str);
        if let Some(secs) = first.duration_secs {
            let mode = if !first.recovered {
                "仍处于离线"
            } else if first.auto_reconnected {
                "守护自动重登"
            } else {
                "链路自行恢复"
            };
            detail_kv_row(
                ui,
                p,
                "持续时长",
                &format!("{}（{mode}）", format_duration_short(secs)),
            );
        }
        detail_kv_row(ui, p, "诊断代码", &first.start.reason);

        let sep2 = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
        ui.painter().line_segment(
            [sep2.left_center(), sep2.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );

        ui.label(body_text(
            reason_hint(&first.start.reason),
            10.5,
            p.text_muted,
        ));

        if events.len() > 1 {
            let sep3 = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
            ui.painter().line_segment(
                [sep3.left_center(), sep3.right_center()],
                Stroke::new(1.0_f32, p.divider),
            );
            ui.label(bold_text(
                format!("该时段内其余 {} 次断联", events.len() - 1),
                10.3,
                p.warn,
            ));
            for extra in &events[1..events.len().min(5)] {
                let dur = extra
                    .duration_secs
                    .map(format_duration_short)
                    .unwrap_or_else(|| "离线中".into());
                detail_kv_row(
                    ui,
                    p,
                    &format!(
                        "{} {}",
                        format_hm(extra.start.ts),
                        reason_label(&extra.start.reason)
                    ),
                    &dur,
                );
            }
        }
    });
}

fn detail_kv_row(ui: &mut egui::Ui, p: &Palette, key: &str, value: &str) {
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 15.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(body_text(key, 10.5, p.text_muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(bold_text(value, 10.8, p.text_secondary));
            });
        },
    );
}

fn format_md_hms(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "--".to_string())
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

fn format_hm(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".to_string())
}

fn format_sync_age(secs: i64) -> String {
    if secs < 45 {
        "刚刚同步".to_string()
    } else if secs < 3600 {
        format!("{} 分钟前", secs / 60)
    } else {
        format!("{} 小时前", secs / 3600)
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
            format!("{m}m{s}s")
        }
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn format_mb_compact(mb: f64) -> String {
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

fn format_mb_ring(mb: f64) -> String {
    let mb = if mb.is_finite() && mb > 0.0 { mb } else { 0.0 };
    if mb >= 1024.0 * 1024.0 {
        format!("{:.2}T", mb / (1024.0 * 1024.0))
    } else if mb >= 1024.0 {
        let gb = mb / 1024.0;
        if gb >= 10.0 {
            format!("{gb:.1}G")
        } else {
            format!("{gb:.2}G")
        }
    } else if mb >= 10.0 {
        format!("{mb:.0}M")
    } else if mb > 0.0 {
        format!("{mb:.1}M")
    } else {
        "0M".to_string()
    }
}

pub fn format_bytes_compact(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        let gb = b / GB;
        if gb >= 100.0 {
            format!("{gb:.0} GB")
        } else {
            format!("{gb:.1} GB")
        }
    } else if b >= MB {
        let mb = b / MB;
        if mb >= 100.0 {
            format!("{mb:.0} MB")
        } else {
            format!("{mb:.1} MB")
        }
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else if bytes == 0 {
        "0 MB".to_string()
    } else {
        format!("{bytes} B")
    }
}

fn format_bytes_ring(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1}G", b / GB)
    } else if b >= MB {
        format!("{:.0}M", b / MB)
    } else if bytes > 0 {
        "<1M".to_string()
    } else {
        "0M".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes_mb_and_durations_compactly() {
        assert_eq!(format_bytes_compact(0), "0 MB");
        assert_eq!(format_bytes_compact(512 * 1024), "512 KB");
        assert_eq!(format_bytes_compact(12 * 1024 * 1024), "12.0 MB");
        assert_eq!(format_bytes_compact(256 * 1024 * 1024), "256 MB");
        assert_eq!(format_bytes_compact(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(format_mb_compact(0.0), "0 MB");
        assert_eq!(format_mb_compact(535.203), "535 MB");
        assert_eq!(format_mb_compact(553_035.463), "540.1 GB");
        assert_eq!(format_mb_compact(365_007.37), "356.5 GB");
        assert_eq!(format_mb_ring(365_885.566), "357.3G");
        assert_eq!(format_duration_short(8), "8s");
        assert_eq!(format_duration_short(125), "2m5s");
        assert_eq!(format_duration_short(3660), "1h01m");
    }
}
