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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewAccountMode {
    /// 复用当前活跃学号与已存密码，一键点选新出口。
    QuickExit,
    /// 填写新学号、密码、出口与选填备注的完整双栏表单。
    FullAccount,
}

pub struct SettingsForm {
    pub tab: SettingsTab,
    /// 当前正在编辑的账号 ID；`None` 表示正在创建新账号。
    pub editing_account_id: Option<String>,
    /// 新增账号时的子视图模式（为当前学号加出口 vs 添加其他学号）。
    pub new_account_mode: NewAccountMode,
    /// 进入完整新增表单时自动聚焦学号输入框。
    pub request_student_id_focus: bool,
    pub remark: String,
    pub student_id: String,
    pub password: String,
    pub show_password: bool,
    pub carrier: usize,
    /// 新增运营商账号时是否同时自动生成同密码的「校园网」快捷切换项。
    pub also_create_campus_exit: bool,
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
                acc.carrier.parse::<usize>().unwrap_or(2).min(4),
                acc.monthly_quota_gb,
            ),
            None => (
                None,
                String::new(),
                settings.student_id.clone(),
                settings.carrier.parse::<usize>().unwrap_or(2).min(4),
                settings.monthly_quota_gb,
            ),
        };
        let is_empty = settings.accounts.is_empty();
        Self {
            tab: SettingsTab::Account,
            editing_account_id,
            new_account_mode: NewAccountMode::FullAccount,
            request_student_id_focus: is_empty && settings.student_id.trim().is_empty(),
            remark,
            student_id,
            password: String::new(),
            show_password: false,
            carrier,
            also_create_campus_exit: is_empty,
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
        self.new_account_mode = NewAccountMode::FullAccount;
        self.request_student_id_focus = false;
        self.remark = profile.remark.clone();
        self.student_id = profile.student_id.clone();
        self.password.clear();
        self.show_password = false;
        self.carrier = profile.carrier.parse::<usize>().unwrap_or(2).min(4);
        self.also_create_campus_exit = false;
        self.verify_error = None;
        self.monthly_quota_gb = profile.monthly_quota_gb;
    }

    pub fn start_new_account(&mut self) {
        self.editing_account_id = None;
        self.new_account_mode = NewAccountMode::FullAccount;
        self.request_student_id_focus = true;
        self.remark.clear();
        self.student_id.clear();
        self.password.clear();
        self.show_password = false;
        self.carrier = 2;
        self.also_create_campus_exit = true;
        self.verify_error = None;
        self.monthly_quota_gb = 0;
    }

    /// 从已有账号派生新出口草稿：若当前学号具备可复用凭据且仍有未添加出口，默认进入极简「为当前学号新增出口」视图；否则进入完整双栏新增表单。
    pub fn start_new_from_existing_with_creds(
        &mut self,
        saved: &Settings,
        has_cred: impl Fn(&str) -> bool,
    ) {
        if let Some(base) = saved
            .active_account()
            .filter(|a| !a.student_id.trim().is_empty())
        {
            let sid = base.student_id.trim();
            let has_reusable_pwd = saved.reusable_cred_account_id(sid, has_cred).is_some();
            let has_missing_exit =
                (0..=4).any(|c| !saved.has_carrier_for_student(sid, &c.to_string()));
            if has_reusable_pwd && has_missing_exit {
                self.editing_account_id = None;
                self.new_account_mode = NewAccountMode::QuickExit;
                self.request_student_id_focus = false;
                self.remark.clear();
                self.student_id = sid.to_string();
                self.password.clear();
                self.show_password = false;
                self.carrier = saved.suggest_next_carrier_for(sid);
                self.also_create_campus_exit = false;
                self.verify_error = None;
                self.monthly_quota_gb = 0;
                return;
            }
        }
        self.start_new_account();
    }

    pub fn start_new_from_existing(&mut self, saved: &Settings) {
        self.start_new_from_existing_with_creds(saved, |_| true);
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
            match self.new_account_mode {
                NewAccountMode::QuickExit => {
                    !self.remark.trim().is_empty() || self.monthly_quota_gb != 0
                }
                NewAccountMode::FullAccount => {
                    !self.remark.trim().is_empty()
                        || !self.student_id.trim().is_empty()
                        || !self.password.is_empty()
                        || self.monthly_quota_gb != 0
                }
            }
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
        || (draft_dirty && form.tab != SettingsTab::Account && form.editing_account_id.is_some())
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
    actions: &mut SettingsActions,
) {
    page_header(
        ui,
        p,
        "账号管理",
        "管理多个校园网账号与联网出口，主页顶栏与托盘浮窗可一键切换。",
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
                        .on_hover_text(if saved.accounts.is_empty() {
                            "添加校园网账号"
                        } else {
                            "为当前学号新增出口，或添加其他校园网账号"
                        })
                        .clicked()
                    {
                        form.start_new_from_existing_with_creds(saved, |id| {
                            account_creds.get(id).copied().unwrap_or(false)
                        });
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
                        "尚未保存校园网账号，请在下方填写学号与密码后添加。",
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
                let disp = saved.account_display_name(acc);
                draw_saved_account_row(
                    ui, p, acc, &disp, is_active, is_editing, has_cred, form, actions,
                );
            }
        }
    });

    ui.add_space(10.0);

    // 2. 账号新增 / 编辑表单卡（融合「为当前学号新增出口」极简视图与「左右双栏」对称表单）
    let can_offer_quick_exit = saved.active_account().is_some_and(|acc| {
        let sid = acc.student_id.trim();
        !sid.is_empty()
            && saved
                .reusable_cred_account_id(sid, |id| account_creds.get(id).copied().unwrap_or(false))
                .is_some()
    });

    group_card(ui, p, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;

        if form.editing_account_id.is_none() {
            // ── 新增卡片顶栏：有可复用学号时提供双模式分段切换，否则显示干净单标题 ──
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 26.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    if can_offer_quick_exit {
                        let mut seg_idx = match form.new_account_mode {
                            NewAccountMode::QuickExit => 0,
                            NewAccountMode::FullAccount => 1,
                        };
                        if segmented_control(
                            ui,
                            p,
                            "account_new_mode_seg",
                            &mut seg_idx,
                            &["为当前学号新增出口", "添加其他学号"],
                        ) {
                            if seg_idx == 0 {
                                form.start_new_from_existing_with_creds(saved, |id| {
                                    account_creds.get(id).copied().unwrap_or(false)
                                });
                                form.new_account_mode = NewAccountMode::QuickExit;
                            } else {
                                form.start_new_account();
                            }
                        }
                    } else {
                        ui.label(bold_text("添加校园网账号", 13.0, p.text));
                    }

                    if let Some(active) = saved.active_account() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if draw_compact_action_btn(ui, p, None, "取消", false).clicked() {
                                form.load_account_draft(active);
                            }
                        });
                    }
                },
            );
        } else {
            // ── 编辑已有账号顶栏 ──
            let editing_saved = form
                .editing_account_id
                .as_deref()
                .and_then(|id| saved.find_account(id));
            let draft_dirty = form.is_account_draft_dirty(saved);
            let disp_name = editing_saved
                .map(|acc| saved.account_display_name(acc))
                .unwrap_or_else(|| "账号".to_string());
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 24.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(bold_text(format!("编辑账号 · {disp_name}"), 12.5, p.text));
                    if draft_dirty && !verifying_account {
                        if let Some(acc) = editing_saved {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if draw_compact_action_btn(ui, p, None, "还原", false).clicked()
                                    {
                                        form.load_account_draft(acc);
                                    }
                                },
                            );
                        }
                    }
                },
            );
        }

        // 注意：必须在顶栏交互（分段切换 / 取消 / 还原）之后再读取最新模式与草稿状态，
        // 避免同帧内刚切到「添加其他学号」清空学号后，又误入旧的 QuickExit 分支把原学号写回！
        if form.editing_account_id.is_none() {
            let quick_exit_active =
                can_offer_quick_exit && form.new_account_mode == NewAccountMode::QuickExit;
            if quick_exit_active {
                draw_quick_exit_body(ui, p, form, saved, verifying_account, actions);
            } else {
                let reusable_peer_cred = saved
                    .reusable_cred_account_id(&form.student_id, |id| {
                        account_creds.get(id).copied().unwrap_or(false)
                    })
                    .is_some();
                draw_full_new_account_dual_body(
                    ui,
                    p,
                    form,
                    saved,
                    reusable_peer_cred,
                    verifying_account,
                    actions,
                );
            }
        } else {
            let editing_saved = form
                .editing_account_id
                .as_deref()
                .and_then(|id| saved.find_account(id));
            let editing_has_cred = editing_saved.is_some_and(|acc| {
                account_creds.get(&acc.id).copied().unwrap_or(false)
                    && acc.student_id.trim() == form.student_id.trim()
            });
            let draft_dirty = form.is_account_draft_dirty(saved);
            draw_edit_account_dual_body(
                ui,
                p,
                form,
                editing_has_cred,
                draft_dirty,
                verifying_account,
                actions,
            );
        }
    });
}

fn draw_verify_error_banner(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &SettingsForm,
    verifying_account: bool,
    actions: &mut SettingsActions,
) {
    let Some(err_msg) = form.verify_error.clone() else {
        return;
    };
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
                            "请核对学号、密码与联网出口后重试",
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

fn draw_quick_exit_body(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &mut SettingsForm,
    saved: &Settings,
    verifying_account: bool,
    actions: &mut SettingsActions,
) {
    let base_sid = saved
        .active_account()
        .map(|a| a.student_id.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| form.student_id.trim())
        .to_string();
    if form.student_id.trim() != base_sid && !base_sid.is_empty() {
        form.student_id = base_sid.clone();
    }
    form.also_create_campus_exit = false;

    let all_exits_added = (0..=4).all(|c| saved.has_carrier_for_student(&base_sid, &c.to_string()));
    if !all_exits_added && saved.has_carrier_for_student(&base_sid, &form.carrier.to_string()) {
        form.carrier = saved.suggest_next_carrier_for(&base_sid);
    }

    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 18.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(body_text("选择新出口", 11.5, p.text_secondary));
            ui.label(body_text(
                if all_exits_added {
                    format!("· 学号 {base_sid} 的全部出口均已添加")
                } else {
                    format!("· 复用学号 {base_sid}")
                },
                11.0,
                p.text_muted,
            ));
            if !all_exits_added && form.carrier == 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(body_text(
                        "使用「校园网」出口时请关闭 VPN / 代理",
                        10.5,
                        p.warn,
                    ));
                });
            }
        },
    );

    let labels = ["校园网", "中国电信", "中国联通", "中国移动", "广电网络"];
    let n = labels.len();
    let gap = 7.0_f32;
    let usable = (ui.available_width() - gap * (n as f32 - 1.0)).max(300.0);
    let base_w = (usable / n as f32).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (idx, name) in labels.iter().enumerate() {
            let w = if idx + 1 == n {
                (usable - base_w * (n as f32 - 1.0)).max(base_w)
            } else {
                base_w
            };
            let existed = saved.has_carrier_for_student(&base_sid, &idx.to_string());
            let selected = !existed && form.carrier == idx;
            if existed {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 36.0), Sense::hover());
                ui.painter().rect(
                    r,
                    CornerRadius::same(RADIUS_SM),
                    with_alpha(p.subtle, 140),
                    Stroke::new(1.0_f32, p.divider),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    Pos2::new(r.center().x, r.top() + 12.5),
                    egui::Align2::CENTER_CENTER,
                    *name,
                    egui::FontId::proportional(11.2),
                    p.text_muted,
                );
                ui.painter().text(
                    Pos2::new(r.center().x, r.bottom() - 9.5),
                    egui::Align2::CENTER_CENTER,
                    "已添加",
                    egui::FontId::proportional(9.5),
                    with_alpha(p.text_muted, 175),
                );
                resp.on_hover_text("该学号已存在此出口");
            } else {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 36.0), Sense::click());
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                let (fill, stroke, fg) = if selected {
                    (p.accent_soft, Stroke::new(1.3_f32, p.accent), p.accent)
                } else if resp.hovered() {
                    (p.card_hover, Stroke::new(1.0_f32, p.border_strong), p.text)
                } else {
                    (p.subtle, Stroke::new(1.0_f32, p.border), p.text_secondary)
                };
                ui.painter().rect(
                    r,
                    CornerRadius::same(RADIUS_SM),
                    fill,
                    stroke,
                    StrokeKind::Inside,
                );
                let font = if selected {
                    egui::FontId::new(11.5, crate::ui::theme::bold_family())
                } else {
                    egui::FontId::proportional(11.5)
                };
                let g = ui.painter().layout_no_wrap((*name).to_string(), font, fg);
                if selected {
                    let tot = 12.5 + g.size().x;
                    let sx = r.center().x - tot * 0.5;
                    let ck = Rect::from_center_size(
                        Pos2::new(sx + 4.8, r.center().y),
                        Vec2::splat(10.0),
                    );
                    icons::paint(ui.painter(), ck, Icon::Check, fg);
                    ui.painter().galley(
                        Pos2::new(sx + 12.5, r.center().y - g.size().y * 0.5),
                        g,
                        fg,
                    );
                } else {
                    ui.painter().galley(
                        Pos2::new(
                            r.center().x - g.size().x * 0.5,
                            r.center().y - g.size().y * 0.5,
                        ),
                        g,
                        fg,
                    );
                }
                if resp.clicked() && form.carrier != idx {
                    form.carrier = idx;
                    if idx == 0 {
                        actions.selected_campus_carrier = true;
                    }
                }
            }
        }
    });

    draw_verify_error_banner(ui, p, form, verifying_account, actions);
    row_divider(ui, p);

    let remark_hint = format!("留空显示「{}」", carrier_label(&form.carrier.to_string()));
    let mut no_focus = false;
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 30.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(body_text("备注（选填）", 11.2, p.text_secondary));
            styled_text_box(
                ui,
                p,
                "quick_exit_remark",
                220.0,
                None,
                &mut form.remark,
                &remark_hint,
                false,
                None,
                &mut no_focus,
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let lbl = if verifying_account {
                    "正在核验…"
                } else {
                    "添加出口"
                };
                if ui
                    .add_enabled_ui(!verifying_account && !all_exits_added, |ui| {
                        primary_button(ui, p, Some(Icon::Check), lbl).clicked()
                    })
                    .inner
                {
                    actions.save_profile = true;
                }
            });
        },
    );
}

fn draw_full_new_account_dual_body(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &mut SettingsForm,
    saved: &Settings,
    reusable_peer_cred: bool,
    verifying_account: bool,
    actions: &mut SettingsActions,
) {
    let total_w = ui.available_width();
    let sep_pad = 12.0_f32;
    let left_w = ((total_w - sep_pad * 2.0 - 1.0) * 0.45).floor().max(160.0);
    let right_w = (total_w - left_w - sep_pad * 2.0 - 1.0).max(180.0);
    let body_h = 156.0_f32;

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;

        // 左栏：学号 → 密码 → 备注（三行垂直登录流）
        ui.allocate_ui_with_layout(
            Vec2::new(left_w, body_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(body_text("校园网学号", 11.2, p.text_secondary));
                styled_text_box(
                    ui,
                    p,
                    "new_account_student_id",
                    left_w,
                    Some(Icon::User),
                    &mut form.student_id,
                    "输入校园网学号",
                    false,
                    None,
                    &mut form.request_student_id_focus,
                );

                ui.add_space(5.0);
                ui.label(body_text("登录密码", 11.2, p.text_secondary));
                let pwd_hint = if reusable_peer_cred {
                    "留空直接复用该学号已存密码"
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
                let mut no_focus = false;
                styled_text_box(
                    ui,
                    p,
                    "new_account_password",
                    left_w,
                    Some(Icon::Key),
                    &mut form.password,
                    pwd_hint,
                    mask_password,
                    eye,
                    &mut no_focus,
                );

                ui.add_space(5.0);
                ui.label(body_text("备注名称（选填）", 11.2, p.text_secondary));
                styled_text_box(
                    ui,
                    p,
                    "new_account_remark",
                    left_w,
                    None,
                    &mut form.remark,
                    "如：宿舍联通（留空按出口显示）",
                    false,
                    None,
                    &mut no_focus,
                );
            },
        );

        // 中间 1px 竖向分隔线
        let (sep_r, _) =
            ui.allocate_exact_size(Vec2::new(sep_pad * 2.0 + 1.0, body_h), Sense::hover());
        ui.painter().line_segment(
            [
                Pos2::new(sep_r.center().x, sep_r.top() + 2.0),
                Pos2::new(sep_r.center().x, sep_r.bottom() - 2.0),
            ],
            Stroke::new(1.0_f32, p.divider),
        );

        // 右栏：单行 5 等分对称出口胶囊 + 固定高度联动说明卡 + 右下角主操作按钮
        ui.allocate_ui_with_layout(
            Vec2::new(right_w, body_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(body_text("运营商出口", 11.2, p.text_secondary));
                if draw_symmetric_carrier_row(ui, p, right_w, &mut form.carrier)
                    && form.carrier == 0
                {
                    actions.selected_campus_carrier = true;
                }

                ui.add_space(8.0);
                let campus_already_saved = !form.student_id.trim().is_empty()
                    && saved.has_carrier_for_student(&form.student_id, "0");
                draw_exit_companion_or_vpn_card(
                    ui,
                    p,
                    right_w,
                    form.carrier == 0,
                    campus_already_saved,
                    &mut form.also_create_campus_exit,
                );

                let used = ui.cursor().top();
                let target_btn_top = sep_r.bottom() - 30.0;
                if target_btn_top > used {
                    ui.add_space(target_btn_top - used);
                }
                ui.allocate_ui_with_layout(
                    Vec2::new(right_w, 30.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let btn_label = if verifying_account {
                            "正在核验账号…"
                        } else {
                            "验证并添加账号"
                        };
                        if ui
                            .add_enabled_ui(!verifying_account, |ui| {
                                primary_button(ui, p, Some(Icon::Check), btn_label).clicked()
                            })
                            .inner
                        {
                            actions.save_profile = true;
                        }
                    },
                );
            },
        );
    });

    draw_verify_error_banner(ui, p, form, verifying_account, actions);
}

fn draw_edit_account_dual_body(
    ui: &mut egui::Ui,
    p: &Palette,
    form: &mut SettingsForm,
    editing_has_cred: bool,
    draft_dirty: bool,
    verifying_account: bool,
    actions: &mut SettingsActions,
) {
    let total_w = ui.available_width();
    let sep_pad = 12.0_f32;
    let left_w = ((total_w - sep_pad * 2.0 - 1.0) * 0.45).floor().max(160.0);
    let right_w = (total_w - left_w - sep_pad * 2.0 - 1.0).max(180.0);
    let cols_h = 104.0_f32;
    let mut no_focus = false;

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;

        ui.allocate_ui_with_layout(
            Vec2::new(left_w, cols_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(body_text("校园网学号", 11.2, p.text_secondary));
                styled_text_box(
                    ui,
                    p,
                    "edit_account_student_id",
                    left_w,
                    Some(Icon::User),
                    &mut form.student_id,
                    "输入校园网学号",
                    false,
                    None,
                    &mut no_focus,
                );

                ui.add_space(6.0);
                ui.label(body_text("登录密码", 11.2, p.text_secondary));
                let pwd_hint = if editing_has_cred {
                    "留空不修改"
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
                    "edit_account_password",
                    left_w,
                    Some(Icon::Key),
                    &mut form.password,
                    pwd_hint,
                    mask_password,
                    eye,
                    &mut no_focus,
                );
            },
        );

        let (sep_r, _) =
            ui.allocate_exact_size(Vec2::new(sep_pad * 2.0 + 1.0, cols_h), Sense::hover());
        ui.painter().line_segment(
            [
                Pos2::new(sep_r.center().x, sep_r.top() + 2.0),
                Pos2::new(sep_r.center().x, sep_r.bottom() - 2.0),
            ],
            Stroke::new(1.0_f32, p.divider),
        );

        ui.allocate_ui_with_layout(
            Vec2::new(right_w, cols_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(body_text("运营商出口", 11.2, p.text_secondary));
                if draw_symmetric_carrier_row(ui, p, right_w, &mut form.carrier)
                    && form.carrier == 0
                {
                    actions.selected_campus_carrier = true;
                }

                ui.add_space(6.0);
                ui.label(body_text("备注名称（选填）", 11.2, p.text_secondary));
                styled_text_box(
                    ui,
                    p,
                    "edit_account_remark",
                    right_w,
                    None,
                    &mut form.remark,
                    "如：宿舍联通（留空按出口显示）",
                    false,
                    None,
                    &mut no_focus,
                );
            },
        );
    });

    draw_verify_error_banner(ui, p, form, verifying_account, actions);
    row_divider(ui, p);

    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 30.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            if form.carrier == 0 {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (ir, _) = ui.allocate_exact_size(Vec2::splat(13.5), Sense::hover());
                icons::paint(ui.painter(), ir, Icon::Shield, p.warn);
                ui.label(body_text(
                    "使用「校园网」出口时请关闭本机 VPN / 代理软件",
                    10.8,
                    p.warn,
                ));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let btn_label = if verifying_account {
                    "正在核验账号…"
                } else {
                    "保存修改"
                };
                let clicked = ui
                    .add_enabled_ui(!verifying_account, |ui| {
                        if draft_dirty {
                            primary_button(ui, p, Some(Icon::Check), btn_label).clicked()
                        } else {
                            secondary_button(ui, p, Some(Icon::Check), btn_label).clicked()
                        }
                    })
                    .inner;
                if clicked {
                    actions.save_profile = true;
                }
            });
        },
    );
}

fn draw_exit_companion_or_vpn_card(
    ui: &mut egui::Ui,
    p: &Palette,
    width: f32,
    campus_selected: bool,
    campus_already_saved: bool,
    also_create_campus: &mut bool,
) {
    if campus_selected {
        let (box_r, _) = ui.allocate_exact_size(Vec2::new(width, 48.0), Sense::hover());
        ui.painter().rect(
            box_r,
            CornerRadius::same(RADIUS_SM),
            p.warn_soft,
            Stroke::new(1.0_f32, p.warn_border),
            StrokeKind::Inside,
        );
        let ic_r = Rect::from_center_size(
            Pos2::new(box_r.left() + 17.0, box_r.top() + 16.5),
            Vec2::splat(13.5),
        );
        icons::paint(ui.painter(), ic_r, Icon::Shield, p.warn);
        ui.painter().text(
            Pos2::new(ic_r.right() + 7.0, box_r.top() + 16.5),
            egui::Align2::LEFT_CENTER,
            "使用「校园网」出口时请关闭 VPN / 代理",
            egui::FontId::proportional(11.0),
            p.warn,
        );
        ui.painter().text(
            Pos2::new(ic_r.right() + 7.0, box_r.bottom() - 13.0),
            egui::Align2::LEFT_CENTER,
            "内网出口下开启代理工具易触发学校防共享封禁",
            egui::FontId::proportional(10.0),
            p.text_secondary,
        );
    } else if campus_already_saved {
        let (box_r, _) = ui.allocate_exact_size(Vec2::new(width, 48.0), Sense::hover());
        ui.painter().rect(
            box_r,
            CornerRadius::same(RADIUS_SM),
            p.subtle,
            Stroke::new(1.0_f32, p.divider),
            StrokeKind::Inside,
        );
        let ic_r = Rect::from_center_size(
            Pos2::new(box_r.left() + 17.0, box_r.top() + 16.5),
            Vec2::splat(13.0),
        );
        icons::paint(ui.painter(), ic_r, Icon::Check, p.ok);
        ui.painter().text(
            Pos2::new(ic_r.right() + 7.0, box_r.top() + 16.5),
            egui::Align2::LEFT_CENTER,
            "该学号已存在「校园网」切换项",
            egui::FontId::proportional(11.0),
            p.text_secondary,
        );
        ui.painter().text(
            Pos2::new(ic_r.right() + 7.0, box_r.bottom() - 13.0),
            egui::Align2::LEFT_CENTER,
            "保存后将自动同步并共享最新密码",
            egui::FontId::proportional(10.0),
            p.text_muted,
        );
    } else {
        let (box_r, resp) = ui.allocate_exact_size(Vec2::new(width, 48.0), Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            *also_create_campus = !*also_create_campus;
        }
        let border = if resp.hovered() {
            p.border_strong
        } else {
            p.divider
        };
        ui.painter().rect(
            box_r,
            CornerRadius::same(RADIUS_SM),
            p.subtle,
            Stroke::new(1.0_f32, border),
            StrokeKind::Inside,
        );
        let ck_r = Rect::from_center_size(
            Pos2::new(box_r.left() + 17.0, box_r.top() + 16.5),
            Vec2::splat(14.5),
        );
        let (ck_fill, ck_stroke) = if *also_create_campus {
            (p.accent, Stroke::NONE)
        } else if resp.hovered() {
            (p.card_hover, Stroke::new(1.0_f32, p.accent))
        } else {
            (p.input_bg, Stroke::new(1.0_f32, p.border_strong))
        };
        ui.painter().rect(
            ck_r,
            CornerRadius::same(4),
            ck_fill,
            ck_stroke,
            StrokeKind::Inside,
        );
        if *also_create_campus {
            icons::paint(ui.painter(), ck_r.shrink(2.2), Icon::Check, p.on_accent);
        }
        let title_c = if *also_create_campus || resp.hovered() {
            p.text
        } else {
            p.text_secondary
        };
        ui.painter().text(
            Pos2::new(ck_r.right() + 7.0, box_r.top() + 16.5),
            egui::Align2::LEFT_CENTER,
            "同时生成「校园网」快捷切换项（共享密码）",
            egui::FontId::proportional(11.0),
            title_c,
        );
        ui.painter().text(
            Pos2::new(ck_r.right() + 7.0, box_r.bottom() - 13.0),
            egui::Align2::LEFT_CENTER,
            "主页顶栏与托盘菜单可一键在宽带与校园网间互切",
            egui::FontId::proportional(10.0),
            p.text_muted,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_saved_account_row(
    ui: &mut egui::Ui,
    p: &Palette,
    acc: &AccountProfile,
    display_name: &str,
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
    let right_reserved = if !has_cred {
        if is_active {
            104.0_f32
        } else {
            156.0_f32
        }
    } else if is_active {
        44.0_f32
    } else {
        98.0_f32
    };
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

    let title_font = egui::FontId::new(12.5, crate::ui::theme::bold_family());
    let title_galley = ui
        .painter()
        .layout_no_wrap(display_name.to_string(), title_font, p.text);
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
    let carrier_lbl = carrier_label(&acc.carrier);
    let subtitle = if !acc.remark.trim().is_empty() {
        format!(
            "{} · {}{}",
            acc.masked_student_id(),
            carrier_lbl,
            quota_part
        )
    } else if display_name == carrier_lbl {
        format!("{}{}", acc.masked_student_id(), quota_part)
    } else {
        format!("{}{}", carrier_lbl, quota_part)
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

    if !has_cred {
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

/// 右栏单行 5 等分对称出口选择器（与师大门户 `R3 = "0".."4"` 严格一一对应）。
fn draw_symmetric_carrier_row(
    ui: &mut egui::Ui,
    p: &Palette,
    total_w: f32,
    carrier: &mut usize,
) -> bool {
    let mut changed = false;
    let items: &[(&str, &str)] = &[
        ("校园网", "校园网（内网出口）"),
        ("电信", "中国电信"),
        ("联通", "中国联通"),
        ("移动", "中国移动"),
        ("广电", "广电网络"),
    ];
    let n = items.len();
    let gap = 5.0_f32;
    let usable_w = (total_w - gap * (n.saturating_sub(1) as f32)).max(220.0);
    let base_w = (usable_w / n as f32).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (idx, (short_lbl, full_tip)) in items.iter().enumerate() {
            let active = *carrier == idx;
            let btn_w = if idx + 1 == n {
                (usable_w - base_w * (n.saturating_sub(1) as f32)).max(base_w)
            } else {
                base_w
            };
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(btn_w, 30.0), Sense::click());
            let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
            let (fill, stroke, fg) = if active {
                (p.accent_soft, Stroke::new(1.3_f32, p.accent), p.accent)
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
                egui::FontId::new(11.5, crate::ui::theme::bold_family())
            } else {
                egui::FontId::proportional(11.5)
            };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                *short_lbl,
                font,
                fg,
            );
            if resp.on_hover_text(*full_tip).clicked() && *carrier != idx {
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
                        UpdateStatus::Checking => {
                            badge(ui, p.accent, p.accent_soft, p.accent_border, "检查中…");
                        }
                        UpdateStatus::Applying => {
                            badge(ui, p.ok, p.ok_soft, p.ok_border, "更新中…");
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

#[allow(clippy::too_many_arguments)]
fn styled_text_box(
    ui: &mut egui::Ui,
    p: &Palette,
    id_salt: &str,
    width: f32,
    leading_icon: Option<Icon>,
    value: &mut String,
    hint: &str,
    password: bool,
    eye_toggle: Option<&mut bool>,
    request_focus: &mut bool,
) -> egui::Response {
    let (box_rect, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
    let glow_idx = ui.painter().add(egui::Shape::Noop);
    let bg_idx = ui.painter().add(egui::Shape::Noop);

    let icon_pad = if leading_icon.is_some() { 19.0 } else { 0.0 };
    let inner_rect = Rect::from_min_max(
        Pos2::new(box_rect.left() + 9.0 + icon_pad, box_rect.top() + 4.0),
        Pos2::new(box_rect.right() - 9.0, box_rect.bottom() - 4.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 4.0;
    let edit_w = if eye_toggle.is_some() {
        (inner_rect.width() - 24.0).max(50.0)
    } else {
        inner_rect.width().max(50.0)
    };
    let edit_resp = child.add_sized(
        [edit_w, 20.0],
        egui::TextEdit::singleline(value)
            .id_salt(id_salt)
            .password(password)
            .hint_text(egui::RichText::new(hint).color(p.text_muted))
            .vertical_align(egui::Align::Center)
            .margin(egui::Margin::ZERO)
            .frame(false),
    );
    if *request_focus {
        edit_resp.request_focus();
        *request_focus = false;
    }

    let focused = edit_resp.has_focus();
    let hovered = ui.rect_contains_pointer(box_rect);
    let border_stroke = if focused {
        Stroke::new(1.3_f32, p.accent)
    } else if hovered {
        Stroke::new(1.0_f32, p.border_strong)
    } else {
        Stroke::new(1.0_f32, p.border)
    };
    ui.painter().set(
        bg_idx,
        egui::epaint::RectShape::new(
            box_rect,
            CornerRadius::same(RADIUS_SM),
            p.input_bg,
            border_stroke,
            StrokeKind::Inside,
        ),
    );
    if focused {
        ui.painter().set(
            glow_idx,
            egui::epaint::RectShape::new(
                box_rect.expand(1.5),
                CornerRadius::same(RADIUS_SM + 1),
                egui::Color32::TRANSPARENT,
                Stroke::new(2.0_f32, with_alpha(p.accent, 30)),
                StrokeKind::Outside,
            ),
        );
    }

    if let Some(ic) = leading_icon {
        let ic_col = if focused { p.accent } else { p.text_muted };
        let ir = Rect::from_center_size(
            Pos2::new(box_rect.left() + 15.0, box_rect.center().y),
            Vec2::splat(13.0),
        );
        icons::paint(ui.painter(), ir, ic, ic_col);
    }

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

    edit_resp
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

    #[test]
    fn quick_exit_mode_reuses_student_id_without_false_dirty_and_falls_back_when_all_exits_exist() {
        let mut saved = Settings::default();
        saved.upsert_account(
            AccountProfile {
                id: "acc-unicom".into(),
                remark: "联通宽带".into(),
                student_id: "2024010615".into(),
                carrier: "2".into(),
                monthly_quota_gb: 0,
            },
            true,
        );

        let mut form = SettingsForm::from_settings(&saved);
        form.start_new_from_existing_with_creds(&saved, |_| true);
        assert_eq!(form.editing_account_id, None);
        assert_eq!(form.new_account_mode, NewAccountMode::QuickExit);
        assert_eq!(form.student_id, "2024010615");
        assert_eq!(form.carrier, 0);
        assert!(!form.also_create_campus_exit);
        assert!(!form.is_account_draft_dirty(&saved));

        form.remark = "宿舍校园网".into();
        assert!(form.is_account_draft_dirty(&saved));

        // 当同一学号已配齐全部 5 个出口（0..=4）时，自动回退到完整新增表单
        for c in ["0", "1", "3", "4"] {
            saved.upsert_account(
                AccountProfile {
                    id: format!("acc-{c}"),
                    remark: String::new(),
                    student_id: "2024010615".into(),
                    carrier: c.into(),
                    monthly_quota_gb: 0,
                },
                false,
            );
        }
        form.start_new_from_existing_with_creds(&saved, |_| true);
        assert_eq!(form.new_account_mode, NewAccountMode::FullAccount);
        assert!(form.student_id.is_empty());
        assert!(form.also_create_campus_exit);
        assert!(form.request_student_id_focus);
        assert!(!form.is_account_draft_dirty(&saved));
    }

    #[test]
    fn switching_to_add_other_student_id_in_ui_never_retains_existing_student_id() {
        fn find_in_shape(shape: &egui::Shape, needle: &str) -> Option<Pos2> {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == needle => {
                    Some(t.pos + t.galley.size() * 0.5)
                }
                egui::Shape::Vec(items) => items.iter().find_map(|s| find_in_shape(s, needle)),
                _ => None,
            }
        }
        fn find_label_center(shapes: &[egui::epaint::ClippedShape], needle: &str) -> Option<Pos2> {
            shapes.iter().find_map(|c| find_in_shape(&c.shape, needle))
        }

        let mut saved = Settings::default();
        saved.upsert_account(
            AccountProfile {
                id: "acc-unicom".into(),
                remark: "联通宽带".into(),
                student_id: "2024010615".into(),
                carrier: "2".into(),
                monthly_quota_gb: 0,
            },
            true,
        );
        let mut creds = HashMap::new();
        creds.insert("acc-unicom".to_string(), true);

        let mut form = SettingsForm::from_settings(&saved);
        form.start_new_from_existing_with_creds(&saved, |id| {
            creds.get(id).copied().unwrap_or(false)
        });
        assert_eq!(form.new_account_mode, NewAccountMode::QuickExit);
        assert_eq!(form.student_id, "2024010615");

        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        let prop = fonts.families[&egui::FontFamily::Proportional].clone();
        fonts.families.insert(crate::ui::theme::bold_family(), prop);
        ctx.set_fonts(fonts);
        let palette = Palette::of(ThemeMode::Dark);

        // 第 1 帧：渲染 QuickExit 视图并从绘制图元中定位「添加其他学号」按钮中心
        let out1 = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut actions = SettingsActions::default();
                draw_account_page(ui, &palette, &mut form, &saved, &creds, false, &mut actions);
            });
        });
        let click_pos =
            find_label_center(&out1.shapes, "添加其他学号").expect("应找到「添加其他学号」分段项");

        // 第 2 帧 + 第 3 帧：真实模拟鼠标按下并抬起点击「添加其他学号」
        for pressed in [true, false] {
            let input = egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(click_pos),
                    egui::Event::PointerButton {
                        pos: click_pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut actions = SettingsActions::default();
                    draw_account_page(ui, &palette, &mut form, &saved, &creds, false, &mut actions);
                });
            });
        }

        // 切到「添加其他学号」后必须清空原有学号，绝不能被同帧残留分支写回
        assert_eq!(form.new_account_mode, NewAccountMode::FullAccount);
        assert_eq!(form.student_id, "");
        assert!(form.also_create_campus_exit);
    }
}
