//! 左侧设置面板包含账号、监控和通知三个标签页。

use std::path::Path;

use eframe::egui::{self, CornerRadius, CursorIcon, Sense, Stroke, StrokeKind, Vec2};

use crate::models::Settings;
use crate::ui::icons::{self, Icon};
use crate::ui::theme::{body_text, bold_text, carrier_label, with_alpha, Palette, RADIUS_SM};
use crate::ui::widgets::{badge, card_padded, secondary_button, stepper_field, toggle_switch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Account,
    Monitor,
    Notify,
}

impl SettingsTab {
    pub fn index(self) -> usize {
        match self {
            Self::Account => 0,
            Self::Monitor => 1,
            Self::Notify => 2,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        match idx {
            1 => Self::Monitor,
            2 => Self::Notify,
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

pub struct SettingsActions {
    pub test_toast: bool,
}

pub fn draw_panel(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &mut SettingsForm,
    saved: &Settings,
    app_dir: &Path,
    credential_account: Option<&str>,
    target_height: f32,
) -> SettingsActions {
    let mut actions = SettingsActions { test_toast: false };

    card_padded(ui, p, 12, |ui| {
        let inner_h = (target_height - 24.0).max(260.0);
        ui.set_min_height(inner_h);
        ui.set_max_height(inner_h);

        // ── 顶部全宽三等分标签条（账号 | 监控 | 通知）────────────────────
        draw_full_width_tabs(ui, p, &mut form.tab);
        ui.add_space(10.0);

        // 底部留出 72px 放置常驻配置摘要坞，留足底边呼吸空间
        let footer_h = 72.0;
        let body_h = (inner_h - 32.0 - 10.0 - footer_h - 8.0).max(160.0);

        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), body_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("settings_panel_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match form.tab {
                            SettingsTab::Account => {
                                draw_account_tab(ui, p, form, credential_account)
                            }
                            SettingsTab::Monitor => draw_monitor_tab(ui, p, form),
                            SettingsTab::Notify => {
                                if draw_notify_tab(ui, p, form) {
                                    actions.test_toast = true;
                                }
                            }
                        }
                    });
            },
        );

        ui.add_space(8.0);
        draw_settings_footer(ui, p, saved, app_dir, form.is_dirty(saved));
    });

    actions
}

fn draw_settings_footer(
    ui: &mut egui::Ui,
    p: &Palette,
    saved: &Settings,
    app_dir: &Path,
    dirty: bool,
) {
    egui::Frame::new()
        .fill(p.subtle)
        .stroke(Stroke::new(1.0_f32, p.divider))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.set_width((ui.available_width()).max(160.0));
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 16.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    let (dot_c, status_str) = if dirty {
                        (p.warn, "有未保存修改")
                    } else {
                        (p.ok, "配置已生效")
                    };
                    let (dr, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(dr.center(), 3.2, dot_c);
                    ui.label(bold_text(status_str, 11.0, p.text));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let link = ui
                            .add(
                                egui::Label::new(body_text("打开配置目录", 10.5, p.accent))
                                    .sense(Sense::click()),
                            )
                            .on_hover_cursor(CursorIcon::PointingHand);
                        if link.clicked() {
                            let _ = std::process::Command::new("explorer").arg(app_dir).spawn();
                        }
                    });
                },
            );
            ui.add_space(4.0);
            let summary = format!(
                "{} · {}s 巡检 · {}次复核 · 自启{}",
                carrier_label(&saved.carrier),
                saved.check_interval,
                saved.confirm_failures,
                if saved.auto_boot { "开" } else { "关" },
            );
            ui.label(body_text(summary, 10.5, p.text_muted));
        });
}

fn draw_full_width_tabs(ui: &mut egui::Ui, p: &Palette, current: &mut SettingsTab) {
    let labels = ["账号", "监控", "通知"];
    let total_w = ui.available_width();
    egui::Frame::new()
        .fill(p.subtle)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            let inner_w = (total_w - 8.0).max(180.0);
            let item_w = ((inner_w - 4.0) / 3.0).floor();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (idx, label) in labels.iter().enumerate() {
                    let active = current.index() == idx;
                    let (rect, resp) =
                        ui.allocate_exact_size(Vec2::new(item_w, 26.0), Sense::click());
                    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                    if active {
                        ui.painter().rect(
                            rect,
                            CornerRadius::same(6),
                            p.card,
                            Stroke::new(1.0_f32, p.border),
                            StrokeKind::Inside,
                        );
                    } else if resp.hovered() {
                        ui.painter().rect_filled(
                            rect,
                            CornerRadius::same(6),
                            with_alpha(p.card, 130),
                        );
                    }
                    let fg = if active { p.accent } else { p.text_secondary };
                    let font = if active {
                        egui::FontId::new(12.5, crate::ui::theme::bold_family())
                    } else {
                        egui::FontId::proportional(12.5)
                    };
                    ui.painter()
                        .text(rect.center(), egui::Align2::CENTER_CENTER, *label, font, fg);
                    if resp.clicked() {
                        *current = SettingsTab::from_index(idx);
                    }
                }
            });
        });
}

fn draw_account_tab(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &mut SettingsForm,
    credential_account: Option<&str>,
) {
    ui.horizontal(|ui| {
        ui.label(bold_text("账号与网络", 13.0, p.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if credential_account == Some(form.student_id.trim()) {
                badge(ui, p.ok, p.ok_soft, p.ok_border, "凭据已加密");
            } else {
                badge(ui, p.warn, p.warn_soft, p.warn_border, "未保存密码");
            }
        });
    });
    ui.add_space(6.0);

    let full_w = ui.available_width();
    ui.label(body_text("学号", 12.0, p.text_secondary));
    ui.add_space(3.0);
    styled_text_box(
        ui,
        p,
        full_w,
        &mut form.student_id,
        "输入校园网学号",
        false,
        None,
    );

    ui.add_space(7.0);
    ui.label(body_text("密码", 12.0, p.text_secondary));
    ui.add_space(3.0);
    let hint = if credential_account == Some(form.student_id.trim()) {
        "已保存，留空不修改"
    } else {
        "输入校园网密码"
    };
    // 仅在用户输入新密码时显示眼睛图标，避免已保存的密码被他人直接查看
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
    styled_text_box(ui, p, full_w, &mut form.password, hint, mask_password, eye);

    ui.add_space(8.0);
    ui.label(body_text("运营商出口", 12.0, p.text_secondary));
    ui.add_space(4.0);
    draw_carrier_grid(ui, p, &mut form.carrier);

    ui.add_space(9.0);
    egui::Frame::new()
        .fill(p.subtle)
        .stroke(Stroke::new(1.0_f32, p.divider))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(9, 7))
        .show(ui, |ui| {
            ui.set_width((full_w - 20.0).max(160.0));
            ui.label(body_text(
                "密码仅保存在当前用户的 Windows 凭据管理器中。",
                10.5,
                p.text_muted,
            ));
        });
}

fn draw_carrier_grid(ui: &mut egui::Ui, p: &Palette, carrier: &mut usize) {
    let labels = ["校园用户", "中国电信", "中国联通", "中国移动"];
    let gap = 6.0;
    let btn_w = ((ui.available_width() - gap) * 0.5).max(80.0);
    for row in 0..2 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for col in 0..2 {
                let idx = row * 2 + col;
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
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    labels[idx],
                    font,
                    fg,
                );
                if resp.clicked() {
                    *carrier = idx;
                }
            }
        });
        if row == 0 {
            ui.add_space(gap);
        }
    }
}

fn draw_monitor_tab(ui: &mut egui::Ui, p: &Palette, form: &mut SettingsForm) {
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.label(bold_text("监控与自启策略", 13.0, p.text));
    ui.label(body_text(
        "网络正常时低占用巡检，异常时自动复核重连",
        11.0,
        p.text_muted,
    ));
    ui.add_space(4.0);

    compact_stepper_row(
        ui,
        p,
        "在线检测间隔",
        &mut form.check_interval,
        10..=3600,
        10,
        "秒",
    );
    compact_stepper_row(
        ui,
        p,
        "心跳日志间隔",
        &mut form.heartbeat_minutes,
        1..=1440,
        5,
        "分",
    );
    compact_stepper_row(
        ui,
        p,
        "失败复核次数",
        &mut form.confirm_failures,
        1..=5,
        1,
        "次",
    );
    compact_stepper_row(
        ui,
        p,
        "离线复检间隔",
        &mut form.offline_interval,
        5..=300,
        5,
        "秒",
    );

    hairline(ui, p);

    compact_toggle_row(
        ui,
        p,
        "开机自动启动到托盘",
        "写入 HKCU\\Run（GXNUCampusLoginRS）",
        &mut form.auto_boot,
        true,
        None,
    );
    ui.add_space(4.0);
    compact_toggle_row(
        ui,
        p,
        "启动后立即开始监控",
        "打开软件自动进入在线守护状态",
        &mut form.auto_start_monitoring,
        true,
        None,
    );
}

fn draw_notify_tab(ui: &mut egui::Ui, p: &Palette, form: &mut SettingsForm) -> bool {
    let mut test_clicked = false;
    ui.label(bold_text("断网与恢复提醒", 13.0, p.text));
    ui.label(body_text(
        "断网自动弹窗告知原因，恢复后汇总断网时长",
        11.0,
        p.text_muted,
    ));
    ui.add_space(8.0);

    compact_toggle_row(
        ui,
        p,
        "Windows 系统通知弹窗",
        "原生 Toast 推送断网与恢复提醒",
        &mut form.notify_toast,
        true,
        None,
    );
    ui.add_space(7.0);
    if secondary_button(ui, p, Some(Icon::Bell), "发送测试弹窗").clicked() {
        test_clicked = true;
    }

    hairline(ui, p);

    compact_stepper_row(
        ui,
        p,
        "断网静默时间",
        &mut form.notify_quiet_minutes,
        1..=60,
        1,
        "分",
    );
    ui.add_space(2.0);
    ui.label(body_text(
        "远程推送预留设置，系统弹窗仍会即时提醒。",
        10.5,
        p.text_muted,
    ));

    hairline(ui, p);

    compact_toggle_row(
        ui,
        p,
        "企业微信推送",
        "后续版本补齐 Webhook 通道",
        &mut form.notify_wechat,
        false,
        Some("规划中"),
    );

    test_clicked
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
    let inner_w = (width - 18.0).max(120.0);
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
                        (inner_w - 26.0).max(80.0)
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

fn compact_stepper_row(
    ui: &mut egui::Ui,
    p: &Palette,
    label: &str,
    value: &mut i64,
    range: std::ops::RangeInclusive<i64>,
    step: i64,
    unit: &str,
) {
    let row_w = ui.available_width();
    ui.allocate_ui_with_layout(
        Vec2::new(row_w, 32.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(body_text(label, 12.0, p.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                stepper_field(ui, p, value, range, step, unit);
            });
        },
    );
}

fn compact_toggle_row(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    subtitle: &str,
    value: &mut bool,
    enabled: bool,
    tag: Option<&str>,
) {
    let row_w = ui.available_width();
    let left_w = (row_w - 46.0).max(140.0);
    ui.allocate_ui_with_layout(
        Vec2::new(row_w, 36.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, 34.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        let fg = if enabled { p.text } else { p.text_muted };
                        ui.label(body_text(title, 12.0, fg));
                        if let Some(t) = tag {
                            badge(ui, p.neutral, p.neutral_soft, p.neutral_border, t);
                        }
                    });
                    ui.label(body_text(subtitle, 10.5, p.text_muted));
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                toggle_switch(ui, p, value, enabled);
            });
        },
    );
}

fn hairline(ui: &mut egui::Ui, p: &Palette) {
    ui.add_space(7.0);
    let sep = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
    ui.painter().line_segment(
        [sep.left_center(), sep.right_center()],
        Stroke::new(1.0_f32, p.divider),
    );
    ui.add_space(7.0);
}
