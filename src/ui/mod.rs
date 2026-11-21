//! 主窗口双视图架构（极简薄顶栏 + 主页 / 成熟版 Windows 11 Fluent 设置中心）：
//! - 顶部：36px 无边框自定义标题栏（主页右侧提供「设置」入口，设置页左上角提供「← 返回主页」）
//! - 主页：上窄状态条（含当前账号入口）→ 中 62:38 非对称图文双卡（连接稳定性 + 双模式环形流量）→ 下通栏运行日志
//! - 设置页：左侧分组目录（账号管理 / 监控与自启 / 通知与数据）+ 右侧分组卡片 + 底部常驻保存栏

mod charts;
mod icons;
mod logs;
mod settings;
mod theme;
mod widgets;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use chrono::Local;
use eframe::egui::{
    self, CornerRadius, CursorIcon, Pos2, Rect, ResizeDirection, Sense, Stroke, StrokeKind, Vec2,
    ViewportCommand,
};

use crate::config::SettingsStore;
use crate::credentials;
use crate::ipc::{IpcClient, SnapshotEnvelope, UiToDaemon};
use crate::metrics_store::{TelemetryPoint, TimeRange};
use crate::models::{MonitorSnapshot, MonitorState, Settings, TrafficSummary};
use crate::startup;

use charts::TrafficPeriod;
use icons::Icon;
use logs as logs_page;
use settings as settings_page;
use theme::{
    body_text, bold_text, carrier_label, clamp_ui_zoom, format_check_ms, format_check_time,
    format_uptime, status_title, with_alpha, Palette, ThemeMode, UiPrefs,
};
pub use theme::{load_ui_prefs, MIN_WINDOW_SIZE};
use widgets::{
    accent_card, badge, horizontal_splitter, icon_button, primary_button, pulsing_dot,
    secondary_button,
};

const CREDENTIAL_TARGET: &str = "GXNUCampusLogin/v4";
const MAX_LOG_LINES: usize = 4000;
const MAX_TELEMETRY_POINTS: usize = 5000;
const TITLEBAR_HEIGHT: f32 = 36.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppView {
    Home,
    Settings,
}

pub struct GuardDeps {
    pub store: SettingsStore,
    pub settings: Settings,
    pub ipc: IpcClient,
    pub snapshots: Receiver<SnapshotEnvelope>,
    pub logs: Receiver<String>,
    pub log_path: PathBuf,
    pub initial_logs: Vec<String>,
    pub initial_telemetry: Vec<TelemetryPoint>,
    pub initial_traffic: TrafficSummary,
}

struct PendingCommand {
    command: UiToDaemon,
    deadline: Instant,
    startup_error: Option<String>,
}

impl PendingCommand {
    fn feedback(&self, result: Result<(), String>) -> Option<(String, bool)> {
        let (mut message, mut error) = match (&self.command, result) {
            (UiToDaemon::ReloadSettings { .. }, Err(e)) => {
                (format!("设置已写入磁盘，守护进程未确认生效：{e}"), true)
            }
            (_, Err(e)) => (format!("守护命令未获确认：{e}"), true),
            (UiToDaemon::ReloadSettings { start: true, .. }, Ok(())) => {
                ("设置已生效，守护已启动".into(), false)
            }
            (UiToDaemon::ReloadSettings { start: false, .. }, Ok(())) => {
                ("设置已保存并生效".into(), false)
            }
            (UiToDaemon::SwitchAccount { .. }, Ok(())) => ("正在切换校园网账号…".into(), false),
            (UiToDaemon::Start, Ok(())) => ("守护已启动".into(), false),
            (UiToDaemon::Pause, Ok(())) => ("守护已暂停".into(), false),
            (UiToDaemon::TestToast { .. }, Ok(())) => ("已处理系统测试通知请求".into(), false),
            _ => return None,
        };
        if let Some(err) = &self.startup_error {
            message.push_str(&format!("；修改开机自启失败：{err}"));
            error = true;
        }
        Some((message, error))
    }
}

fn needs_account_setup(configured: &str, stored: Option<&str>) -> bool {
    !stored.is_some_and(|id| !id.is_empty() && (configured.is_empty() || id == configured))
}

pub struct GuardApp {
    store: SettingsStore,
    settings: Settings,
    ipc: IpcClient,
    snapshots: Receiver<SnapshotEnvelope>,
    latest: Option<MonitorSnapshot>,
    logs: Receiver<String>,
    log_lines: Vec<String>,
    log_path: PathBuf,
    ui_prefs: UiPrefs,
    window_size_dirty_since: Option<Instant>,
    view: AppView,
    form: settings_page::SettingsForm,
    credential_account: Option<String>,
    logs_state: logs_page::LogsState,
    telemetry: Vec<TelemetryPoint>,
    traffic: TrafficSummary,
    time_range: TimeRange,
    traffic_period: TrafficPeriod,
    last_check_seq: u64,
    last_check_unix: Option<i64>,
    online_since_unix: Option<i64>,
    toast: Option<(String, bool, Instant)>,
    pending_manual_check: bool,
    pending: HashMap<u64, PendingCommand>,
    needs_setup: bool,
    account_alert_opened: bool,
    trimmed_while_idle: bool,
    brand_icon: egui::TextureHandle,
}

impl GuardApp {
    pub fn new(cc: &eframe::CreationContext<'_>, deps: GuardDeps) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let ui_prefs = theme::load_ui_prefs(&deps.store.app_dir);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        cc.egui_ctx.set_zoom_factor(ui_prefs.zoom);
        let palette = Palette::of(ui_prefs.theme);
        theme::apply_theme(&cc.egui_ctx, &palette);

        let credential_account = credentials::load(CREDENTIAL_TARGET)
            .ok()
            .flatten()
            .filter(|stored| !stored.password.is_empty())
            .map(|stored| stored.username);
        let mut form = settings_page::SettingsForm::from_settings(&deps.settings);
        if form.student_id.is_empty() {
            form.student_id = credential_account.clone().unwrap_or_default();
        }
        // 不把已保存密码回填到输入框，仅在用户输入新密码时才允许预览并保存。
        form.password.clear();

        let env_tab = std::env::var("GXNU_SETTINGS_TAB").ok();
        if let Some(tab) = env_tab.as_deref() {
            form.tab = match tab {
                "monitor" => settings_page::SettingsTab::Monitor,
                "display" => settings_page::SettingsTab::Display,
                "notify" => settings_page::SettingsTab::Notify,
                _ => settings_page::SettingsTab::Account,
            };
        }

        let mut log_lines = deps.initial_logs;
        if log_lines.len() > MAX_LOG_LINES {
            log_lines = log_lines.split_off(log_lines.len() - MAX_LOG_LINES);
        }

        let needs_setup =
            needs_account_setup(&deps.settings.student_id, credential_account.as_deref());
        let env_open_settings = std::env::var("GXNU_VIEW")
            .map(|v| v.eq_ignore_ascii_case("settings"))
            .unwrap_or(false)
            || env_tab.is_some();
        let view = if needs_setup || env_open_settings {
            if needs_setup {
                form.tab = settings_page::SettingsTab::Account;
            }
            AppView::Settings
        } else {
            AppView::Home
        };

        // 上传与 .exe / 任务栏 / 托盘同源的 64×64 柔和圆角矩形品牌图标用于标题栏左上角。
        let icon_rgba = crate::app_icon::render_icon_rgba(None, 64);
        let color_img = egui::ColorImage::from_rgba_unmultiplied([64, 64], &icon_rgba);
        let brand_icon = cc.egui_ctx.load_texture(
            "brand_titlebar_icon",
            color_img,
            egui::TextureOptions::LINEAR,
        );

        Self {
            store: deps.store,
            settings: deps.settings,
            ipc: deps.ipc,
            snapshots: deps.snapshots,
            latest: None,
            logs: deps.logs,
            log_lines,
            log_path: deps.log_path,
            ui_prefs,
            window_size_dirty_since: None,
            view,
            form,
            credential_account,
            logs_state: logs_page::LogsState::default(),
            telemetry: deps.initial_telemetry,
            traffic: deps.initial_traffic,
            time_range: TimeRange::H24,
            traffic_period: TrafficPeriod::Today,
            last_check_seq: 0,
            last_check_unix: None,
            online_since_unix: None,
            toast: None,
            pending_manual_check: false,
            pending: HashMap::new(),
            needs_setup,
            account_alert_opened: false,
            trimmed_while_idle: false,
            brand_icon,
        }
    }

    fn show_toast(&mut self, message: impl Into<String>, is_error: bool) {
        self.toast = Some((message.into(), is_error, Instant::now()));
    }

    fn state(&self) -> Option<MonitorState> {
        self.latest.as_ref().map(|snapshot| snapshot.state)
    }

    fn monitoring(&self) -> bool {
        self.latest
            .as_ref()
            .is_some_and(|snapshot| snapshot.monitoring_enabled)
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.ui_prefs.theme = self.ui_prefs.theme.toggled();
        let palette = Palette::of(self.ui_prefs.theme);
        theme::apply_theme(ctx, &palette);
        theme::save_ui_prefs(&self.store.app_dir, &self.ui_prefs);
    }

    fn apply_zoom(&mut self, ctx: &egui::Context, target: f32, notify: bool) {
        let next = clamp_ui_zoom(target);
        if (next - self.ui_prefs.zoom).abs() < 0.001 {
            return;
        }
        self.ui_prefs.zoom = next;
        ctx.set_zoom_factor(next);
        theme::save_ui_prefs(&self.store.app_dir, &self.ui_prefs);
        if notify {
            self.show_toast(
                format!("界面缩放：{}%", (next * 100.0).round() as i32),
                false,
            );
        }
    }

    fn handle_zoom_shortcuts(&mut self, ctx: &egui::Context) {
        let mut step: Option<f32> = None;
        let mut reset = false;
        ctx.input(|i| {
            let ctrl = i.modifiers.ctrl || i.modifiers.command;
            if !ctrl {
                return;
            }
            if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                step = Some(0.05);
            } else if i.key_pressed(egui::Key::Minus) {
                step = Some(-0.05);
            } else if i.key_pressed(egui::Key::Num0) {
                reset = true;
            } else {
                for ev in &i.events {
                    if let egui::Event::MouseWheel {
                        delta, modifiers, ..
                    } = ev
                    {
                        if (modifiers.ctrl || modifiers.command) && delta.y.abs() > 0.01 {
                            step = Some(if delta.y > 0.0 { 0.05 } else { -0.05 });
                            break;
                        }
                    }
                }
            }
        });
        if reset {
            self.apply_zoom(ctx, 1.0, true);
        } else if let Some(delta) = step {
            self.apply_zoom(ctx, self.ui_prefs.zoom + delta, true);
        }
    }

    fn track_window_size(&mut self, ctx: &egui::Context) {
        let (maximized, minimized, inner_rect) = ctx.input(|i| {
            let vp = i.viewport();
            (
                vp.maximized.unwrap_or(false),
                vp.minimized.unwrap_or(false),
                vp.inner_rect,
            )
        });
        if !maximized && !minimized {
            if let Some(rect) = inner_rect {
                let zoom = ctx.zoom_factor().max(0.5);
                let os_w = (rect.width() * zoom).round();
                let os_h = (rect.height() * zoom).round();
                if os_w >= MIN_WINDOW_SIZE[0] && os_h >= MIN_WINDOW_SIZE[1] {
                    let prev = self.ui_prefs.effective_window_size();
                    if (os_w - prev[0]).abs() >= 4.0 || (os_h - prev[1]).abs() >= 4.0 {
                        self.ui_prefs.window_size = Some([os_w, os_h]);
                        self.window_size_dirty_since = Some(Instant::now());
                    }
                }
            }
        }
        if let Some(since) = self.window_size_dirty_since {
            if since.elapsed() >= Duration::from_millis(600) {
                self.window_size_dirty_since = None;
                theme::save_ui_prefs(&self.store.app_dir, &self.ui_prefs);
            }
        }
    }

    fn request(&mut self, command: UiToDaemon, startup_error: Option<String>) {
        let result = self.ipc.send(command.clone());
        let pending = PendingCommand {
            command,
            deadline: Instant::now() + crate::ipc::COMMAND_TIMEOUT,
            startup_error,
        };
        match result {
            Ok(id) => {
                self.pending.insert(id, pending);
            }
            Err(err) => self.finish_command(pending, Err(err)),
        }
    }

    fn finish_command(&mut self, pending: PendingCommand, result: Result<(), String>) {
        if matches!(
            pending.command,
            UiToDaemon::ReloadSettings { start: true, .. }
        ) && result.is_ok()
        {
            self.needs_setup = false;
            self.view = AppView::Home;
        }
        if matches!(pending.command, UiToDaemon::ForceCheck) && result.is_err() {
            self.pending_manual_check = false;
        }
        if let Some((message, error)) = pending.feedback(result) {
            self.show_toast(message, error);
        }
    }

    fn save(&mut self) {
        let settings = self.form.to_settings();
        let start = self.needs_setup;
        let result = credentials::load(CREDENTIAL_TARGET).and_then(|stored| {
            if start
                && (settings.student_id.is_empty()
                    || (self.form.password.is_empty()
                        && !stored
                            .as_ref()
                            .is_some_and(|c| c.matches_account(&settings.student_id))))
            {
                return Err("请输入当前学号和密码".into());
            }
            self.store.save_account(
                &settings,
                &self.settings.student_id,
                &self.form.password,
                stored.as_ref(),
                |value| match value {
                    Some(cred) => {
                        credentials::save(CREDENTIAL_TARGET, &cred.username, &cred.password)
                    }
                    None => credentials::delete(CREDENTIAL_TARGET),
                },
            )
        });
        let password_changed = match result {
            Ok(changed) => changed,
            Err(err) => {
                self.show_toast(format!("保存设置失败：{err}"), true);
                return;
            }
        };
        if !self.form.password.is_empty() {
            self.form.password.clear();
            self.form.show_password = false;
            self.credential_account = Some(settings.student_id.clone());
        }
        let startup_error = if settings.auto_boot != self.settings.auto_boot {
            startup::set_enabled(settings.auto_boot).err()
        } else {
            None
        };
        let command = UiToDaemon::ReloadSettings {
            start,
            password_changed,
            expected: settings.clone(),
        };
        self.settings = settings;
        if startup_error.is_some() {
            self.settings.auto_boot = startup::is_enabled();
            self.form.auto_boot = self.settings.auto_boot;
        }
        self.show_toast("设置已写入磁盘，等待守护进程确认", false);
        self.request(command, startup_error);
    }

    fn drain_channels(&mut self) {
        while let Ok((id, result)) = self.ipc.replies.try_recv() {
            if let Some(pending) = self.pending.remove(&id) {
                self.finish_command(pending, result);
            }
        }
        let lost = self.ipc.connection_lost.load(Ordering::Acquire);
        let now = Instant::now();
        let expired: Vec<_> = self
            .pending
            .extract_if(|_, p| lost || p.deadline <= now)
            .map(|(_, p)| p)
            .collect();
        for pending in expired {
            let message = if lost {
                "连接已断开，请重新打开主界面"
            } else {
                "等待回执超时"
            };
            self.finish_command(pending, Err(message.into()));
        }
        while let Ok(env) = self.snapshots.try_recv() {
            let snapshot = env.snapshot;
            self.online_since_unix = env.online_since_unix;
            self.last_check_unix = env.last_check_unix;
            self.traffic = env.traffic;

            if snapshot.metrics.check_seq > self.last_check_seq {
                self.last_check_seq = snapshot.metrics.check_seq;
                if self.pending_manual_check && snapshot.state != MonitorState::Checking {
                    self.pending_manual_check = false;
                    if snapshot.metrics.last_point.is_none() && snapshot.reason.is_none() {
                        self.show_toast(&snapshot.message, false);
                    } else if snapshot.reason == Some(crate::models::DisconnectReason::Online) {
                        self.show_toast("检测完成：校园网正常", false);
                    } else {
                        let reason = snapshot.reason.map(|r| r.label()).unwrap_or("网络异常");
                        self.show_toast(format!("检测异常：{reason}"), true);
                    }
                }
            }
            if let Some(point) = &snapshot.metrics.last_point {
                merge_sample(&mut self.telemetry, point);
            }
            if prompt_account(&snapshot, &mut self.account_alert_opened) {
                self.view = AppView::Settings;
                self.form.tab = settings_page::SettingsTab::Account;
            }
            self.latest = Some(snapshot);
        }

        let mut appended = false;
        while let Ok(line) = self.logs.try_recv() {
            self.log_lines.push(line);
            appended = true;
        }
        if appended && self.log_lines.len() > MAX_LOG_LINES {
            let drop_count = self.log_lines.len() - MAX_LOG_LINES;
            self.log_lines.drain(..drop_count);
        }
    }

    fn draw_titlebar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, p: &Palette) {
        let bar_rect = ui.max_rect();
        ui.painter()
            .rect_filled(bar_rect, CornerRadius::ZERO, p.titlebar);
        ui.painter().line_segment(
            [bar_rect.left_bottom(), bar_rect.right_bottom()],
            Stroke::new(1.0_f32, p.border),
        );

        let left_skip = if self.view == AppView::Settings {
            110.0
        } else {
            0.0
        };
        let controls_w = 186.0;
        let drag_rect = Rect::from_min_max(
            Pos2::new(bar_rect.left() + left_skip, bar_rect.top()),
            Pos2::new(
                (bar_rect.right() - controls_w).max(bar_rect.left() + left_skip),
                bar_rect.bottom(),
            ),
        );
        let drag_resp = ui.interact(
            drag_rect,
            ui.id().with("title_drag"),
            Sense::click_and_drag(),
        );
        if drag_resp.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if drag_resp.double_clicked() {
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }

        let dirty = self.form.is_dirty(&self.settings);

        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_space(10.0);
            if self.view == AppView::Settings {
                if draw_titlebar_chip_btn(ui, p, Icon::ArrowLeft, "返回主页", false, false)
                    .clicked()
                {
                    self.view = AppView::Home;
                }
                ui.add_space(4.0);
                ui.label(body_text("设置中心", 11.5, p.text_muted));
            } else {
                let (badge_rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                ui.painter().image(
                    self.brand_icon.id(),
                    badge_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                ui.label(bold_text(crate::APP_NAME, 12.5, p.text));
                ui.label(body_text(
                    format!("v{}", env!("CARGO_PKG_VERSION")),
                    10.5,
                    p.text_muted,
                ));
            }

            // 右侧窗口控制区：设置入口 + 主题 + 最小化 + 关闭
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;

                if icon_button(
                    ui,
                    p,
                    Icon::Close,
                    Vec2::new(38.0, TITLEBAR_HEIGHT),
                    11.5,
                    true,
                    "关闭窗口（后台守护继续在托盘运行）",
                )
                .clicked()
                {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }

                if icon_button(
                    ui,
                    p,
                    Icon::Minimize,
                    Vec2::new(36.0, TITLEBAR_HEIGHT),
                    11.5,
                    false,
                    "最小化到任务栏",
                )
                .clicked()
                {
                    self.trimmed_while_idle = false;
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                    trim_working_set_win32();
                }

                ui.add_space(4.0);
                let (theme_icon, theme_tip) = match self.ui_prefs.theme {
                    ThemeMode::Light => (Icon::Moon, "切换深色主题"),
                    ThemeMode::Dark => (Icon::Sun, "切换浅色主题"),
                };
                if icon_button(
                    ui,
                    p,
                    theme_icon,
                    Vec2::new(28.0, 26.0),
                    14.0,
                    false,
                    theme_tip,
                )
                .clicked()
                {
                    self.toggle_theme(ctx);
                }

                ui.add_space(4.0);
                let in_settings = self.view == AppView::Settings;
                if draw_titlebar_chip_btn(ui, p, Icon::Settings, "设置", in_settings, dirty)
                    .clicked()
                {
                    self.view = if in_settings {
                        AppView::Home
                    } else {
                        AppView::Settings
                    };
                }
            });
        });
    }

    fn draw_status_ribbon(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let state = self.state();
        let running = self.monitoring();
        let (status_fg, status_soft, status_border) = p.state_colors(state);
        let pulse = matches!(
            state,
            Some(MonitorState::Online)
                | Some(MonitorState::Checking)
                | Some(MonitorState::Authenticating)
                | Some(MonitorState::Verifying)
                | Some(MonitorState::Suspect)
        );

        let metrics = self.latest.as_ref().map(|s| &s.metrics);
        let dur = metrics.map(format_check_ms).unwrap_or_else(|| "—".into());
        let last_at_str = metrics.map(format_check_time).unwrap_or_else(|| "—".into());
        let mut open_account_settings = false;

        accent_card(ui, p, status_fg, |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 44.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    pulsing_dot(ui, status_fg, 26.0, pulse);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        // 第一行：连接状态 + 在线时长 + 嵌入式账号胶囊
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 7.0;
                            ui.label(bold_text(status_title(state), 15.0, p.text));
                            let now_unix = Local::now().timestamp();
                            if let Some(since_ts) = self.online_since_unix {
                                if state == Some(MonitorState::Online) {
                                    let elapsed = (now_unix - since_ts).max(0) as u64;
                                    badge(
                                        ui,
                                        status_fg,
                                        status_soft,
                                        status_border,
                                        &format!("在线 {}", format_uptime(elapsed)),
                                    );
                                }
                            }
                            if draw_account_chip(
                                ui,
                                p,
                                &self.settings.student_id,
                                carrier_label(&self.settings.carrier),
                            ) {
                                open_account_settings = true;
                            }
                        });

                        // 第二行：巡检倒计时与最近检测，异常时直接显示可读故障说明
                        let meta_text = if self.ipc.connection_lost.load(Ordering::Acquire) {
                            "与守护进程的连接已断开，请重新打开主界面".to_string()
                        } else if state == Some(MonitorState::Online) {
                            let mut parts: Vec<String> = Vec::new();
                            if running {
                                if let Some(last_ts) = self.last_check_unix {
                                    let interval = self.settings.check_interval.max(1) as u64;
                                    let elapsed =
                                        (Local::now().timestamp() - last_ts).max(0) as u64;
                                    let rem = interval.saturating_sub(elapsed);
                                    parts.push(format!("{rem}s 后自动巡检"));
                                }
                            }
                            if last_at_str != "—" {
                                if dur != "—" {
                                    parts.push(format!("上次检测 {last_at_str}（网关 {dur}）"));
                                } else {
                                    parts.push(format!("上次检测 {last_at_str}"));
                                }
                            }
                            if parts.is_empty() {
                                "校园网已连接，守护运行正常".to_string()
                            } else {
                                parts.join(" · ")
                            }
                        } else {
                            self.latest
                                .as_ref()
                                .map(|s| s.message.clone())
                                .filter(|m| !m.is_empty())
                                .unwrap_or_else(|| "正在初始化网络诊断…".to_string())
                        };
                        ui.label(body_text(meta_text, 11.0, p.text_muted));
                    });

                    // 右侧主操作按钮（精简为 2 枚：暂停/开始 + 立即检测）
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        if primary_button(ui, p, Some(Icon::Refresh), "立即检测").clicked() {
                            self.pending_manual_check = true;
                            self.show_toast("正在检测校园网连接…", false);
                            self.request(UiToDaemon::ForceCheck, None);
                        }
                        if running {
                            if secondary_button(ui, p, Some(Icon::Pause), "暂停").clicked() {
                                self.request(UiToDaemon::Pause, None);
                            }
                        } else if !self.needs_setup
                            && secondary_button(ui, p, Some(Icon::Play), "开始").clicked()
                        {
                            self.request(UiToDaemon::Start, None);
                        }
                    });
                },
            );
        });

        if open_account_settings {
            self.view = AppView::Settings;
            self.form.tab = settings_page::SettingsTab::Account;
        }
    }

    fn draw_toast(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some((message, is_error, at)) = self.toast.as_ref() else {
            return;
        };
        if at.elapsed() > Duration::from_secs(3) {
            self.toast = None;
            return;
        }

        let (fg, bg, border) = if *is_error {
            (p.danger, p.danger_soft, p.danger_border)
        } else {
            (p.ok, p.ok_soft, p.ok_border)
        };

        egui::Area::new(egui::Id::new("floating_toast"))
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-18.0, -24.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(bg)
                    .stroke(Stroke::new(1.0_f32, border))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                            icons::paint(
                                ui.painter(),
                                r,
                                if *is_error { Icon::Close } else { Icon::Check },
                                fg,
                            );
                            ui.label(bold_text(message.clone(), 12.0, fg));
                        });
                    });
            });
    }
}

fn prompt_account(snapshot: &MonitorSnapshot, opened: &mut bool) -> bool {
    if matches!(
        snapshot.state,
        MonitorState::Online | MonitorState::Paused | MonitorState::Stopped
    ) {
        *opened = false;
    }
    snapshot.state == MonitorState::Offline
        && snapshot.reason.is_some_and(|r| r.needs_account())
        && !std::mem::replace(opened, true)
}

fn merge_sample(history: &mut Vec<TelemetryPoint>, point: &TelemetryPoint) {
    let existing = history
        .iter_mut()
        .rev()
        .take_while(|old| old.ts >= point.ts)
        .find(|old| {
            if point.sample_id.is_some() {
                old.sample_id == point.sample_id
            } else {
                *old == point
            }
        });
    if let Some(old) = existing {
        *old = point.clone();
    } else {
        history.push(point.clone());
        if history.len() > MAX_TELEMETRY_POINTS {
            history.drain(..history.len() - MAX_TELEMETRY_POINTS);
        }
    }
}

impl eframe::App for GuardApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_channels();

        if self.ipc.daemon_closed.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        let minimized = ctx.input(|input| input.viewport().minimized.unwrap_or(false));
        if minimized {
            if !self.trimmed_while_idle {
                self.trimmed_while_idle = true;
                trim_working_set_win32();
            }
            return;
        }
        self.trimmed_while_idle = false;

        let fast_refresh = !self.pending.is_empty()
            || self.toast.is_some()
            || matches!(
                self.state(),
                Some(MonitorState::Checking)
                    | Some(MonitorState::Authenticating)
                    | Some(MonitorState::Verifying)
                    | Some(MonitorState::Suspect)
            );
        ctx.request_repaint_after(if fast_refresh {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        });

        let p = Palette::of(self.ui_prefs.theme);
        self.handle_zoom_shortcuts(ctx);
        self.track_window_size(ctx);
        handle_window_resize(ctx);

        // 1. 36px 原生极简无边框标题栏
        egui::TopBottomPanel::top("custom_titlebar")
            .exact_height(TITLEBAR_HEIGHT)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.draw_titlebar(ctx, ui, &p));

        // 2. 主工作区：主页 or 成熟版 Fluent 设置中心
        match self.view {
            AppView::Home => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin {
                        left: 12,
                        right: 12,
                        top: 10,
                        bottom: 10,
                    }))
                    .show(ctx, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;

                        // Tier 1: 精简状态主条
                        self.draw_status_ribbon(ui, &p);
                        ui.add_space(10.0);

                        // Tier 2 + Tier 3 垂直分配：中部采用紧贴内容的紧凑默认高度，将大窗/缩放多出的空间全部留给运行日志
                        let avail_h = ui.available_height().max(320.0);
                        let splitter_h = 10.0_f32;
                        let usable_h = (avail_h - splitter_h).max(300.0);
                        let (default_mid, min_mid) = if self.ui_prefs.show_home_traffic {
                            (242.0_f32, 220.0_f32)
                        } else {
                            (212.0_f32, 194.0_f32)
                        };
                        let min_log = 148.0_f32;
                        let max_mid = (usable_h - min_log).max(min_mid);
                        let mid_h = self
                            .ui_prefs
                            .home_mid_height
                            .unwrap_or(default_mid)
                            .clamp(min_mid, max_mid)
                            .floor();
                        let log_h = (usable_h - mid_h).max(min_log);

                        // Tier 2: 按设置切换「可拖左右双卡」或「可拖双栏通栏稳定性面板」
                        let show_traffic = self.ui_prefs.show_home_traffic;
                        let split_ref = if show_traffic {
                            &mut self.ui_prefs.home_traffic_split
                        } else {
                            &mut self.ui_prefs.home_wide_split
                        };
                        let split_changed = charts::draw_home_visual_row(
                            ui,
                            &p,
                            &self.telemetry,
                            &mut self.time_range,
                            &self.traffic,
                            &mut self.traffic_period,
                            self.settings.monthly_quota_gb,
                            show_traffic,
                            split_ref,
                            mid_h,
                        );
                        if split_changed {
                            self.window_size_dirty_since = Some(Instant::now());
                        }

                        // 中下可自由拖拽的横向分隔条（双击恢复默认）
                        let (dy, dbl) = horizontal_splitter(ui, &p, ui.available_width());
                        if dbl {
                            self.ui_prefs.home_mid_height = None;
                            self.window_size_dirty_since = Some(Instant::now());
                            self.show_toast("已恢复默认面板高度比例", false);
                        } else if dy.abs() > 0.01 {
                            let next_h = (mid_h + dy).clamp(min_mid, max_mid).round();
                            self.ui_prefs.home_mid_height = Some(next_h);
                            self.window_size_dirty_since = Some(Instant::now());
                        }

                        // Tier 3: 通栏静谧运行日志（吃满剩余全部高度）
                        logs_page::draw_card(
                            ui,
                            &p,
                            &self.log_lines,
                            &mut self.logs_state,
                            &self.log_path,
                            log_h,
                        );
                    });
            }
            AppView::Settings => {
                let saving = self
                    .pending
                    .values()
                    .any(|p| matches!(p.command, UiToDaemon::ReloadSettings { .. }));
                let mut actions = settings_page::SettingsActions::default();
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(p.bg))
                    .show(ctx, |ui| {
                        let total_h = ui.available_height().max(320.0);
                        actions = settings_page::draw_settings_view(
                            ui,
                            &p,
                            &mut self.form,
                            &self.settings,
                            &mut self.ui_prefs,
                            &self.store.app_dir,
                            &self.log_path,
                            self.credential_account.as_deref(),
                            self.needs_setup,
                            saving,
                            total_h,
                        );
                    });
                if actions.ui_prefs_changed {
                    self.ui_prefs = self.ui_prefs.clone().normalized();
                    ctx.set_zoom_factor(self.ui_prefs.zoom);
                    theme::apply_theme(ctx, &Palette::of(self.ui_prefs.theme));
                    theme::save_ui_prefs(&self.store.app_dir, &self.ui_prefs);
                }
                if let Some([w, h]) = actions.set_window_size {
                    let zoom = self.ui_prefs.zoom.max(0.5);
                    self.ui_prefs.window_size = Some([w, h]);
                    self.window_size_dirty_since = None;
                    theme::save_ui_prefs(&self.store.app_dir, &self.ui_prefs);
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(
                        w / zoom,
                        h / zoom,
                    )));
                    self.show_toast(format!("窗口已调整为 {} × {}", w as i32, h as i32), false);
                }
                if actions.save {
                    self.save();
                }
                if actions.reset {
                    self.form.reset_from(&self.settings);
                    self.show_toast("已还原未保存的修改", false);
                }
                if actions.test_toast {
                    self.request(
                        UiToDaemon::TestToast {
                            body: "这是一条测试通知：断网或恢复时会弹出类似提醒".to_string(),
                        },
                        None,
                    );
                }
            }
        }

        self.draw_toast(ctx, &p);

        // 无边框窗口 1px 外描边
        let screen = ctx.screen_rect();
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("window_outline"),
        ))
        .rect_stroke(
            screen,
            CornerRadius::ZERO,
            Stroke::new(1.0_f32, with_alpha(p.window_border, 220)),
            StrokeKind::Inside,
        );
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.window_size_dirty_since.take().is_some() {
            theme::save_ui_prefs(&self.store.app_dir, &self.ui_prefs);
        }
    }
}

fn draw_titlebar_chip_btn(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: Icon,
    label: &str,
    active: bool,
    dot: bool,
) -> egui::Response {
    let font = egui::FontId::new(11.5, crate::ui::theme::bold_family());
    let fg = if active { p.accent } else { p.text_secondary };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    let w = 8.0 + 13.0 + 5.0 + galley.size().x + 8.0 + if dot { 8.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 25.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    if active || resp.hovered() {
        let (fill, stroke) = if active {
            (p.card, Stroke::new(1.0_f32, p.accent_border))
        } else {
            (p.card, Stroke::new(1.0_f32, p.border))
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            fill,
            stroke,
            StrokeKind::Inside,
        );
    }
    let cy = rect.center().y;
    let icon_rect = Rect::from_center_size(Pos2::new(rect.left() + 14.0, cy), Vec2::splat(13.0));
    let ic_color = if active {
        p.accent
    } else if resp.hovered() {
        p.text
    } else {
        p.text_secondary
    };
    icons::paint(ui.painter(), icon_rect, icon, ic_color);
    let text_color = if active {
        p.accent
    } else if resp.hovered() {
        p.text
    } else {
        p.text_secondary
    };
    ui.painter().galley(
        Pos2::new(rect.left() + 25.0, cy - galley.size().y * 0.5),
        galley,
        text_color,
    );
    if dot {
        ui.painter()
            .circle_filled(Pos2::new(rect.right() - 7.0, cy), 3.0, p.warn);
    }
    resp
}

/// 顶部状态条内的嵌入式账号胶囊（阶段二将升级为一键下拉切号菜单）。
fn draw_account_chip(ui: &mut egui::Ui, p: &Palette, student_id: &str, carrier: &str) -> bool {
    let trimmed = student_id.trim();
    let has_account = !trimmed.is_empty();
    let display = if !has_account {
        "未配置账号".to_string()
    } else if trimmed.chars().count() <= 4 {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(4).collect();
        format!("{head}***")
    };

    let mut job = egui::text::LayoutJob::default();
    job.append(
        &display,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::new(11.5, crate::ui::theme::bold_family()),
            color: if has_account { p.text } else { p.warn },
            ..Default::default()
        },
    );
    job.append(
        &format!(" · {carrier}"),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(11.0),
            color: p.text_muted,
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    let w = 8.0 + 12.0 + 5.0 + galley.size().x + 5.0 + 10.0 + 7.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 23.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let stroke = if resp.hovered() {
        Stroke::new(1.0_f32, p.accent)
    } else {
        Stroke::new(1.0_f32, p.border)
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        p.subtle,
        stroke,
        StrokeKind::Inside,
    );
    let cy = rect.center().y;
    let user_r = Rect::from_center_size(Pos2::new(rect.left() + 13.0, cy), Vec2::splat(11.5));
    icons::paint(
        ui.painter(),
        user_r,
        Icon::User,
        if has_account {
            p.text_secondary
        } else {
            p.warn
        },
    );
    ui.painter().galley(
        Pos2::new(rect.left() + 24.0, cy - galley.size().y * 0.5),
        galley,
        p.text,
    );
    let chev_r = Rect::from_center_size(Pos2::new(rect.right() - 11.0, cy), Vec2::splat(9.5));
    icons::paint(ui.painter(), chev_r, Icon::ChevronDown, p.accent);
    let clicked = resp.clicked();
    resp.on_hover_text("点击管理校园网账号与运营商出口");
    clicked
}

/// 无边框窗口四边与四角拖拽调整大小。
fn handle_window_resize(ctx: &egui::Context) {
    let screen = ctx.screen_rect();
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let edge = 5.0;
    let left = pos.x <= screen.left() + edge;
    let right = pos.x >= screen.right() - edge;
    let top = pos.y <= screen.top() + edge;
    let bottom = pos.y >= screen.bottom() - edge;

    let direction = match (left, right, top, bottom) {
        (true, _, true, _) => Some((ResizeDirection::NorthWest, CursorIcon::ResizeNwSe)),
        (_, true, true, _) => Some((ResizeDirection::NorthEast, CursorIcon::ResizeNeSw)),
        (true, _, _, true) => Some((ResizeDirection::SouthWest, CursorIcon::ResizeNeSw)),
        (_, true, _, true) => Some((ResizeDirection::SouthEast, CursorIcon::ResizeNwSe)),
        (true, _, _, _) => Some((ResizeDirection::West, CursorIcon::ResizeHorizontal)),
        (_, true, _, _) => Some((ResizeDirection::East, CursorIcon::ResizeHorizontal)),
        (_, _, true, _) => Some((ResizeDirection::North, CursorIcon::ResizeVertical)),
        (_, _, _, true) => Some((ResizeDirection::South, CursorIcon::ResizeVertical)),
        _ => None,
    };

    if let Some((dir, cursor)) = direction {
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

/// 通知 Windows 裁剪闲置工作集（最小化时显著降低常驻内存）。
fn trim_working_set_win32() {
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetProcessWorkingSetSize(
            hprocess: *mut std::ffi::c_void,
            dwminimumworkingsetsize: usize,
            dwmaximumworkingsetsize: usize,
        ) -> i32;
    }

    unsafe {
        let proc = GetCurrentProcess();
        let _ = SetProcessWorkingSetSize(proc, usize::MAX, usize::MAX);
    }
}

#[cfg(test)]
mod workflow_tests {
    use super::*;
    use crate::models::DisconnectReason;

    #[test]
    fn first_run_respects_shared_credentials_and_account_alerts_open_once() {
        assert!(needs_account_setup("", None));
        assert!(needs_account_setup("fake-other", Some("fake-student")));
        assert!(!needs_account_setup("", Some("fake-student")));
        assert!(!needs_account_setup("fake-student", Some("fake-student")));
        let mut opened = false;
        let rejected = MonitorSnapshot::new(
            MonitorState::Offline,
            Some(DisconnectReason::LoginRejected),
            "",
            true,
        );
        let checking = MonitorSnapshot::new(MonitorState::Checking, None, "", true);
        assert!(prompt_account(&rejected, &mut opened));
        assert!(!prompt_account(&checking, &mut opened));
        assert!(!prompt_account(&rejected, &mut opened));
        assert!(!prompt_account(
            &MonitorSnapshot::new(MonitorState::Online, None, "", true),
            &mut opened
        ));
        assert!(prompt_account(&rejected, &mut opened));
    }

    #[test]
    fn save_and_start_feedback_distinguishes_disk_apply_and_autostart() {
        let mut pending = PendingCommand {
            command: UiToDaemon::ReloadSettings {
                start: true,
                password_changed: true,
                expected: Settings::default(),
            },
            deadline: Instant::now(),
            startup_error: None,
        };
        let (failure, error) = pending.feedback(Err("断开".into())).unwrap();
        assert!(error && failure.contains("已写入磁盘") && failure.contains("未确认"));
        assert_eq!(
            pending.feedback(Ok(())).unwrap(),
            ("设置已生效，守护已启动".into(), false)
        );
        pending.startup_error = Some("mock-denied".into());
        let (partial, error) = pending.feedback(Ok(())).unwrap();
        assert!(error && partial.contains("守护已启动") && partial.contains("修改开机自启失败"));
    }

    #[test]
    fn one_latency_update_replaces_its_initial_sample_and_keeps_same_second_checks() {
        let mut history = Vec::new();
        let mut point = TelemetryPoint::check(100, 0, true, "ONLINE");
        point.sample_id = Some("mock-1".into());
        merge_sample(&mut history, &point);
        point.ms = 42;
        merge_sample(&mut history, &point);
        point.sample_id = Some("mock-2".into());
        merge_sample(&mut history, &point);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].ms, 42);
    }
}
