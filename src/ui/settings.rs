//! Windows 11 Fluent 风格设置中心：
//! - 左侧分组目录（账号管理 / 监控与自启 / 通知与数据）
//! - 右侧带标题与说明的分组卡片
//! - 底部常驻状态与操作栏（还原修改 / 保存设置）

use std::path::Path;

use eframe::egui::{self, CornerRadius, CursorIcon, Sense, Stroke, StrokeKind, Vec2};

use crate::models::Settings;
use crate::ui::icons::{self, Icon};
use crate::ui::theme::{
    body_text, bold_text, carrier_label, clamp_ui_zoom, Palette, ThemeMode, UiPrefs, RADIUS_MD,
    RADIUS_SM,
};
use crate::ui::widgets::{
    badge, nav_rail_item, primary_button, save_button, secondary_button, segmented_control,
    stepper_field, toggle_switch,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Account,
    Monitor,
    Display,
    Notify,
}

impl SettingsTab {
    pub fn index(self) -> usize {
        match self {
            Self::Account => 0,
            Self::Monitor => 1,
            Self::Display => 2,
            Self::Notify => 3,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        match idx {
            1 => Self::Monitor,
            2 => Self::Display,
            3 => Self::Notify,
            _ => Self::Account,
        }
    }
}

pub struct SettingsForm {
    pub tab: SettingsTab,
    pub student_id: String,
    pub password: String,
    pub show_password: bool,
    pub carrier: usize,
    pub check_interval: i64,
    pub heartbeat_minutes: i64,
    pub offline_interval: i64,
    pub confirm_failures: i64,
    pub auto_start_monitoring: bool,
    pub auto_boot: bool,
    pub notify_toast: bool,
    pub notify_wechat: bool,
    pub notify_quiet_minutes: i64,
    pub monthly_quota_gb: i64,
}

impl SettingsForm {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            tab: SettingsTab::Account,
            student_id: settings.student_id.clone(),
            password: String::new(),
            show_password: false,
            carrier: settings.carrier.parse::<usize>().unwrap_or(2).min(3),
            check_interval: settings.check_interval,
            heartbeat_minutes: (settings.heartbeat_interval / 60).clamp(1, 1440),
            offline_interval: settings.offline_interval,
            confirm_failures: settings.confirm_failures,
            auto_start_monitoring: settings.auto_start_monitoring,
            auto_boot: settings.auto_boot,
            notify_toast: settings.notify_toast,
            notify_wechat: settings.notify_wechat,
            notify_quiet_minutes: settings.notify_quiet_minutes,
            monthly_quota_gb: settings.monthly_quota_gb,
        }
    }

    pub fn reset_from(&mut self, settings: &Settings) {
        let tab = self.tab;
        *self = Self::from_settings(settings);
        self.tab = tab;
    }

    pub fn to_settings(&self) -> Settings {
        Settings {
            student_id: self.student_id.trim().to_string(),
            carrier: self.carrier.to_string(),
            check_interval: self.check_interval,
            heartbeat_interval: self.heartbeat_minutes.clamp(1, 1440) * 60,
            offline_interval: self.offline_interval,
            confirm_failures: self.confirm_failures,
            auto_start_monitoring: self.auto_start_monitoring,
            auto_boot: self.auto_boot,
            notify_toast: self.notify_toast,
            notify_wechat: self.notify_wechat,
            notify_quiet_minutes: self.notify_quiet_minutes,
            monthly_quota_gb: self.monthly_quota_gb,
        }
        .normalized()
    }

    pub fn is_dirty(&self, saved: &Settings) -> bool {
        let current = self.to_settings();
        current.student_id != saved.student_id
            || current.carrier != saved.carrier
            || current.check_interval != saved.check_interval
            || current.heartbeat_interval != saved.heartbeat_interval
            || current.offline_interval != saved.offline_interval
            || current.confirm_failures != saved.confirm_failures
            || current.auto_start_monitoring != saved.auto_start_monitoring
            || current.auto_boot != saved.auto_boot
            || current.notify_toast != saved.notify_toast
            || current.notify_quiet_minutes != saved.notify_quiet_minutes
            || current.monthly_quota_gb != saved.monthly_quota_gb
            || !self.password.is_empty()
    }
}

#[derive(Default)]
pub struct SettingsActions {
    pub test_toast: bool,
    pub save: bool,
    pub reset: bool,
    pub ui_prefs_changed: bool,
    pub set_window_size: Option<[f32; 2]>,
}

/// 成熟版 Windows 11 Fluent 设置中心视图（左侧分组导航 + 右侧分组卡片 + 底部常驻保存栏）。
#[allow(clippy::too_many_arguments)]
pub fn draw_settings_view(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &mut SettingsForm,
    saved: &Settings,
    ui_prefs: &mut UiPrefs,
    app_dir: &Path,
    log_path: &Path,
    credential_account: Option<&str>,
    needs_setup: bool,
    saving: bool,
    total_height: f32,
) -> SettingsActions {
    let mut actions = SettingsActions::default();
    let total_w = ui.available_width();
    let nav_w = 166.0_f32.min((total_w * 0.28).max(138.0));
    let content_w = (total_w - nav_w - 1.0).max(300.0);
    let dirty = form.is_dirty(saved);

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;

        // ── 左侧分组目录（显式 top_down 容器，避免继承外层 horizontal_top 导致底部文字横向溢出） ──
        ui.allocate_ui_with_layout(
            Vec2::new(nav_w, total_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::Frame::new()
                    .fill(p.sidebar)
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 10,
                        top: 12,
                        bottom: 12,
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let inner_w = (nav_w - 20.0).max(118.0);
                        let inner_h = (total_height - 24.0).max(260.0);
                        ui.set_min_size(Vec2::new(inner_w, inner_h));
                        ui.set_max_size(Vec2::new(inner_w, inner_h));

                        let footer_h = 44.0_f32;
                        let top_h = (inner_h - footer_h).max(120.0);

                        ui.allocate_ui_with_layout(
                            Vec2::new(inner_w, top_h),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_min_size(Vec2::new(inner_w, top_h));
                                ui.spacing_mut().item_spacing.y = 3.0;
                                if nav_rail_item(
                                    ui,
                                    p,
                                    form.tab == SettingsTab::Account,
                                    Icon::User,
                                    "账号管理",
                                ) {
                                    form.tab = SettingsTab::Account;
                                }
                                if nav_rail_item(
                                    ui,
                                    p,
                                    form.tab == SettingsTab::Monitor,
                                    Icon::Sliders,
                                    "监控与自启",
                                ) {
                                    form.tab = SettingsTab::Monitor;
                                }
                                if nav_rail_item(
                                    ui,
                                    p,
                                    form.tab == SettingsTab::Display,
                                    Icon::Monitor,
                                    "外观与显示",
                                ) {
                                    form.tab = SettingsTab::Display;
                                }
                                if nav_rail_item(
                                    ui,
                                    p,
                                    form.tab == SettingsTab::Notify,
                                    Icon::Bell,
                                    "通知与数据",
                                ) {
                                    form.tab = SettingsTab::Notify;
                                }
                            },
                        );

                        // 左侧栏底部版本与架构提示
                        let sep = ui.allocate_space(Vec2::new(inner_w, 1.0)).1;
                        ui.painter().line_segment(
                            [sep.left_center(), sep.right_center()],
                            Stroke::new(1.0_f32, p.divider),
                        );
                        ui.add_space(7.0);
                        ui.label(bold_text(
                            format!("GXNU Guard v{}", env!("CARGO_PKG_VERSION")),
                            10.5,
                            p.text_secondary,
                        ));
                        ui.add_space(2.0);
                        ui.label(body_text("关闭窗口即释放界面内存", 10.0, p.text_muted));
                    });
            },
        );

        // 左侧栏与主内容区的 1px 竖向分割线
        let rail_sep = ui.allocate_space(Vec2::new(1.0, total_height)).1;
        ui.painter().line_segment(
            [rail_sep.center_top(), rail_sep.center_bottom()],
            Stroke::new(1.0_f32, p.border),
        );

        // ── 右侧内容区 + 底部常驻操作栏 ──────────────────────────────
        let footer_bar_h = 51.0_f32;
        let scroll_h = (total_height - footer_bar_h).max(200.0);

        ui.allocate_ui_with_layout(
            Vec2::new(content_w, total_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;

                // 上部可滚动分组表单
                ui.allocate_ui_with_layout(
                    Vec2::new(content_w, scroll_h),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::Frame::new()
                            .fill(p.bg)
                            .inner_margin(egui::Margin {
                                left: 18,
                                right: 18,
                                top: 14,
                                bottom: 14,
                            })
                            .show(ui, |ui| {
                                let inner_w = (content_w - 36.0).max(260.0);
                                let inner_h = (scroll_h - 28.0).max(160.0);
                                ui.set_min_size(Vec2::new(inner_w, inner_h));
                                ui.set_max_size(Vec2::new(inner_w, inner_h));
                                egui::ScrollArea::vertical()
                                    .id_salt(("settings_content_scroll", form.tab.index()))
                                    .max_height(inner_h)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_width(inner_w);
                                        match form.tab {
                                            SettingsTab::Account => draw_account_page(
                                                ui,
                                                p,
                                                form,
                                                saved,
                                                credential_account,
                                            ),
                                            SettingsTab::Monitor => draw_monitor_page(ui, p, form),
                                            SettingsTab::Display => {
                                                draw_display_page(ui, p, ui_prefs, &mut actions)
                                            }
                                            SettingsTab::Notify => {
                                                if draw_notify_data_page(
                                                    ui, p, form, app_dir, log_path,
                                                ) {
                                                    actions.test_toast = true;
                                                }
                                            }
                                        }
                                    });
                            });
                    },
                );

                // 底部常驻操作条（顶边 1px 描边）
                let top_line = ui.allocate_space(Vec2::new(content_w, 1.0)).1;
                ui.painter().line_segment(
                    [top_line.left_center(), top_line.right_center()],
                    Stroke::new(1.0_f32, p.border),
                );
                egui::Frame::new()
                    .fill(p.card)
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        let bar_inner_w = (content_w - 32.0).max(260.0);
                        ui.set_width(bar_inner_w);
                        ui.allocate_ui_with_layout(
                            Vec2::new(bar_inner_w, 30.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let (dot_c, status_str) = if dirty {
                                    (p.warn, "有未保存修改")
                                } else {
                                    (p.ok, "配置已生效")
                                };
                                let (dr, _) =
                                    ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                                ui.painter().circle_filled(dr.center(), 3.4, dot_c);
                                ui.label(bold_text(status_str, 11.5, p.text));
                                let summary = format!(
                                    "· {} · {}s 巡检 · 自启{}",
                                    carrier_label(&saved.carrier),
                                    saved.check_interval,
                                    if saved.auto_boot { "开" } else { "关" },
                                );
                                ui.label(body_text(summary, 11.0, p.text_muted));

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        let save_resp = ui
                                            .add_enabled_ui(!saving, |ui| {
                                                if needs_setup {
                                                    primary_button(
                                                        ui,
                                                        p,
                                                        Some(Icon::Check),
                                                        "保存并开始守护",
                                                    )
                                                } else {
                                                    save_button(ui, p, dirty)
                                                }
                                            })
                                            .inner;
                                        if save_resp.clicked() {
                                            actions.save = true;
                                        }
                                        if dirty
                                            && secondary_button(ui, p, None, "还原修改").clicked()
                                        {
                                            actions.reset = true;
                                        }
                                    },
                                );
                            },
                        );
                    });
            },
        );
    });

    actions
}

fn page_header(ui: &mut egui::Ui, p: &Palette, title: &str, subtitle: &str) {
    ui.label(bold_text(title, 15.0, p.text));
    ui.add_space(2.0);
    ui.label(body_text(subtitle, 11.5, p.text_muted));
    ui.add_space(12.0);
}

fn group_card(ui: &mut egui::Ui, p: &Palette, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(RADIUS_MD))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
}

fn draw_account_page(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &mut SettingsForm,
    saved: &Settings,
    credential_account: Option<&str>,
) {
    page_header(
        ui,
        p,
        "账号管理",
        "配置广西师范大学校园网学号、运营商出口与可选流量提醒；密码加密保存在 Windows 凭据管理器中。",
    );

    let has_cred =
        credential_account == Some(form.student_id.trim()) && !form.student_id.trim().is_empty();

    // 1. 当前账号摘要卡（为阶段二多账号列表预留卡片容器结构）
    group_card(ui, p, |ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 34.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (ico_r, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                ui.painter()
                    .rect_filled(ico_r, CornerRadius::same(8), p.accent_soft);
                icons::paint(ui.painter(), ico_r.shrink(7.0), Icon::User, p.accent);

                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let title = if saved.student_id.trim().is_empty() {
                            "尚未配置校园网账号".to_string()
                        } else {
                            mask_student_id(&saved.student_id)
                        };
                        ui.label(bold_text(title, 12.5, p.text));
                        badge(
                            ui,
                            p.accent,
                            p.accent_soft,
                            p.accent_border,
                            carrier_label(&saved.carrier),
                        );
                    });
                    let quota_text = if saved.monthly_quota_gb > 0 {
                        format!("月流量提醒 {} GB", saved.monthly_quota_gb)
                    } else {
                        "月流量不限（适合运营商出口）".to_string()
                    };
                    ui.label(body_text(quota_text, 10.5, p.text_muted));
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if has_cred {
                        badge(ui, p.ok, p.ok_soft, p.ok_border, "凭据已加密");
                    } else {
                        badge(ui, p.warn, p.warn_soft, p.warn_border, "未保存密码");
                    }
                });
            },
        );
    });

    ui.add_space(10.0);

    // 2. 账号编辑表单卡
    group_card(ui, p, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(bold_text("账号与出口配置", 12.5, p.text));

        let full_w = ui.available_width();
        let col_gap = 12.0_f32;
        let col_w = ((full_w - col_gap) * 0.5).floor().max(140.0);

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = col_gap;
            ui.allocate_ui_with_layout(
                Vec2::new(col_w, 48.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.label(body_text("校园网学号", 11.5, p.text_secondary));
                    ui.add_space(3.0);
                    styled_text_box(
                        ui,
                        p,
                        col_w,
                        &mut form.student_id,
                        "输入校园网学号",
                        false,
                        None,
                    );
                },
            );

            ui.allocate_ui_with_layout(
                Vec2::new(col_w, 48.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.label(body_text("密码", 11.5, p.text_secondary));
                    ui.add_space(3.0);
                    let hint = if has_cred {
                        "已保存，留空不修改"
                    } else {
                        "输入校园网密码"
                    };
                    let editing_password = !form.password.is_empty();
                    if !editing_password {
                        form.show_password = false;
                    }
                    let mask_password = !form.show_password;
                    let eye = if editing_password {
                        Some(&mut form.show_password)
                    } else {
                        None
                    };
                    styled_text_box(ui, p, col_w, &mut form.password, hint, mask_password, eye);
                },
            );
        });

        ui.add_space(2.0);
        ui.label(body_text("运营商出口", 11.5, p.text_secondary));
        draw_carrier_row(ui, p, &mut form.carrier);

        row_divider(ui, p);

        fluent_stepper_row(
            ui,
            p,
            "月流量提醒上限（可选）",
            "设为 0 表示不限制（运营商免计费）；设为正值时主页流量环自动切为月额度进度",
            &mut form.monthly_quota_gb,
            0..=2000,
            10,
            "GB",
        );

        ui.add_space(2.0);
        egui::Frame::new()
            .fill(p.subtle)
            .stroke(Stroke::new(1.0_f32, p.divider))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(body_text(
                    "密码通过 Windows DPAPI 写入当前用户的 Windows 凭据管理器（GXNUCampusLogin/v4），绝不以明文存入配置文件或日志。",
                    10.5,
                    p.text_muted,
                ));
            });
    });
}

fn draw_carrier_row(ui: &mut egui::Ui, p: &Palette, carrier: &mut usize) {
    let labels = ["校园用户", "中国电信", "中国联通", "中国移动"];
    let gap = 8.0_f32;
    let btn_w = ((ui.available_width() - gap * 3.0) * 0.25)
        .floor()
        .max(74.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (idx, label) in labels.iter().enumerate() {
            let active = *carrier == idx;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(btn_w, 30.0), Sense::click());
            let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
            let (fill, stroke, fg) = if active {
                (p.accent_soft, Stroke::new(1.2_f32, p.accent), p.accent)
            } else if resp.hovered() {
                (p.card_hover, Stroke::new(1.0_f32, p.border_strong), p.text)
            } else {
                (p.subtle, Stroke::new(1.0_f32, p.border), p.text_secondary)
            };
            ui.painter().rect(
                rect,
                CornerRadius::same(RADIUS_SM),
                fill,
                stroke,
                StrokeKind::Inside,
            );
            let font = if active {
                egui::FontId::new(12.0, crate::ui::theme::bold_family())
            } else {
                egui::FontId::proportional(12.0)
            };
            ui.painter()
                .text(rect.center(), egui::Align2::CENTER_CENTER, *label, font, fg);
            if resp.clicked() {
                *carrier = idx;
            }
        }
    });
}

fn draw_monitor_page(ui: &mut egui::Ui, p: &Palette, form: &mut SettingsForm) {
    page_header(
        ui,
        p,
        "监控与自启",
        "自定义网络巡检周期、防抖复核策略以及 Windows 登录自启行为。",
    );

    group_card(ui, p, |ui| {
        fluent_stepper_row(
            ui,
            p,
            "在线检测间隔",
            "网络正常时的常规巡检周期（10 – 3600 秒）",
            &mut form.check_interval,
            10..=3600,
            10,
            "秒",
        );
        row_divider(ui, p);
        fluent_stepper_row(
            ui,
            p,
            "失败复核次数",
            "连续失败达到此次数才判定断联并重连，避免短暂抖动误报（1 – 5 次）",
            &mut form.confirm_failures,
            1..=5,
            1,
            "次",
        );
        row_divider(ui, p);
        fluent_stepper_row(
            ui,
            p,
            "离线复检间隔",
            "处于离线或等待重连状态时的复查间隔（5 – 300 秒）",
            &mut form.offline_interval,
            5..=300,
            5,
            "秒",
        );
        row_divider(ui, p);
        fluent_stepper_row(
            ui,
            p,
            "心跳日志间隔",
            "持续在线时写入健康心跳日志的周期（1 – 1440 分钟）",
            &mut form.heartbeat_minutes,
            1..=1440,
            5,
            "分",
        );
    });

    ui.add_space(10.0);

    group_card(ui, p, |ui| {
        fluent_toggle_row(
            ui,
            p,
            "开机自动启动到托盘",
            "登录 Windows 时静默启动守护进程至通知区（写入 HKCU\\Run\\GXNUCampusLoginRS）",
            &mut form.auto_boot,
            true,
            None,
        );
        row_divider(ui, p);
        fluent_toggle_row(
            ui,
            p,
            "启动后立即开始监控",
            "程序启动完成即自动进入在线守护状态，无需手动点击开始",
            &mut form.auto_start_monitoring,
            true,
            None,
        );
    });
}

fn draw_display_page(
    ui: &mut egui::Ui,
    p: &Palette,
    ui_prefs: &mut UiPrefs,
    actions: &mut SettingsActions,
) {
    page_header(
        ui,
        p,
        "外观与显示",
        "调整主题模式、整体界面缩放比例、窗口大小与主页模块展示；本页修改即时生效并自动保存。",
    );

    group_card(ui, p, |ui| {
        // 1. 主题模式
        let row_w = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(row_w, 36.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let left_w = (row_w - 110.0).max(160.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(left_w, 34.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(bold_text("主题外观", 12.0, p.text));
                        ui.label(body_text(
                            "支持浅色与深色模式，与标题栏快捷切换实时同步",
                            10.5,
                            p.text_muted,
                        ));
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut idx = match ui_prefs.theme {
                        ThemeMode::Light => 0,
                        ThemeMode::Dark => 1,
                    };
                    if segmented_control(ui, p, "settings_theme_seg", &mut idx, &["浅色", "深色"])
                    {
                        ui_prefs.theme = if idx == 0 {
                            ThemeMode::Light
                        } else {
                            ThemeMode::Dark
                        };
                        actions.ui_prefs_changed = true;
                    }
                });
            },
        );

        row_divider(ui, p);

        // 2. 全局界面缩放比例（5% 步进 + 6 档快捷预设）
        let mut zoom_pct = (ui_prefs.zoom * 100.0).round() as i64;
        let prev_pct = zoom_pct;
        fluent_stepper_row(
            ui,
            p,
            "界面缩放比例",
            "元素偏大时可调小至 85% / 90%；亦支持 Ctrl + / − / 0 或 Ctrl + 滚轮随时缩放",
            &mut zoom_pct,
            75..=125,
            5,
            "%",
        );
        if zoom_pct != prev_pct {
            ui_prefs.zoom = clamp_ui_zoom(zoom_pct as f32 / 100.0);
            actions.ui_prefs_changed = true;
        }

        ui.add_space(6.0);
        let presets: &[(i64, &str)] = &[
            (75, "75%"),
            (85, "85%"),
            (90, "90%"),
            (100, "100% 标准"),
            (110, "110%"),
            (125, "125%"),
        ];
        let gap = 6.0_f32;
        let chip_w = ((ui.available_width() - gap * (presets.len() as f32 - 1.0))
            / presets.len() as f32)
            .floor()
            .max(54.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let cur_pct = (ui_prefs.zoom * 100.0).round() as i64;
            for (pct, label) in presets {
                let active = cur_pct == *pct;
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(chip_w, 26.0), Sense::click());
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                let (fill, stroke, fg) = if active {
                    (p.accent_soft, Stroke::new(1.2_f32, p.accent), p.accent)
                } else if resp.hovered() {
                    (p.card_hover, Stroke::new(1.0_f32, p.border_strong), p.text)
                } else {
                    (p.subtle, Stroke::new(1.0_f32, p.border), p.text_secondary)
                };
                ui.painter().rect(
                    rect,
                    CornerRadius::same(RADIUS_SM),
                    fill,
                    stroke,
                    StrokeKind::Inside,
                );
                let font = if active {
                    egui::FontId::new(11.0, crate::ui::theme::bold_family())
                } else {
                    egui::FontId::proportional(11.0)
                };
                ui.painter()
                    .text(rect.center(), egui::Align2::CENTER_CENTER, *label, font, fg);
                if resp.clicked() && !active {
                    ui_prefs.zoom = clamp_ui_zoom(*pct as f32 / 100.0);
                    actions.ui_prefs_changed = true;
                }
            }
        });

        row_divider(ui, p);

        // 3. 窗口物理尺寸预设
        let row_w = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(row_w, 36.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let left_w = (row_w - 220.0).max(150.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(left_w, 34.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(bold_text("窗口尺寸预设", 12.0, p.text));
                        ui.label(body_text(
                            "一键切换窗口大小；手动拖拽边缘缩放也会自动记住",
                            10.5,
                            p.text_muted,
                        ));
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let sizes: &[(&str, [f32; 2], &str)] = &[
                        ("宽松", [960.0, 640.0], "调整为 960 × 640"),
                        ("标准", [840.0, 580.0], "调整为默认 840 × 580"),
                        (
                            "紧凑",
                            [720.0, 500.0],
                            "调整为紧凑 720 × 500（推荐配合 85% 缩放）",
                        ),
                    ];
                    for (label, sz, tip) in sizes {
                        let resp = secondary_button(ui, p, None, label).on_hover_text(*tip);
                        if resp.clicked() {
                            actions.set_window_size = Some(*sz);
                        }
                    }
                });
            },
        );
    });

    ui.add_space(10.0);

    // 4. 主页模块展示开关
    group_card(ui, p, |ui| {
        let prev = ui_prefs.show_home_traffic;
        fluent_toggle_row(
            ui,
            p,
            "主页展示「本机流量」卡片",
            "关闭后，中央区域由「连接稳定性与断联记录」宽屏双栏接管；后台仍会持续记录，随时可重开",
            &mut ui_prefs.show_home_traffic,
            true,
            None,
        );
        if ui_prefs.show_home_traffic != prev {
            actions.ui_prefs_changed = true;
        }
    });
}

fn draw_notify_data_page(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &mut SettingsForm,
    app_dir: &Path,
    log_path: &Path,
) -> bool {
    let mut test_clicked = false;
    page_header(
        ui,
        p,
        "通知与数据",
        "管理断网与自动恢复的系统弹窗提醒，以及本机配置、日志与流量统计信息。",
    );

    group_card(ui, p, |ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 38.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let left_w = (ui.available_width() - 165.0).max(160.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(left_w, 36.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(bold_text("Windows 系统通知弹窗", 12.0, p.text));
                        ui.label(body_text(
                            "断网自动告知原因，恢复后汇总本次断联时长",
                            10.5,
                            p.text_muted,
                        ));
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    toggle_switch(ui, p, &mut form.notify_toast, true);
                    if secondary_button(ui, p, Some(Icon::Bell), "测试弹窗").clicked() {
                        test_clicked = true;
                    }
                });
            },
        );
        row_divider(ui, p);
        fluent_stepper_row(
            ui,
            p,
            "远程推送静默时间（预留）",
            "系统弹窗仍会即时提醒；此项为后续远程推送渠道预留",
            &mut form.notify_quiet_minutes,
            1..=60,
            1,
            "分",
        );
        row_divider(ui, p);
        fluent_toggle_row(
            ui,
            p,
            "企业微信推送",
            "后续版本补齐 Webhook 推送通道",
            &mut form.notify_wechat,
            false,
            Some("规划中"),
        );
    });

    ui.add_space(10.0);

    group_card(ui, p, |ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 38.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let left_w = (ui.available_width() - 196.0).max(150.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(left_w, 36.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(bold_text("应用数据与日志目录", 12.0, p.text));
                        let dir_text = app_dir.to_string_lossy();
                        ui.add(
                            egui::Label::new(body_text(dir_text.as_ref(), 10.5, p.text_muted))
                                .truncate(),
                        )
                        .on_hover_text(dir_text.as_ref());
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if secondary_button(ui, p, Some(Icon::ExternalLink), "打开日志").clicked() {
                        open_path(log_path);
                    }
                    if secondary_button(ui, p, Some(Icon::Folder), "打开目录").clicked() {
                        let _ = std::process::Command::new("explorer").arg(app_dir).spawn();
                    }
                });
            },
        );
        row_divider(ui, p);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 36.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let left_w = (ui.available_width() - 110.0).max(160.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(left_w, 34.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(bold_text("流量统计口径", 12.0, p.text));
                        ui.label(body_text(
                            "仅在校园网在线期间采集物理网卡增量，自动排除 Clash / Mihomo TUN 虚拟网卡",
                            10.5,
                            p.text_muted,
                        ));
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    badge(ui, p.ok, p.ok_soft, p.ok_border, "物理网卡增量");
                });
            },
        );
    });

    test_clicked
}

#[allow(clippy::too_many_arguments)]
fn fluent_stepper_row(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    desc: &str,
    value: &mut i64,
    range: std::ops::RangeInclusive<i64>,
    step: i64,
    unit: &str,
) {
    let row_w = ui.available_width();
    let left_w = (row_w - 138.0).max(160.0);
    ui.allocate_ui_with_layout(
        Vec2::new(row_w, 38.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, 36.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(bold_text(title, 12.0, p.text));
                    ui.label(body_text(desc, 10.5, p.text_muted));
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                stepper_field(ui, p, value, range, step, unit);
            });
        },
    );
}

fn fluent_toggle_row(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    desc: &str,
    value: &mut bool,
    enabled: bool,
    tag: Option<&str>,
) {
    let row_w = ui.available_width();
    let left_w = (row_w - 52.0).max(160.0);
    ui.allocate_ui_with_layout(
        Vec2::new(row_w, 38.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, 36.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        let fg = if enabled { p.text } else { p.text_muted };
                        ui.label(bold_text(title, 12.0, fg));
                        if let Some(t) = tag {
                            badge(ui, p.neutral, p.neutral_soft, p.neutral_border, t);
                        }
                    });
                    ui.label(body_text(desc, 10.5, p.text_muted));
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                toggle_switch(ui, p, value, enabled);
            });
        },
    );
}

fn styled_text_box(
    ui: &mut egui::Ui,
    p: &Palette,
    width: f32,
    value: &mut String,
    hint: &str,
    password: bool,
    eye_toggle: Option<&mut bool>,
) {
    let inner_w = (width - 18.0).max(100.0);
    egui::Frame::new()
        .fill(p.input_bg)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(RADIUS_SM))
        .inner_margin(egui::Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(inner_w, 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let edit_w = if eye_toggle.is_some() {
                        (inner_w - 26.0).max(70.0)
                    } else {
                        inner_w
                    };
                    ui.add_sized(
                        [edit_w, 20.0],
                        egui::TextEdit::singleline(value)
                            .password(password)
                            .hint_text(hint)
                            .vertical_align(egui::Align::Center)
                            .frame(false),
                    );
                    if let Some(show) = eye_toggle {
                        let (rect, resp) =
                            ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
                        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                        let color = if resp.hovered() { p.text } else { p.text_muted };
                        let icon = if *show { Icon::EyeOff } else { Icon::Eye };
                        icons::paint(ui.painter(), rect.shrink(2.0), icon, color);
                        if resp.clicked() {
                            *show = !*show;
                        }
                        resp.on_hover_text(if *show {
                            "隐藏密码"
                        } else {
                            "显示密码"
                        });
                    }
                },
            );
        });
}

fn row_divider(ui: &mut egui::Ui, p: &Palette) {
    ui.add_space(6.0);
    let sep = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
    ui.painter().line_segment(
        [sep.left_center(), sep.right_center()],
        Stroke::new(1.0_f32, p.divider),
    );
    ui.add_space(6.0);
}

fn mask_student_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.chars().count() <= 4 {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(4).collect();
        format!("{head}***")
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
