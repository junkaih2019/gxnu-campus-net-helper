//! 主页核心可视化模块（v0.2.1）：
//! - Tier 2：双栏自助服务卡片（左 47%「账号流量」自适应运营商/校园网双视角 + 右 53%「在线设备与记录」含并发配额与真实流量历史）
//! - Tier 3：通栏紧凑「连接稳定性」胶囊时间轴（位于双卡之下、运行日志之上，悬停黄格即看完整断联卡片）

use chrono::{Local, TimeZone, Timelike};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::metrics_store::{
    build_health_track, compute_stats, filter_range, outages_in_range, OutageEvent, TelemetryPoint,
    TimeRange, TrackSegment, TrackState,
};
use crate::models::{
    OnlineDevice, RecentSession, SelfServiceData, SelfServiceState, SelfServiceStatus,
    TrafficSummary,
};
use crate::ui::icons::Icon;
use crate::ui::theme::{
    body_text, bold_text, mono_text, reason_hint, reason_label, with_alpha, Palette,
    DEFAULT_TRAFFIC_SPLIT,
};
use crate::ui::widgets::{
    badge, card_padded, icon_button, section_heading, segmented_control, vertical_splitter,
};

/// 账号流量卡展示视角：
/// - `Carrier`：运营商出口视角（主显本月全账号实际流量 + 上下行比例，次行附校园网免费额度）
/// - `Campus`：校园用户出口视角（主显校园网剩余额度 + 进度条，次行附全账号实际流量）
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

/// Tier 2：账号流量（左） + 在线设备与记录（右）双栏卡片（可左右拖拽调整比例）。
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
    let min_left = 250.0_f32.min(usable_w * 0.44);
    let min_right = 260.0_f32.min(usable_w * 0.46);
    let left_w = (usable_w * (*split_ratio).clamp(0.36, 0.64))
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
            *split_ratio = ((left_w + dx) / usable_w).clamp(0.36, 0.64);
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

/// 左卡：账号流量（融合校园网自助服务双口径 + 本机今日物理网卡参考）。
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
        let inner_h = (target_height - 26.0).max(148.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);
        ui.spacing_mut().item_spacing.y = 0.0;

        let card_w = ui.available_width();
        let has_data = self_service.data.is_some();

        // 1. 顶部标题行（26px）
        ui.allocate_ui_with_layout(
            Vec2::new(card_w, 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                section_heading(ui, Icon::ArrowUpDown, p.accent, "账号流量", p.text, 26.0);

                if card_w >= 305.0 {
                    let (lbl, fg, bg, bd, tip) = match (self_service.status, has_data) {
                        (SelfServiceStatus::Refreshing, _) => (
                            "同步中",
                            p.accent,
                            p.accent_soft,
                            p.accent_border,
                            "正在向校园网自助服务同步最新数据",
                        ),
                        (_, true) if *view_mode == TrafficViewMode::Carrier => (
                            "运营商不限量",
                            p.ok,
                            p.ok_soft,
                            p.ok_border,
                            "电信/联通/移动宽带出口不限流量，上方为全账号本月实际总流量",
                        ),
                        (_, true) => (
                            "校园网限额",
                            p.accent,
                            p.accent_soft,
                            p.accent_border,
                            "仅校园网出口计入套餐额度，上方为自助服务官方剩余流量",
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
                        (
                            SelfServiceStatus::Unreachable
                            | SelfServiceStatus::AuthFailed
                            | SelfServiceStatus::UnsupportedPassword,
                            false,
                        ) => (
                            "未同步",
                            p.neutral,
                            p.neutral_soft,
                            p.neutral_border,
                            self_service.message.as_str(),
                        ),
                        _ => (
                            "本地参考",
                            p.neutral,
                            p.neutral_soft,
                            p.neutral_border,
                            "等待自助服务同步，当前展示本机物理网卡流量",
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
                    if has_data && card_w >= 260.0 {
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

        ui.add_space(7.0);

        // 2. 主指标与底部两行参考（预留微软雅黑粗体真实行高，保证左右双卡底边 0 像素齐平）
        let body_h = (inner_h - 33.0).max(115.0);
        let footer_h = 50.0_f32;
        let top_h = (body_h - footer_h).max(65.0);

        ui.allocate_ui_with_layout(
            Vec2::new(card_w, top_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if let Some(data) = &self_service.data {
                    let t = &data.traffic;
                    match *view_mode {
                        TrafficViewMode::Carrier => {
                            let actual_total = t.month_actual_total_mb();
                            let down = t.month_down_mb.max(0.0);
                            let up = t.month_up_mb.max(0.0);
                            let sum = (down + up).max(0.001);
                            let dl_frac = (down / sum) as f32;

                            ui.allocate_ui_with_layout(
                                Vec2::new(card_w, 25.0),
                                egui::Layout::left_to_right(egui::Align::BOTTOM),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    ui.label(bold_text(
                                        format_mb_compact(actual_total),
                                        21.0,
                                        p.text,
                                    ));
                                    ui.label(body_text("本月全账号实际", 10.5, p.text_muted));
                                },
                            );

                            ui.add_space(7.0);
                            draw_dual_bar(ui, p, card_w, 6.0, dl_frac, down > 0.0, up > 0.0);
                            ui.add_space(6.0);

                            ui.allocate_ui_with_layout(
                                Vec2::new(card_w, 15.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 12.0;
                                    inline_dot_metric(
                                        ui,
                                        p,
                                        p.accent,
                                        "下行",
                                        &format_mb_compact(down),
                                    );
                                    inline_dot_metric(ui, p, p.ok, "上行", &format_mb_compact(up));
                                },
                            );
                        }
                        TrafficViewMode::Campus => {
                            let rem = t.campus_remaining_mb.max(0.0);
                            let used = t.campus_used_mb.max(0.0);
                            let total = t.campus_total_mb().max(0.001);
                            let used_ratio = (used / total).clamp(0.0, 1.0) as f32;
                            let depleted = rem <= 0.1;

                            ui.allocate_ui_with_layout(
                                Vec2::new(card_w, 25.0),
                                egui::Layout::left_to_right(egui::Align::BOTTOM),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    let (hero_val, hero_col) = if depleted {
                                        ("已用尽".to_string(), p.danger)
                                    } else {
                                        (format_mb_compact(rem), p.text)
                                    };
                                    ui.label(bold_text(hero_val, 21.0, hero_col));
                                    ui.label(body_text("校园网剩余", 10.5, p.text_muted));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::BOTTOM),
                                        |ui| {
                                            ui.label(bold_text(
                                                format_mb_compact(used),
                                                11.0,
                                                p.text,
                                            ));
                                            ui.label(body_text("已用", 10.2, p.text_muted));
                                        },
                                    );
                                },
                            );

                            ui.add_space(7.0);
                            let bar_color = if depleted || used_ratio >= 0.95 {
                                p.danger
                            } else if used_ratio >= 0.80 {
                                p.warn
                            } else {
                                p.accent
                            };
                            draw_single_bar(
                                ui,
                                p,
                                card_w,
                                6.0,
                                if depleted { 1.0 } else { used_ratio },
                                used > 0.0 || depleted,
                                bar_color,
                            );
                            ui.add_space(6.0);

                            ui.allocate_ui_with_layout(
                                Vec2::new(card_w, 15.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let plan = if t.plan_description.is_empty() {
                                        "校园网套餐额度"
                                    } else {
                                        t.plan_description.as_str()
                                    };
                                    ui.add(
                                        egui::Label::new(body_text(plan, 10.3, p.text_muted))
                                            .truncate(),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(body_text(
                                                format!("已用 {:.1}%", used_ratio * 100.0),
                                                10.3,
                                                p.text_muted,
                                            ));
                                        },
                                    );
                                },
                            );
                        }
                    }
                } else {
                    // 无门户缓存时优雅降级：展示本机物理网卡今日流量
                    let rx = local_traffic.today_rx_bytes;
                    let tx = local_traffic.today_tx_bytes;
                    let total = rx.saturating_add(tx);
                    let dl_frac = if total > 0 {
                        rx as f32 / total as f32
                    } else {
                        0.0
                    };

                    ui.allocate_ui_with_layout(
                        Vec2::new(card_w, 25.0),
                        egui::Layout::left_to_right(egui::Align::BOTTOM),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(bold_text(format_bytes_compact(total), 21.0, p.text));
                            ui.label(body_text("本机今日（网卡统计）", 10.5, p.text_muted));
                        },
                    );
                    ui.add_space(7.0);
                    draw_dual_bar(ui, p, card_w, 6.0, dl_frac, rx > 0, tx > 0);
                    ui.add_space(6.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(card_w, 15.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            inline_dot_metric(ui, p, p.accent, "下载", &format_bytes_compact(rx));
                            inline_dot_metric(ui, p, p.ok, "上传", &format_bytes_compact(tx));
                        },
                    );
                }
            },
        );

        // 3. 底部固定两行对照（严格控制总高 = 46px，与右侧卡片底线严格齐平）
        ui.add_space(3.0);
        let sep = ui.allocate_space(Vec2::new(card_w, 1.0)).1;
        ui.painter().line_segment(
            [sep.left_center(), sep.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );
        ui.add_space(6.0);

        if let Some(data) = &self_service.data {
            let t = &data.traffic;
            match *view_mode {
                TrafficViewMode::Carrier => {
                    let row_tip = if t.plan_description.is_empty() {
                        "校园网出口套餐余量（与运营商出口独立计费）".to_string()
                    } else {
                        format!("校园网套餐：{}（仅校园网出口计费）", t.plan_description)
                    };
                    compact_kv_row(
                        ui,
                        p,
                        "校园网免费额度",
                        &format!(
                            "余 {} · 已用 {}",
                            format_mb_compact(t.campus_remaining_mb),
                            format_mb_compact(t.campus_used_mb)
                        ),
                        Some(&row_tip),
                    );
                }
                TrafficViewMode::Campus => {
                    compact_kv_row(
                        ui,
                        p,
                        "本月全账号实际",
                        &format!(
                            "{} · ↓{} ↑{}",
                            format_mb_compact(t.month_actual_total_mb()),
                            format_mb_short(t.month_down_mb),
                            format_mb_short(t.month_up_mb),
                        ),
                        Some("含电信/联通/移动运营商与校园网出口的全部实际上下行总流量"),
                    );
                }
            }
            ui.add_space(4.0);
            compact_kv_row(
                ui,
                p,
                "本机今日（网卡参考）",
                &format!(
                    "{} · ↓{} ↑{}",
                    format_bytes_compact(local_traffic.today_total_bytes()),
                    format_bytes_short(local_traffic.today_rx_bytes),
                    format_bytes_short(local_traffic.today_tx_bytes),
                ),
                Some("本机物理网卡今日在线增量（已自动排除 Clash/Mihomo TUN 虚拟网卡）"),
            );
        } else {
            compact_kv_row(
                ui,
                p,
                "本机本月累计",
                &format_bytes_compact(local_traffic.month_total_bytes()),
                None,
            );
            ui.add_space(4.0);
            let hint = if self_service.message.is_empty() {
                "连入校园网后自动同步自助服务"
            } else {
                self_service.message.as_str()
            };
            compact_kv_row(ui, p, "自助服务", hint, Some(hint));
        }
    });

    (refresh_clicked, view_changed)
}

/// 右卡：在线设备与近期上网记录（带并发上限配额 `N / M 台在线` + 真实流量历史会话）。
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
        let inner_h = (target_height - 26.0).max(148.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);
        ui.spacing_mut().item_spacing.y = 0.0;

        let card_w = ui.available_width();

        // 1. 顶部标题行（26px）
        ui.allocate_ui_with_layout(
            Vec2::new(card_w, 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                section_heading(ui, Icon::Monitor, p.ok, "在线设备与记录", p.text, 26.0);

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
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if icon_button(
                        ui,
                        p,
                        Icon::Refresh,
                        Vec2::splat(24.0),
                        12.0,
                        false,
                        "刷新在线终端与近期上网记录",
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

        ui.add_space(7.0);

        let body_h = (inner_h - 33.0).max(90.0);

        let Some(data) = &self_service.data else {
            draw_devices_placeholder(ui, p, self_service, local_campus_ip, card_w, body_h);
            return;
        };

        // 2. 上半区：当前在线设备（1–2 台直出不建 ScrollArea，避免 egui 默认 64px 最小高度撑高卡片；超 2 台可滚）
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

        let at_limit = data
            .max_online_devices
            .is_some_and(|m| devices.len() >= m as usize);
        let online_area_h = match devices.len() {
            0 | 1 => 28.0_f32,
            2 => 60.0_f32.min(body_h * 0.56),
            _ => 62.0_f32.min(body_h * 0.56),
        };

        ui.allocate_ui_with_layout(
            Vec2::new(card_w, online_area_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                if devices.is_empty() {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(card_w, 28.0), Sense::hover());
                    ui.painter().rect(
                        rect,
                        CornerRadius::same(6),
                        p.subtle,
                        Stroke::new(1.0_f32, p.divider),
                        StrokeKind::Inside,
                    );
                    ui.painter().text(
                        Pos2::new(rect.left() + 10.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        "当前账号暂无在线终端会话",
                        egui::FontId::proportional(10.8),
                        p.text_muted,
                    );
                } else if devices.len() <= 2 {
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
                        .max_height(online_area_h)
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

        ui.add_space(7.0);
        let sep = ui.allocate_space(Vec2::new(card_w, 1.0)).1;
        ui.painter().line_segment(
            [sep.left_center(), sep.right_center()],
            Stroke::new(1.0_f32, p.divider),
        );
        ui.add_space(5.0);

        // 3. 下半区：门户最近上网记录（带真实运营商流量，严控剩余高度与左卡底部严格齐平）
        ui.allocate_ui_with_layout(
            Vec2::new(card_w, 15.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(body_text("最近上网记录（实际流量）", 10.0, p.text_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let count_str = match data.traffic.month_session_count {
                        Some(n) => format!("本月 {n} 次"),
                        None if !data.recent_sessions.is_empty() => {
                            format!("最近 {} 条", data.recent_sessions.len())
                        }
                        None => String::new(),
                    };
                    if !count_str.is_empty() {
                        let resp = ui.label(body_text(&count_str, 10.0, p.text_muted));
                        if !data.recent_sessions.is_empty() {
                            resp.on_hover_ui(|ui| {
                                draw_recent_sessions_full_tooltip(ui, p, data);
                            });
                        }
                    }
                });
            },
        );
        ui.add_space(3.0);

        let hist_h = (body_h - online_area_h - 31.0).max(20.0);
        if data.recent_sessions.is_empty() {
            ui.allocate_ui_with_layout(
                Vec2::new(card_w, hist_h),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(body_text("暂无近期上网记录", 10.5, p.text_muted));
                },
            );
        } else {
            ui.allocate_ui_with_layout(
                Vec2::new(card_w, hist_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("recent_sessions_scroll")
                        .min_scrolled_height(0.0)
                        .max_height(hist_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(card_w);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for rec in &data.recent_sessions {
                                draw_recent_session_row(ui, p, rec, local_campus_ip);
                            }
                        });
                },
            );
        }
    });

    refresh_clicked
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
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            let iw = (width - 26.0).max(120.0);
            let ih = (height - 22.0).max(60.0);
            ui.set_min_size(Vec2::new(iw, ih));
            ui.set_max_size(Vec2::new(iw, ih));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                if !local_campus_ip.is_empty() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (dr, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                        ui.painter().circle_filled(dr.center(), 3.2, p.ok);
                        ui.label(body_text("本机校园网 IP", 11.0, p.text_secondary));
                        ui.label(mono_text(local_campus_ip, 11.5, p.text));
                    });
                }
                let hint = match self_service.status {
                    SelfServiceStatus::Refreshing => "正在向校园网自助服务拉取在线终端与上网记录…",
                    SelfServiceStatus::CaptchaCooldown => self_service.message.as_str(),
                    SelfServiceStatus::NoCredentials => "请先在设置中保存校园网学号与密码",
                    _ if !self_service.message.is_empty() => self_service.message.as_str(),
                    _ => "连入校园网内网后，将自动同步当前在线终端、并发上限与近期记录",
                };
                ui.add(egui::Label::new(body_text(hint, 10.8, p.text_muted)).wrap());
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
    let row_h = 28.0_f32;
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
    let mut x = rect.left() + 9.0;

    // 1. 状态圆点
    let dot_c = if is_local {
        p.ok
    } else if highlight_conflict {
        p.warn
    } else {
        p.accent
    };
    painter.circle_filled(Pos2::new(x + 3.0, cy), 3.0, dot_c);
    x += 12.0;

    // 2. 终端类型小标签（如 PC / 手机）
    if !dev.terminal_type.is_empty() {
        x = paint_mini_pill(
            painter,
            x,
            cy,
            &dev.terminal_type,
            p.text_secondary,
            p.card,
            p.border,
        ) + 6.0;
    }

    // 3. 等宽 IP
    let ip_galley = painter.layout_no_wrap(
        dev.ip.clone(),
        egui::FontId::new(11.3, egui::FontFamily::Monospace),
        p.text,
    );
    painter.galley(
        Pos2::new(x, cy - ip_galley.size().y * 0.5),
        ip_galley.clone(),
        p.text,
    );
    x += ip_galley.size().x + 6.0;

    // 4. 身份徽章（本机 / 其他终端）
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
    let _ = paint_mini_pill(painter, x, cy, id_lbl, id_fg, id_bg, id_bd);

    // 5. 右侧上线时刻 + 实时在线时长
    let start_hm = dev
        .login_time
        .split_whitespace()
        .nth(1)
        .and_then(|t| t.get(..5))
        .unwrap_or("--:--");
    let elapsed_secs = if let Some(use_secs) = dev.use_time_secs {
        use_secs.saturating_add((now_unix - fetched_at_unix).max(0) as u64)
    } else if let Some(ts) = dev.login_unix {
        (now_unix - ts).max(0) as u64
    } else {
        0
    };
    let right_txt = if elapsed_secs > 0 {
        format!("{start_hm} 上线 · {}", format_duration_short(elapsed_secs))
    } else {
        format!("{start_hm} 上线")
    };
    painter.text(
        Pos2::new(rect.right() - 9.0, cy),
        egui::Align2::RIGHT_CENTER,
        right_txt,
        egui::FontId::proportional(10.2),
        p.text_muted,
    );

    if resp.hovered() {
        resp.on_hover_ui(|ui| {
            ui.set_min_width(210.0);
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
        });
    }
}

fn draw_recent_session_row(
    ui: &mut egui::Ui,
    p: &Palette,
    rec: &RecentSession,
    local_campus_ip: &str,
) {
    let row_w = ui.available_width();
    let row_h = 18.0_f32;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(row_w, row_h), Sense::hover());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), p.subtle);
    }

    let cy = rect.center().y;
    let painter = ui.painter();
    let start_hm = format_hm(rec.login_unix_ms / 1000);
    let end_hm = rec
        .logout_unix_ms
        .map(|ms| format_hm(ms / 1000))
        .unwrap_or_else(|| "在线".to_string());
    let span = format!("{start_hm}–{end_hm}");

    let mut x = rect.left() + 3.0;
    let span_g = painter.layout_no_wrap(
        span,
        egui::FontId::new(10.2, egui::FontFamily::Monospace),
        p.text,
    );
    painter.galley(
        Pos2::new(x, cy - span_g.size().y * 0.5),
        span_g.clone(),
        p.text,
    );
    x += span_g.size().x + 5.0;

    let dur_txt = if rec.duration_secs > 0 {
        format_duration_short(rec.duration_secs)
    } else {
        "<1m".to_string()
    };
    let same_ip = !local_campus_ip.is_empty() && rec.ip == local_campus_ip;
    let meta = format!("· {} · {dur_txt}", rec.ip);
    painter.text(
        Pos2::new(x, cy),
        egui::Align2::LEFT_CENTER,
        meta,
        egui::FontId::proportional(10.2),
        p.text_muted,
    );

    let flow_txt = format_mb_compact(rec.actual_total_mb());
    painter.text(
        Pos2::new(rect.right() - 3.0, cy),
        egui::Align2::RIGHT_CENTER,
        flow_txt,
        egui::FontId::new(10.3, crate::ui::theme::bold_family()),
        p.text,
    );

    if resp.hovered() {
        resp.on_hover_ui(|ui| {
            ui.set_min_width(226.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(bold_text("门户上网记录明细", 11.5, p.text));
            detail_kv_row(ui, p, "上线时间", &format_md_hms(rec.login_unix_ms / 1000));
            let logout_str = rec
                .logout_unix_ms
                .map(|ms| format_md_hms(ms / 1000))
                .unwrap_or_else(|| "尚未下线".to_string());
            detail_kv_row(ui, p, "下线时间", &logout_str);
            detail_kv_row(
                ui,
                p,
                "登录 IP",
                &if same_ip {
                    format!("{}（本机）", rec.ip)
                } else {
                    rec.ip.clone()
                },
            );
            detail_kv_row(ui, p, "真实时长", &dur_txt);
            detail_kv_row(
                ui,
                p,
                "实际流量",
                &format!(
                    "{}（↓{} ↑{}）",
                    format_mb_compact(rec.actual_total_mb()),
                    format_mb_short(rec.actual_down_mb),
                    format_mb_short(rec.actual_up_mb),
                ),
            );
            detail_kv_row(ui, p, "校园网计费", &format_mb_compact(rec.campus_flow_mb));
        });
    }
}

fn draw_recent_sessions_full_tooltip(ui: &mut egui::Ui, p: &Palette, data: &SelfServiceData) {
    ui.set_min_width(276.0);
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.label(bold_text(
        "门户最近上网记录（含运营商实际流量）",
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

fn paint_mini_pill(
    painter: &egui::Painter,
    left: f32,
    cy: f32,
    text: &str,
    fg: Color32,
    bg: Color32,
    border: Color32,
) -> f32 {
    let font = egui::FontId::new(9.5, crate::ui::theme::bold_family());
    let g = painter.layout_no_wrap(text.to_string(), font, fg);
    let w = g.size().x + 10.0;
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

fn draw_dual_bar(
    ui: &mut egui::Ui,
    p: &Palette,
    width: f32,
    height: f32,
    left_frac: f32,
    has_left: bool,
    has_right: bool,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let r = CornerRadius::same((height * 0.5).round() as u8);
    let painter = ui.painter();
    painter.rect_filled(rect, r, p.subtle);

    if !has_left && !has_right {
        return;
    }
    let gap = if has_left && has_right { 2.0_f32 } else { 0.0 };
    let usable = (width - gap).max(4.0);
    let lw = if has_left && has_right {
        (usable * left_frac.clamp(0.06, 0.94)).round()
    } else if has_left {
        usable
    } else {
        0.0
    };
    let rw = if has_right {
        (usable - lw).max(0.0)
    } else {
        0.0
    };

    if lw > 0.5 {
        let lr = Rect::from_min_size(rect.min, Vec2::new(lw, height));
        painter.rect_filled(lr, r, p.accent);
    }
    if rw > 0.5 {
        let rr = Rect::from_min_size(
            Pos2::new(rect.left() + lw + gap, rect.top()),
            Vec2::new(rw, height),
        );
        painter.rect_filled(rr, r, p.ok);
    }
}

fn draw_single_bar(
    ui: &mut egui::Ui,
    p: &Palette,
    width: f32,
    height: f32,
    ratio: f32,
    has_value: bool,
    color: Color32,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let r = CornerRadius::same((height * 0.5).round() as u8);
    let painter = ui.painter();
    painter.rect_filled(rect, r, p.subtle);
    if has_value {
        let fw = (width * ratio.clamp(0.0, 1.0)).max(8.0).min(width);
        let fr = Rect::from_min_size(rect.min, Vec2::new(fw, height));
        painter.rect_filled(fr, r, color);
    }
}

fn inline_dot_metric(ui: &mut egui::Ui, p: &Palette, color: Color32, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (dr, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
        ui.painter().circle_filled(dr.center(), 3.0, color);
        ui.label(body_text(label, 10.5, p.text_muted));
        ui.label(bold_text(value, 10.8, p.text));
    });
}

fn compact_kv_row(ui: &mut egui::Ui, p: &Palette, key: &str, value: &str, tooltip: Option<&str>) {
    let resp = ui
        .allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(body_text(key, 10.4, p.text_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(bold_text(value, 10.5, p.text_secondary)).truncate());
                });
            },
        )
        .response;
    if let Some(tip) = tooltip {
        resp.on_hover_text(tip);
    }
}

// ── Tier 3：通栏紧凑「连接稳定性」胶囊时间轴 ────────────────────────────

/// 位于双卡之下、运行日志之上的通栏紧凑稳定性面板（固定自然高度，无冗余断联列表）。
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

    card_padded(ui, p, 12, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let card_w = ui.available_width();

        // 1. 单行紧凑标题 + 在线率 + 主因摘要 + 图例 + 1h/7h/24h
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
                        15.5,
                        rate_color,
                    ));
                } else {
                    ui.label(bold_text("—", 15.5, p.text_muted));
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

                // 右侧先留出固定宽度的图例 + 分段器，确保左侧摘要按剩余宽度干净截断
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let mut idx = range.index();
                    let labels = ["1h", "7h", "24h"];
                    if segmented_control(ui, p, "stab_strip_range_seg", &mut idx, &labels) {
                        *range = TimeRange::from_index(idx);
                    }
                    if card_w >= 620.0 {
                        // 在 right_to_left 容器中倒序放置，最终从左到右呈现：「● 在线  ● 断联  ○ 未运行」
                        legend_dot_item(ui, p, None, "未运行");
                        legend_dot_item(ui, p, Some(p.warn), "断联");
                        legend_dot_item(ui, p, Some(Color32::from_rgb(0x10, 0xB9, 0x81)), "在线");
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(body_text(&summary, 10.5, p.text_muted)).truncate(),
                        )
                        .on_hover_text(&summary);
                    });
                });
            },
        );

        ui.add_space(6.0);

        // 2. 通栏 48/42/30 枚细胶囊时间轴 + 真实时刻标尺
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

/// 细粒度胶囊时间轴：
/// - 24h = 48 枚细胶囊（30m/枚）、7h = 42 枚（10m/枚）、1h = 30 枚（2m/枚）
/// - 悬停黄色/红色胶囊即弹出完整断联详情小卡（同槽多起断联全部在浮层内展示）
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
        TimeRange::H24 => 48,
        TimeRange::H7 => 42,
        TimeRange::H1 => 30,
    };
    let slot_secs = window_secs as f64 / slot_count as f64;

    let top_lift = 3.0_f32;
    let bar_h = 19.0_f32;
    let ruler_gap = 4.0_f32;
    let ruler_h = 16.0_f32;
    let total_h = top_lift + bar_h + ruler_gap + ruler_h;

    let (full_rect, _) = ui.allocate_exact_size(Vec2::new(total_w, total_h), Sense::hover());
    let bar_rect = Rect::from_min_size(
        Pos2::new(full_rect.left(), full_rect.top() + top_lift),
        Vec2::new(total_w, bar_h),
    );
    let ruler_top = bar_rect.bottom() + ruler_gap;
    let painter = ui.painter();

    let gap = if total_w < 500.0 { 2.0_f32 } else { 2.8_f32 };
    let slot_w = ((total_w - gap * (slot_count as f32 - 1.0)) / slot_count as f32).max(2.0);
    let pill_r = CornerRadius::same((slot_w * 0.35).clamp(2.0, 3.0).round() as u8);
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
            let lift = if is_hovered { 2.0 } else { 0.0 };
            let r = Rect::from_min_max(
                Pos2::new(x0, bar_rect.top() + 1.0 - lift),
                Pos2::new(x0 + slot_w, bar_rect.bottom()),
            );
            let fill = if is_hovered {
                online_green
            } else {
                with_alpha(online_green, 205)
            };
            (r, fill)
        } else {
            let idle_h = (bar_h * 0.46).max(8.0);
            let r = Rect::from_min_max(
                Pos2::new(x0, bar_rect.bottom() - idle_h),
                Pos2::new(x0 + slot_w, bar_rect.bottom()),
            );
            let fill = match p.mode {
                crate::ui::theme::ThemeMode::Light => {
                    if is_hovered {
                        Color32::from_rgb(0xB8, 0xC2, 0xD1)
                    } else {
                        Color32::from_rgb(0xDC, 0xE2, 0xEB)
                    }
                }
                crate::ui::theme::ThemeMode::Dark => {
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
    let tick_font = egui::FontId::proportional(9.5);
    let label_y = ruler_top + 11.0;

    painter.line_segment(
        [
            Pos2::new(bar_rect.left(), ruler_top),
            Pos2::new(bar_rect.left(), ruler_top + 4.0),
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
            Pos2::new(bar_rect.right(), ruler_top + 4.0),
        ],
        tick_stroke,
    );
    painter.text(
        Pos2::new(bar_rect.right(), label_y),
        egui::Align2::RIGHT_CENTER,
        "现在",
        egui::FontId::new(9.5, crate::ui::theme::bold_family()),
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

fn format_mb_short(mb: f64) -> String {
    let mb = if mb.is_finite() && mb > 0.0 { mb } else { 0.0 };
    if mb >= 1024.0 {
        let gb = mb / 1024.0;
        if gb >= 100.0 {
            format!("{gb:.0}G")
        } else {
            format!("{gb:.1}G")
        }
    } else if mb >= 1.0 {
        format!("{mb:.0}M")
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

fn format_bytes_short(bytes: u64) -> String {
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
        assert_eq!(format_duration_short(8), "8s");
        assert_eq!(format_duration_short(125), "2m5s");
        assert_eq!(format_duration_short(3660), "1h01m");
    }
}
