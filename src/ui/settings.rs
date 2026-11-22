//! Windows 11 Fluent 风格设置中心：
//! - 左侧分组目录（账号管理 / 监控与自启 / 外观与显示 / 通知与数据）+ 左下角极简版本号轻水印
//! - 右侧带标题与说明的分组卡片
//! - 按需浮现的底部胶囊保存栏（仅在有未保存修改时出现，避免误触与常态视觉干扰）

use std::collections::HashMap;
use std::path::Path;

use eframe::egui::{self, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::models::{AccountProfile, Settings, UpdateState, UpdateStatus};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::{
    body_text, bold_text, carrier_label, clamp_ui_zoom, with_alpha, Palette, ThemeMode, UiPrefs,
    RADIUS_MD, RADIUS_SM,
};
use crate::ui::widgets::{
    badge, icon_button, nav_rail_item, primary_button, save_button, secondary_button,
    segmented_control, stepper_field, toggle_switch,
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
    /// 当前正在编辑的账号 ID；`None` 表示正在创建新账号。
    pub editing_account_id: Option<String>,
    pub remark: String,
    pub student_id: String,
    pub password: String,
    pub show_password: bool,
    pub carrier: usize,
    /// 最近一次保存前验密被拒的中文说明；存在时在账号卡片内展示醒目红框与「仍要强制保存」兜底入口。
    pub verify_error: Option<String>,
    pub check_interval: i64,
    pub heartbeat_minutes: i64,
    pub offline_interval: i64,
    pub confirm_failures: i64,
    pub auto_start_monitoring: bool,
    pub auto_boot: bool,
    pub auto_check_updates: bool,
    pub notify_toast: bool,
    pub notify_wechat: bool,
    pub notify_quiet_minutes: i64,
    pub monthly_quota_gb: i64,
}

impl SettingsForm {
    pub fn from_settings(settings: &Settings) -> Self {
        let active = settings.active_account();
        let (editing_account_id, remark, student_id, carrier, monthly_quota_gb) = match active {
            Some(acc) => (
                Some(acc.id.clone()),
                acc.remark.clone(),
                acc.student_id.clone(),
                acc.carrier.parse::<usize>().unwrap_or(2).min(3),
                acc.monthly_quota_gb,
            ),
            None => (
                None,
                String::new(),
                settings.student_id.clone(),
                settings.carrier.parse::<usize>().unwrap_or(2).min(3),
                settings.monthly_quota_gb,
            ),
        };
        Self {
            tab: SettingsTab::Account,
            editing_account_id,
            remark,
            student_id,
            password: String::new(),
            show_password: false,
            carrier,
            verify_error: None,
            check_interval: settings.check_interval,
            heartbeat_minutes: (settings.heartbeat_interval / 60).clamp(1, 1440),
            offline_interval: settings.offline_interval,
            confirm_failures: settings.confirm_failures,
            auto_start_monitoring: settings.auto_start_monitoring,
            auto_boot: settings.auto_boot,
            auto_check_updates: settings.auto_check_updates,
            notify_toast: settings.notify_toast,
            notify_wechat: settings.notify_wechat,
            notify_quiet_minutes: settings.notify_quiet_minutes,
            monthly_quota_gb,
        }
    }

    pub fn reset_from(&mut self, settings: &Settings) {
        let tab = self.tab;
        *self = Self::from_settings(settings);
        self.tab = tab;
    }

    pub fn load_account_draft(&mut self, profile: &AccountProfile) {
        self.editing_account_id = Some(profile.id.clone());
        self.remark = profile.remark.clone();
        self.student_id = profile.student_id.clone();
        self.password.clear();
        self.show_password = false;
        self.carrier = profile.carrier.parse::<usize>().unwrap_or(2).min(3);
        self.verify_error = None;
        self.monthly_quota_gb = profile.monthly_quota_gb;
    }

    pub fn start_new_account(&mut self) {
        self.editing_account_id = None;
        self.remark.clear();
        self.student_id.clear();
        self.password.clear();
        self.show_password = false;
        self.carrier = 2;
        self.verify_error = None;
        self.monthly_quota_gb = 0;
    }

    /// 将全局监控/自启/通知参数合并回给定配置，保留已有账号列表与活跃账号。
    pub fn to_global_settings(&self, base: &Settings) -> Settings {
        Settings {
            check_interval: self.check_interval,
            heartbeat_interval: self.heartbeat_minutes.clamp(1, 1440) * 60,
            offline_interval: self.offline_interval,
            confirm_failures: self.confirm_failures,
            auto_start_monitoring: self.auto_start_monitoring,
            auto_boot: self.auto_boot,
            auto_check_updates: self.auto_check_updates,
            notify_toast: self.notify_toast,
            notify_wechat: self.notify_wechat,
            notify_quiet_minutes: self.notify_quiet_minutes,
            ..base.clone()
        }
        .normalized()
    }

    pub fn draft_profile(&self, id: String) -> AccountProfile {
        AccountProfile {
            id,
            remark: self.remark.trim().to_string(),
            student_id: self.student_id.trim().to_string(),
            carrier: self.carrier.to_string(),
            monthly_quota_gb: self.monthly_quota_gb,
        }
        .normalized()
    }

    pub fn is_global_dirty(&self, saved: &Settings) -> bool {
        let current = self.to_global_settings(saved);
        current.check_interval != saved.check_interval
            || current.heartbeat_interval != saved.heartbeat_interval
            || current.offline_interval != saved.offline_interval
            || current.confirm_failures != saved.confirm_failures
            || current.auto_start_monitoring != saved.auto_start_monitoring
            || current.auto_boot != saved.auto_boot
            || current.auto_check_updates != saved.auto_check_updates
            || current.notify_toast != saved.notify_toast
            || current.notify_quiet_minutes != saved.notify_quiet_minutes
    }

    pub fn is_account_draft_dirty(&self, saved: &Settings) -> bool {
        if let Some(id) = self.editing_account_id.as_deref() {
            match saved.find_account(id) {
                Some(acc) => {
                    self.remark.trim() != acc.remark.trim()
                        || self.student_id.trim() != acc.student_id.trim()
                        || self.carrier.to_string() != acc.carrier
                        || self.monthly_quota_gb.clamp(0, 2000) != acc.monthly_quota_gb
                        || !self.password.is_empty()
                }
                None => true,
            }
        } else {
            !self.remark.trim().is_empty()
                || !self.student_id.trim().is_empty()
                || !self.password.is_empty()
                || self.monthly_quota_gb != 0
        }
    }

    pub fn is_dirty(&self, saved: &Settings) -> bool {
        self.is_global_dirty(saved) || self.is_account_draft_dirty(saved)
    }
}

#[derive(Default)]
pub struct SettingsActions {
    pub test_toast: bool,
    pub save: bool,
    pub save_profile: bool,
    pub force_save_profile: bool,
    /// 用户在表单中刚从其他出口点击切到「校园网」（`0`）时置为 `true`，触发防封弹窗提醒。
    pub selected_campus_carrier: bool,
    pub switch_account: Option<String>,
    pub delete_account: Option<String>,
    pub reset: bool,
    pub ui_prefs_changed: bool,
    pub set_window_size: Option<[f32; 2]>,
    pub check_update: bool,
    pub open_update_modal: bool,
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
    account_creds: &HashMap<String, bool>,
    update_state: &UpdateState,
    needs_setup: bool,
    saving: bool,
    verifying_account: bool,
    total_height: f32,
) -> SettingsActions {
    let mut actions = SettingsActions::default();
    let total_w = ui.available_width();
    let nav_w = 166.0_f32.min((total_w * 0.28).max(138.0));
    let content_w = (total_w - nav_w - 1.0).max(300.0);
    let global_dirty = form.is_global_dirty(saved);
    let draft_dirty = form.is_account_draft_dirty(saved);
    let show_floating_bar = global_dirty
        || (draft_dirty && form.tab != SettingsTab::Account)
        || (needs_setup && form.tab != SettingsTab::Account);

    let left_wm_h = 34.0_f32;
    let left_nav_h = (total_height - left_wm_h).max(180.0);

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;

        // ── 左侧分组目录（无底部分割线 + 左下角极简版本号轻水印） ──
        ui.allocate_ui_with_layout(
            Vec2::new(nav_w, total_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.painter()
                    .rect_filled(ui.max_rect(), CornerRadius::ZERO, p.sidebar);
                ui.spacing_mut().item_spacing.y = 0.0;

                // 上部导航目录
                ui.allocate_ui_with_layout(
                    Vec2::new(nav_w, left_nav_h),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::Frame::new()
                            .fill(p.sidebar)
                            .inner_margin(egui::Margin {
                                left: 10,
                                right: 10,
                                top: 12,
                                bottom: 10,
                            })
                            .show(ui, |ui| {
                                let inner_w = (nav_w - 20.0).max(118.0);
                                let inner_h = (left_nav_h - 22.0).max(150.0);
                                ui.set_min_size(Vec2::new(inner_w, inner_h));
                                ui.set_max_size(Vec2::new(inner_w, inner_h));
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
                            });
                    },
                );

                // 左下角仅保留极简版本号轻水印（无分割线、低对比度融入侧栏底色）
                let has_new_ver = update_state.has_visible_update(&saved.skipped_update_version);
                let (wm_rect, wm_resp) = ui.allocate_exact_size(
                    Vec2::new(nav_w, left_wm_h),
                    if has_new_ver {
                        Sense::click()
                    } else {
                        Sense::hover()
                    },
                );
                ui.painter()
                    .rect_filled(wm_rect, CornerRadius::ZERO, p.sidebar);
                let is_dark = p.sidebar.r() < 128;
                let alpha: u8 = if wm_resp.hovered() {
                    if is_dark {
                        162
                    } else {
                        198
                    }
                } else if is_dark {
                    112
                } else {
                    152
                };
                let wm_color = if has_new_ver {
                    with_alpha(p.accent, if wm_resp.hovered() { 235 } else { 195 })
                } else {
                    with_alpha(p.text_muted, alpha)
                };
                let ver_txt = if has_new_ver {
                    format!("v{} ↑", crate::VERSION)
                } else {
                    format!("v{}", crate::VERSION)
                };
                let vg = ui.painter().layout_no_wrap(
                    ver_txt,
                    egui::FontId::proportional(10.0),
                    wm_color,
                );
                let lx = wm_rect.left() + 18.0;
                let cy = wm_rect.bottom() - 15.0;
                ui.painter()
                    .galley(Pos2::new(lx, cy - vg.size().y * 0.5), vg, wm_color);

                if has_new_ver {
                    let wm_resp = wm_resp
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text("检测到新版本，点击查看更新详情");
                    if wm_resp.clicked() {
                        actions.open_update_modal = true;
                    }
                } else {
                    wm_resp.on_hover_text(format!("{} v{}", crate::APP_NAME, crate::VERSION));
                }
            },
        );

        // 左侧栏与主内容区的 1px 竖向分割线
        let rail_sep = ui.allocate_space(Vec2::new(1.0, total_height)).1;
        ui.painter().line_segment(
            [rail_sep.center_top(), rail_sep.center_bottom()],
            Stroke::new(1.0_f32, p.border),
        );

        // ── 右侧内容区（方案 B：平时一通到底无底栏，有未保存修改时底部浮出胶囊条） ──
        ui.allocate_ui_with_layout(
            Vec2::new(content_w, total_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let pane_rect = ui.max_rect();

                const PANE_PAD: i8 = 16;
                const PANE_PAD_F: f32 = 16.0;
                const TOP_PAD: i8 = 14;
                const TOP_PAD_F: f32 = 14.0;
                let target_bottom_y = if show_floating_bar {
                    None
                } else {
                    Some(pane_rect.bottom() - PANE_PAD_F)
                };

                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(egui::Margin {
                        left: PANE_PAD,
                        right: PANE_PAD,
                        top: TOP_PAD,
                        bottom: PANE_PAD,
                    })
                    .show(ui, |ui| {
                        let inner_w = (content_w - PANE_PAD_F * 2.0).floor().max(260.0);
                        let inner_h = (total_height - TOP_PAD_F - PANE_PAD_F).max(160.0);
                        ui.set_min_size(Vec2::new(inner_w, inner_h));
                        ui.set_max_size(Vec2::new(inner_w, inner_h));
                        // 悬浮细滚动条，绝不吞占右侧内边距导致卡片右边距窄于左边距/下边距
                        ui.style_mut().spacing.scroll = egui::style::ScrollStyle::floating();
                        egui::ScrollArea::vertical()
                            .id_salt(("settings_content_scroll", form.tab.index()))
                            .max_height(inner_h)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(inner_w);
                                ui.set_max_width(inner_w);
                                match form.tab {
                                    SettingsTab::Account => draw_account_page(
                                        ui,
                                        p,
                                        form,
                                        saved,
                                        account_creds,
                                        verifying_account,
                                        target_bottom_y,
                                        &mut actions,
                                    ),
                                    SettingsTab::Monitor => {
                                        draw_monitor_page(ui, p, form, update_state, &mut actions)
                                    }
                                    SettingsTab::Display => {
                                        draw_display_page(ui, p, ui_prefs, &mut actions)
                                    }
                                    SettingsTab::Notify => {
                                        if draw_notify_data_page(ui, p, form, app_dir, log_path) {
                                            actions.test_toast = true;
                                        }
                                    }
                                }
                                if show_floating_bar {
                                    ui.add_space(52.0);
                                }
                            });
                    });

                if show_floating_bar {
                    let pill_h = 42.0_f32;
                    let pill_rect = Rect::from_min_max(
                        Pos2::new(pane_rect.left() + 18.0, pane_rect.bottom() - pill_h - 12.0),
                        Pos2::new(pane_rect.right() - 18.0, pane_rect.bottom() - 12.0),
                    );
                    let shadow = match p.mode {
                        ThemeMode::Light => egui::Shadow {
                            offset: [0, 4],
                            blur: 14,
                            spread: 0,
                            color: with_alpha(egui::Color32::BLACK, 28),
                        },
                        ThemeMode::Dark => egui::Shadow {
                            offset: [0, 6],
                            blur: 18,
                            spread: 0,
                            color: with_alpha(egui::Color32::BLACK, 135),
                        },
                    };
                    ui.painter()
                        .add(shadow.as_shape(pill_rect, CornerRadius::same(10)));
                    ui.painter().rect(
                        pill_rect,
                        CornerRadius::same(10),
                        p.card,
                        Stroke::new(1.0_f32, p.warn_border),
                        StrokeKind::Inside,
                    );
                    let pcy = pill_rect.center().y;
                    ui.painter().circle_filled(
                        Pos2::new(pill_rect.left() + 16.0, pcy),
                        3.4,
                        p.warn,
                    );
                    let msg = if verifying_account {
                        "正在核验账号密码…"
                    } else if global_dirty {
                        "有未保存的设置修改"
                    } else {
                        "账号编辑草稿尚未保存"
                    };
                    let sg = ui.painter().layout_no_wrap(
                        msg.to_string(),
                        egui::FontId::new(11.5, crate::ui::theme::bold_family()),
                        p.text,
                    );
                    ui.painter().galley(
                        Pos2::new(pill_rect.left() + 26.0, pcy - sg.size().y * 0.5),
                        sg,
                        p.text,
                    );

                    let mut btn_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(pill_rect.shrink2(Vec2::new(10.0, 0.0)))
                            .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    );
                    btn_ui.spacing_mut().item_spacing.x = 8.0;
                    let save_resp = btn_ui
                        .add_enabled_ui(!saving && !verifying_account, |ui| {
                            if verifying_account {
                                primary_button(ui, p, Some(Icon::Refresh), "正在验证…")
                            } else if needs_setup {
                                primary_button(ui, p, Some(Icon::Check), "保存并开始守护")
                            } else {
                                save_button(ui, p, true)
                            }
                        })
                        .inner;
                    if save_resp.clicked() {
                        actions.save = true;
                    }
                    if !verifying_account
                        && secondary_button(&mut btn_ui, p, None, "还原").clicked()
                    {
                        actions.reset = true;
                    }
                }
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
    let outer_w = ui.available_width().floor();
    // Frame 总水平边距 = inner_margin (14*2) + stroke (1*2) = 30px，精确锁死卡片外宽防止子控件撑偏右边线
    let inner_w = (outer_w - 30.0).max(180.0);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(RADIUS_MD))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width(inner_w);
            ui.set_max_width(inner_w);
            body(ui);
        });
}

#[allow(clippy::too_many_arguments)]
fn draw_account_page(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &mut SettingsForm,
    saved: &Settings,
    account_creds: &HashMap<String, bool>,
    verifying_account: bool,
    target_bottom_y: Option<f32>,
    actions: &mut SettingsActions,
) {
    page_header(
        ui,
        p,
        "账号管理",
        "支持保存多个校园网账号与备注名，主页及托盘浮窗可一键切号；各账号密码独立加密存储于 Windows 凭据管理器。",
    );

    // 1. 已保存账号目录卡
    group_card(ui, p, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(bold_text("已保存账号", 12.5, p.text));
                let count_text = if saved.accounts.is_empty() {
                    "尚未添加".to_string()
                } else {
                    format!("共 {} 个 · 点击条目编辑", saved.accounts.len())
                };
                ui.label(body_text(count_text, 11.0, p.text_muted));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if form.editing_account_id.is_none() && !saved.accounts.is_empty() {
                        badge(ui, p.accent, p.accent_soft, p.accent_border, "正在新增账号");
                    } else if draw_compact_action_btn(ui, p, Some(Icon::Plus), "新增账号", true)
                        .clicked()
                    {
                        form.start_new_account();
                    }
                });
            },
        );

        if saved.accounts.is_empty() {
            egui::Frame::new()
                .fill(p.subtle)
                .stroke(Stroke::new(1.0_f32, p.divider))
                .corner_radius(CornerRadius::same(RADIUS_SM))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(body_text(
                        "尚未保存校园网账号，请在下方填写备注名、学号与密码后保存。",
                        11.5,
                        p.text_muted,
                    ));
                });
        } else {
            ui.spacing_mut().item_spacing.y = 6.0;
            for acc in &saved.accounts {
                let is_active = acc.id == saved.active_account_id;
                let is_editing = form.editing_account_id.as_deref() == Some(acc.id.as_str());
                let has_cred = account_creds.get(&acc.id).copied().unwrap_or(false);
                draw_saved_account_row(ui, p, acc, is_active, is_editing, has_cred, form, actions);
            }
        }
    });

    ui.add_space(10.0);

    // 2. 账号编辑 / 新增表单卡
    let is_new = form.editing_account_id.is_none();
    let editing_saved = form
        .editing_account_id
        .as_deref()
        .and_then(|id| saved.find_account(id));
    let editing_has_cred = editing_saved.is_some_and(|acc| {
        account_creds.get(&acc.id).copied().unwrap_or(false)
            && acc.student_id.trim() == form.student_id.trim()
    });
    let draft_dirty = form.is_account_draft_dirty(saved);

    group_card(ui, p, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let heading = match editing_saved {
                    Some(acc) => format!("编辑账号 · {}", acc.display_name()),
                    None => "新增校园网账号".to_string(),
                };
                ui.label(bold_text(heading, 12.5, p.text));
                if let Some(acc) = editing_saved {
                    if acc.id == saved.active_account_id {
                        badge(ui, p.ok, p.ok_soft, p.ok_border, "当前活跃");
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if is_new
                        && !saved.accounts.is_empty()
                        && draw_compact_action_btn(ui, p, None, "取消新增", false).clicked()
                    {
                        if let Some(active) = saved.active_account() {
                            form.load_account_draft(active);
                        }
                    }
                });
            },
        );

        let full_w = ui.available_width();
        let col_gap = 12.0_f32;
        let left_col_w = ((full_w - col_gap) * 0.5).floor().max(120.0);
        let right_col_w = (full_w - col_gap - left_col_w).max(120.0);

        // 第一行：校园网学号（左，与下方登录密码上下对齐） | 备注名称（右）
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = col_gap;
            ui.allocate_ui_with_layout(
                Vec2::new(left_col_w, 52.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.label(body_text("校园网学号", 11.5, p.text_secondary));
                    ui.add_space(4.0);
                    styled_text_box(
                        ui,
                        p,
                        left_col_w,
                        &mut form.student_id,
                        "输入校园网学号",
                        false,
                        None,
                    );
                },
            );

            ui.allocate_ui_with_layout(
                Vec2::new(right_col_w, 52.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.label(body_text(
                        "备注名称（选填，优先显示）",
                        11.5,
                        p.text_secondary,
                    ));
                    ui.add_space(4.0);
                    styled_text_box(
                        ui,
                        p,
                        right_col_w,
                        &mut form.remark,
                        "如：宿舍联通、实验室电信",
                        false,
                        None,
                    );
                },
            );
        });

        ui.add_space(2.0);

        // 第二行：登录密码 | 独立凭据隔离状态
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = col_gap;
            ui.allocate_ui_with_layout(
                Vec2::new(left_col_w, 52.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.label(body_text("登录密码", 11.5, p.text_secondary));
                    ui.add_space(4.0);
                    let hint = if editing_has_cred {
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
                    styled_text_box(
                        ui,
                        p,
                        left_col_w,
                        &mut form.password,
                        hint,
                        mask_password,
                        eye,
                    );
                },
            );

            ui.allocate_ui_with_layout(
                Vec2::new(right_col_w, 52.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.label(body_text("凭据安全存储", 11.5, p.text_secondary));
                    ui.add_space(4.0);
                    let (fg, bg, border, status_txt) = if !form.password.is_empty() {
                        (
                            p.accent,
                            p.accent_soft,
                            p.accent_border,
                            "保存后将加密更新 DPAPI 凭据",
                        )
                    } else if editing_has_cred {
                        (
                            p.ok,
                            p.ok_soft,
                            p.ok_border,
                            "已加密保存在 Windows 凭据管理器",
                        )
                    } else {
                        (
                            p.warn,
                            p.warn_soft,
                            p.warn_border,
                            "尚未保存密码，请输入后保存",
                        )
                    };
                    let (box_rect, _) =
                        ui.allocate_exact_size(Vec2::new(right_col_w, 30.0), Sense::hover());
                    ui.painter().rect(
                        box_rect,
                        CornerRadius::same(RADIUS_SM),
                        bg,
                        Stroke::new(1.0_f32, border),
                        StrokeKind::Inside,
                    );
                    let ico_r = Rect::from_center_size(
                        Pos2::new(box_rect.left() + 15.0, box_rect.center().y),
                        Vec2::splat(12.5),
                    );
                    icons::paint(ui.painter(), ico_r, Icon::Key, fg);
                    let font = egui::FontId::proportional(11.0);
                    let galley = ui
                        .painter()
                        .layout_no_wrap(status_txt.to_string(), font, fg);
                    ui.painter().galley(
                        Pos2::new(
                            box_rect.left() + 27.0,
                            box_rect.center().y - galley.size().y * 0.5,
                        ),
                        galley,
                        fg,
                    );
                },
            );
        });

        ui.add_space(2.0);
        ui.label(body_text("运营商出口", 11.5, p.text_secondary));
        if draw_carrier_row(ui, p, &mut form.carrier) && form.carrier == 0 {
            actions.selected_campus_carrier = true;
        }
        if form.carrier == 0 {
            ui.add_space(3.0);
            ui.label(body_text(
                "⚠ 安全提示：使用「校园网」出口时请关闭本机 VPN / 代理软件，避免校园网账号被封禁",
                10.5,
                p.warn,
            ));
        }

        // 若上次点击保存时当场验密未通过，展示醒目红色原因卡片与「仍要强制保存」兜底按钮
        if let Some(err_msg) = form.verify_error.clone() {
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(p.danger_soft)
                .stroke(Stroke::new(1.0_f32, p.danger_border))
                .corner_radius(CornerRadius::same(RADIUS_SM))
                .inner_margin(egui::Margin::symmetric(12, 9))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (ir, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                        icons::paint(ui.painter(), ir, Icon::Close, p.danger);
                        let text_w = (ui.available_width() - 96.0).max(140.0);
                        ui.allocate_ui_with_layout(
                            Vec2::new(text_w, 32.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.add(
                                    egui::Label::new(bold_text(
                                        format!("登录校验未通过：{err_msg}"),
                                        11.5,
                                        p.danger,
                                    ))
                                    .wrap(),
                                );
                                ui.label(body_text(
                                    "请核对上方学号、密码（点密码框右侧眼睛可查看）与运营商后重新点击保存",
                                    10.5,
                                    p.text_secondary,
                                ));
                            },
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !verifying_account
                                && draw_compact_action_btn(ui, p, None, "仍要强制保存", false)
                                    .on_hover_text(
                                        "若确认运营商宽带密码与自助服务密码不同，可跳过预检强制保存",
                                    )
                                    .clicked()
                            {
                                actions.force_save_profile = true;
                            }
                        });
                    });
                });
        }

        // 当页面无需滚动且底部剩余少量余量时，将底部操作行自然贴底，使卡片下边距与左右边距严格等宽
        if let Some(target_y) = target_bottom_y {
            let tail_h = 62.0_f32;
            let extra = target_y - (ui.cursor().top() + tail_h);
            if (0.5..=80.0).contains(&extra) {
                ui.add_space(extra);
            }
        }

        row_divider(ui, p);

        // 卡片底部操作行：左安全隔离说明 + 右还原/保存账号按钮
        let foot_w = ui.available_width();
        let right_reserve = if draft_dirty && !verifying_account {
            216.0_f32
        } else {
            136.0_f32
        };
        ui.allocate_ui_with_layout(
            Vec2::new(foot_w, 30.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let note_w = (foot_w - right_reserve).max(120.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(note_w, 30.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(body_text(
                                "保存时将自动无损核验学号与密码；密码按账号 ID 独立加密存储，绝不明文落盘",
                                10.5,
                                p.text_muted,
                            ))
                            .truncate(),
                        );
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let btn_label = if verifying_account {
                        "正在核验账号…"
                    } else if is_new {
                        "验证并添加账号"
                    } else {
                        "验证并保存账号"
                    };
                    let clicked = ui
                        .add_enabled_ui(!verifying_account, |ui| {
                            if is_new || draft_dirty {
                                primary_button(ui, p, Some(Icon::Check), btn_label).clicked()
                            } else {
                                secondary_button(ui, p, Some(Icon::Check), btn_label).clicked()
                            }
                        })
                        .inner;
                    if clicked {
                        actions.save_profile = true;
                    }
                    if draft_dirty
                        && !verifying_account
                        && secondary_button(ui, p, None, "还原").clicked()
                    {
                        if let Some(id) = form.editing_account_id.clone() {
                            if let Some(acc) = saved.find_account(&id) {
                                form.load_account_draft(acc);
                            }
                        } else {
                            form.start_new_account();
                        }
                    }
                });
            },
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_saved_account_row(
    ui: &mut egui::Ui,
    p: &Palette,
    acc: &AccountProfile,
    is_active: bool,
    is_editing: bool,
    has_cred: bool,
    form: &mut SettingsForm,
    actions: &mut SettingsActions,
) {
    let row_w = ui.available_width();
    let row_h = 46.0_f32;
    let (row_rect, _) = ui.allocate_exact_size(Vec2::new(row_w, row_h), Sense::hover());
    let hovered = ui.rect_contains_pointer(row_rect);

    let (fill, stroke) = if is_editing {
        (p.accent_soft, Stroke::new(1.1_f32, p.accent_border))
    } else if hovered {
        (p.card_hover, Stroke::new(1.0_f32, p.border_strong))
    } else {
        (p.subtle, Stroke::new(1.0_f32, p.divider))
    };
    ui.painter().rect(
        row_rect,
        CornerRadius::same(RADIUS_SM),
        fill,
        stroke,
        StrokeKind::Inside,
    );

    if is_active {
        let bar = Rect::from_min_max(
            Pos2::new(row_rect.left() + 1.5, row_rect.top() + 8.0),
            Pos2::new(row_rect.left() + 4.5, row_rect.bottom() - 8.0),
        );
        ui.painter().rect_filled(bar, CornerRadius::same(2), p.ok);
    }

    let inner = row_rect.shrink2(Vec2::new(10.0, 5.0));
    let right_reserved = if is_active { 122.0_f32 } else { 174.0_f32 };
    let left_w = (inner.width() - right_reserved).max(140.0);

    // 左侧可点击信息区（点击选中该账号到下方表单编辑）
    let left_rect = Rect::from_min_size(inner.min, Vec2::new(left_w, inner.height()));
    let select_resp = ui
        .interact(
            left_rect,
            ui.id().with(("acct_row_select", &acc.id)),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    if select_resp.clicked() {
        form.load_account_draft(acc);
    }

    // 头像圆角底块
    let avatar_rect = Rect::from_center_size(
        Pos2::new(left_rect.left() + 14.0, left_rect.center().y),
        Vec2::splat(26.0),
    );
    let (av_bg, av_fg) = if is_active {
        (p.ok_soft, p.ok)
    } else {
        (with_alpha(p.accent, 22), p.accent)
    };
    ui.painter()
        .rect_filled(avatar_rect, CornerRadius::same(7), av_bg);
    icons::paint(ui.painter(), avatar_rect.shrink(6.0), Icon::User, av_fg);

    let text_x = avatar_rect.right() + 9.0;
    let title_y = left_rect.top() + 9.5;
    let sub_y = left_rect.bottom() - 8.5;

    let display_name = acc.display_name();
    let title_font = egui::FontId::new(12.5, crate::ui::theme::bold_family());
    let title_galley = ui
        .painter()
        .layout_no_wrap(display_name, title_font, p.text);
    let title_w = title_galley.size().x;
    ui.painter().galley(
        Pos2::new(text_x, title_y - title_galley.size().y * 0.5),
        title_galley,
        p.text,
    );

    if is_active {
        let tag_font = egui::FontId::new(10.0, crate::ui::theme::bold_family());
        let tag_galley = ui
            .painter()
            .layout_no_wrap("当前使用".to_string(), tag_font, p.ok);
        let pill_rect = Rect::from_center_size(
            Pos2::new(
                text_x + title_w + 7.0 + (tag_galley.size().x + 10.0) * 0.5,
                title_y,
            ),
            Vec2::new(tag_galley.size().x + 10.0, 17.0),
        );
        if pill_rect.right() <= left_rect.right() {
            ui.painter().rect(
                pill_rect,
                CornerRadius::same(4),
                p.ok_soft,
                Stroke::new(1.0_f32, p.ok_border),
                StrokeKind::Inside,
            );
            ui.painter().galley(
                Pos2::new(
                    pill_rect.center().x - tag_galley.size().x * 0.5,
                    pill_rect.center().y - tag_galley.size().y * 0.5,
                ),
                tag_galley,
                p.ok,
            );
        }
    }

    let quota_part = if acc.monthly_quota_gb > 0 {
        format!(" · 限额 {} GB", acc.monthly_quota_gb)
    } else {
        String::new()
    };
    let subtitle = if acc.remark.trim().is_empty() {
        format!("{}{}", carrier_label(&acc.carrier), quota_part)
    } else {
        format!(
            "{} · {}{}",
            acc.masked_student_id(),
            carrier_label(&acc.carrier),
            quota_part
        )
    };
    ui.painter().text(
        Pos2::new(text_x, sub_y),
        egui::Align2::LEFT_CENTER,
        subtitle,
        egui::FontId::proportional(10.5),
        p.text_muted,
    );

    // 右侧操作区（凭据状态徽章 + 一键切换 + 删除）
    let right_rect = Rect::from_min_max(Pos2::new(left_rect.right() + 6.0, inner.top()), inner.max);
    let mut right_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(right_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right_ui.spacing_mut().item_spacing.x = 6.0;

    if icon_button(
        &mut right_ui,
        p,
        Icon::Trash,
        Vec2::splat(26.0),
        13.0,
        true,
        "删除此账号及其保存的密码",
    )
    .clicked()
    {
        actions.delete_account = Some(acc.id.clone());
    }

    if !is_active
        && draw_compact_action_btn(&mut right_ui, p, None, "切换", has_cred)
            .on_hover_text(if has_cred {
                "立即下线当前会话并切换登录此账号"
            } else {
                "该账号尚未保存密码，点击前往填写"
            })
            .clicked()
    {
        actions.switch_account = Some(acc.id.clone());
    }

    if has_cred {
        badge(&mut right_ui, p.ok, p.ok_soft, p.ok_border, "已加密");
    } else {
        badge(&mut right_ui, p.warn, p.warn_soft, p.warn_border, "缺密码");
    }
}

fn draw_compact_action_btn(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: Option<Icon>,
    label: &str,
    accent: bool,
) -> egui::Response {
    let font = egui::FontId::new(11.0, crate::ui::theme::bold_family());
    let fg = if accent { p.accent } else { p.text_secondary };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    let icon_w = if icon.is_some() { 15.0 } else { 0.0 };
    let w = (galley.size().x + icon_w + 16.0).max(46.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let (fill, stroke, text_c) = if accent {
        if resp.hovered() {
            (p.accent, Stroke::NONE, p.on_accent)
        } else {
            (
                p.accent_soft,
                Stroke::new(1.0_f32, p.accent_border),
                p.accent,
            )
        }
    } else if resp.hovered() {
        (p.card_hover, Stroke::new(1.0_f32, p.border_strong), p.text)
    } else {
        (p.card, Stroke::new(1.0_f32, p.border), p.text_secondary)
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        fill,
        stroke,
        StrokeKind::Inside,
    );
    let content_w = galley.size().x + icon_w;
    let mut x = rect.center().x - content_w * 0.5;
    if let Some(ic) = icon {
        let ir = Rect::from_center_size(Pos2::new(x + 5.5, rect.center().y), Vec2::splat(11.0));
        icons::paint(ui.painter(), ir, ic, text_c);
        x += icon_w;
    }
    ui.painter().galley(
        Pos2::new(x, rect.center().y - galley.size().y * 0.5),
        galley,
        text_c,
    );
    resp
}

fn draw_carrier_row(ui: &mut egui::Ui, p: &Palette, carrier: &mut usize) -> bool {
    let mut changed = false;
    let labels = ["校园网", "中国电信", "中国联通", "中国移动"];
    let gap = 8.0_f32;
    let total_w = ui.available_width();
    let usable_w = (total_w - gap * 3.0).max(296.0);
    let base_w = (usable_w * 0.25).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (idx, label) in labels.iter().enumerate() {
            let active = *carrier == idx;
            let btn_w = if idx == 3 {
                (usable_w - base_w * 3.0).max(base_w)
            } else {
                base_w
            };
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
            if resp.clicked() && *carrier != idx {
                *carrier = idx;
                changed = true;
            }
        }
    });
    changed
}

fn draw_monitor_page(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &mut SettingsForm,
    update_state: &UpdateState,
    actions: &mut SettingsActions,
) {
    page_header(
        ui,
        p,
        "监控与自启",
        "自定义网络巡检周期、防抖复核策略、Windows 登录自启与版本自动更新。",
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

    ui.add_space(10.0);

    group_card(ui, p, |ui| {
        fluent_toggle_row(
            ui,
            p,
            "自动检查新版本",
            "联网状态下每 12 小时静默检查 GitHub Release 新版本，仅温和提醒绝不擅自重启断网",
            &mut form.auto_check_updates,
            true,
            None,
        );
        row_divider(ui, p);

        let row_w = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(row_w, 40.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let left_w = (row_w - 116.0).max(150.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(left_w, 40.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(bold_text(
                            format!("当前版本 v{}", crate::VERSION),
                            12.5,
                            p.text,
                        ));
                        let (sub_txt, sub_col) = match update_state.status {
                            UpdateStatus::Checking => {
                                ("正在连接 GitHub 检查新版本…".to_string(), p.accent)
                            }
                            UpdateStatus::Downloading => {
                                let pct = update_state
                                    .progress_ratio()
                                    .map(|r| format!(" ({}%)", (r * 100.0).round() as i32))
                                    .unwrap_or_default();
                                (format!("正在下载更新包{pct}…"), p.accent)
                            }
                            UpdateStatus::Applying => ("正在替换安装并重启…".to_string(), p.ok),
                            UpdateStatus::Available => {
                                let ver = update_state
                                    .release
                                    .as_ref()
                                    .map(|r| r.version.as_str())
                                    .unwrap_or("");
                                (format!("发现新版本 v{ver}，点击右侧即可原位覆盖升级"), p.ok)
                            }
                            UpdateStatus::Error if !update_state.message.is_empty() => {
                                (update_state.message.clone(), p.warn)
                            }
                            UpdateStatus::UpToDate => {
                                let checked_str = update_state
                                    .last_checked_unix
                                    .and_then(|ts| {
                                        chrono::DateTime::from_timestamp(ts, 0).map(|dt| {
                                            dt.with_timezone(&chrono::Local)
                                                .format(" · 上次检查 %H:%M")
                                                .to_string()
                                        })
                                    })
                                    .unwrap_or_default();
                                (format!("当前已是最新版本{checked_str}"), p.text_muted)
                            }
                            _ => (
                                "点击右侧可立即前往 GitHub Release 检查是否有新版本".to_string(),
                                p.text_muted,
                            ),
                        };
                        ui.add(egui::Label::new(body_text(sub_txt, 10.8, sub_col)).truncate());
                    },
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match update_state.status {
                        UpdateStatus::Available | UpdateStatus::Downloading => {
                            let lbl = if update_state.status == UpdateStatus::Downloading {
                                "查看进度"
                            } else {
                                "查看新版本"
                            };
                            if draw_compact_action_btn(ui, p, Some(Icon::Download), lbl, true)
                                .clicked()
                            {
                                actions.open_update_modal = true;
                            }
                        }
                        UpdateStatus::Checking | UpdateStatus::Applying => {
                            badge(ui, p.accent, p.accent_soft, p.accent_border, "处理中…");
                        }
                        _ => {
                            if draw_compact_action_btn(
                                ui,
                                p,
                                Some(Icon::Refresh),
                                "检查更新",
                                false,
                            )
                            .clicked()
                            {
                                actions.check_update = true;
                            }
                        }
                    }
                });
            },
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
                        ("宽松", [960.0, 660.0], "调整为 960 × 660"),
                        ("标准", [840.0, 610.0], "调整为默认 840 × 610"),
                        (
                            "紧凑",
                            [740.0, 540.0],
                            "调整为紧凑 740 × 540（推荐配合 85% 缩放）",
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
            "主页展示「账号流量与在线设备」卡片",
            "展示校园网自助服务的运营商实际流量、校园网套餐余量与当前在线终端；关闭后仅保留连接稳定性与运行日志",
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
                    if secondary_button(ui, p, Some(Icon::ExternalLink), "定位日志")
                        .on_hover_text("在文件管理器中打开并选中 app.log")
                        .clicked()
                    {
                        crate::logging::reveal_in_explorer(log_path);
                    }
                    if secondary_button(ui, p, Some(Icon::Folder), "打开目录").clicked() {
                        let _ = std::process::Command::new("explorer.exe")
                            .arg(crate::logging::normalize_win32_path(app_dir))
                            .spawn();
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
                        ui.label(bold_text("流量与设备数据源", 12.0, p.text));
                        ui.label(body_text(
                            "账号流量与在线终端来自校园网自助服务（5 分钟低频缓存）；离校或未同步时自动降级展示本机网卡参考",
                            10.5,
                            p.text_muted,
                        ));
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    badge(ui, p.ok, p.ok_soft, p.ok_border, "自助服务直连");
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
    let (box_rect, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
    ui.painter().rect(
        box_rect,
        CornerRadius::same(RADIUS_SM),
        p.input_bg,
        Stroke::new(1.0_f32, p.border),
        StrokeKind::Inside,
    );

    let inner_rect = box_rect.shrink2(Vec2::new(9.0, 4.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 4.0;
    let edit_w = if eye_toggle.is_some() {
        (inner_rect.width() - 24.0).max(60.0)
    } else {
        inner_rect.width()
    };
    child.add_sized(
        [edit_w, 20.0],
        egui::TextEdit::singleline(value)
            .password(password)
            .hint_text(egui::RichText::new(hint).color(p.text_muted))
            .vertical_align(egui::Align::Center)
            .margin(egui::Margin::ZERO)
            .frame(false),
    );
    if let Some(show) = eye_toggle {
        let (rect, resp) = child.allocate_exact_size(Vec2::splat(20.0), Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        let color = if resp.hovered() { p.text } else { p.text_muted };
        let icon = if *show { Icon::EyeOff } else { Icon::Eye };
        icons::paint(child.painter(), rect.shrink(2.0), icon, color);
        if resp.clicked() {
            *show = !*show;
        }
        resp.on_hover_text(if *show {
            "隐藏密码"
        } else {
            "显示密码"
        });
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_form_tracks_account_draft_and_global_dirty_independently() {
        let mut saved = Settings::default();
        saved.upsert_account(
            AccountProfile {
                id: "acc-1".into(),
                remark: "宿舍联通".into(),
                student_id: "20240001".into(),
                carrier: "2".into(),
                monthly_quota_gb: 0,
            },
            true,
        );
        saved.upsert_account(
            AccountProfile {
                id: "acc-2".into(),
                remark: "实验室电信".into(),
                student_id: "20240002".into(),
                carrier: "1".into(),
                monthly_quota_gb: 50,
            },
            false,
        );

        let mut form = SettingsForm::from_settings(&saved);
        assert_eq!(form.editing_account_id.as_deref(), Some("acc-1"));
        assert_eq!(form.remark, "宿舍联通");
        assert!(!form.is_dirty(&saved));

        // 仅修改备注名 -> 账号草稿变脏，全局配置不变脏
        form.remark = "南区宿舍联通".into();
        assert!(form.is_account_draft_dirty(&saved));
        assert!(!form.is_global_dirty(&saved));
        assert!(form.is_dirty(&saved));

        // 切换到新建空白草稿 -> 空草稿不算脏
        form.start_new_account();
        assert!(!form.is_account_draft_dirty(&saved));
        assert!(!form.is_dirty(&saved));

        // 修改全局巡检间隔 -> 保留所有多账号配置
        form.check_interval = 60;
        assert!(form.is_global_dirty(&saved));
        let merged = form.to_global_settings(&saved);
        assert_eq!(merged.check_interval, 60);
        assert_eq!(merged.accounts.len(), 2);
        assert_eq!(merged.active_account_id, "acc-1");
    }
}
