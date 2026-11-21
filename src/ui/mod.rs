//! 主窗口包含双主题监控看板和设置面板。
//! - 无侧边栏，零整页切换
//! - 顶部：无边框自定义标题栏 + 统一状态/操作栏 + 紧凑四层诊断条
//! - 主体：可拖拽左右分栏（左：账号/监控/通知三标签设置卡，支持一键折叠；右：监控图表 + 实时日志上下可拖分栏）

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
use crate::models::{MonitorSnapshot, MonitorState, Settings};
use crate::startup;

use icons::Icon;
use logs as logs_page;
use settings as settings_page;
use theme::{
    body_text, bold_text, carrier_label, format_check_ms, format_check_time, format_uptime,
    status_title, with_alpha, Palette, ThemeMode,
};
use widgets::{
    accent_card, badge, horizontal_splitter, icon_button, primary_button, pulsing_dot, save_button,
    secondary_button, vertical_splitter,
};

const CREDENTIAL_TARGET: &str = "GXNUCampusLogin/v4";
const MAX_LOG_LINES: usize = 4000;
const MAX_TELEMETRY_POINTS: usize = 5000;
const TITLEBAR_HEIGHT: f32 = 34.0;

pub struct GuardDeps {
    pub store: SettingsStore,
    pub settings: Settings,
    pub ipc: IpcClient,
    pub snapshots: Receiver<SnapshotEnvelope>,
    pub logs: Receiver<String>,
    pub log_path: PathBuf,
    pub initial_logs: Vec<String>,
    pub initial_telemetry: Vec<TelemetryPoint>,
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
    theme_mode: ThemeMode,
    form: settings_page::SettingsForm,
    credential_account: Option<String>,
    logs_state: logs_page::LogsState,
    telemetry: Vec<TelemetryPoint>,
    time_range: TimeRange,
    left_pane_width: f32,
    settings_collapsed: bool,
    chart_ratio: f32,
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
        let theme_mode = theme::load_theme_mode(&deps.store.app_dir);
        let palette = Palette::of(theme_mode);
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
        if let Ok(tab) = std::env::var("GXNU_SETTINGS_TAB") {
            form.tab = match tab.as_str() {
                "monitor" => settings_page::SettingsTab::Monitor,
                "notify" => settings_page::SettingsTab::Notify,
                _ => settings_page::SettingsTab::Account,
            };
        }

        let mut log_lines = deps.initial_logs;
        if log_lines.len() > MAX_LOG_LINES {
            log_lines = log_lines.split_off(log_lines.len() - MAX_LOG_LINES);
        }

        // 默认折叠左侧设置栏，保持主画面清爽聚焦；点击顶部「设置」或标题栏侧栏按钮随时展开。
        let settings_collapsed = std::env::var("GXNU_COLLAPSE")
            .map(|v| v != "0")
            .unwrap_or(true);

        let needs_setup =
            needs_account_setup(&deps.settings.student_id, credential_account.as_deref());
        let settings_collapsed = settings_collapsed && !needs_setup;
        if needs_setup {
            form.tab = settings_page::SettingsTab::Account;
        }

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
            theme_mode,
            form,
            credential_account,
            logs_state: logs_page::LogsState::default(),
            telemetry: deps.initial_telemetry,
            time_range: TimeRange::H24,
            left_pane_width: 292.0,
            settings_collapsed,
            chart_ratio: 0.50,
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
        self.theme_mode = self.theme_mode.toggled();
        let palette = Palette::of(self.theme_mode);
        theme::apply_theme(ctx, &palette);
        theme::save_theme_mode(&self.store.app_dir, self.theme_mode);
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
                self.settings_collapsed = false;
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

        let controls_w = 152.0;
        let drag_rect = Rect::from_min_max(
            bar_rect.min,
            Pos2::new(
                (bar_rect.right() - controls_w).max(bar_rect.left()),
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

        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_space(10.0);
            let (badge_rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
            ui.painter().image(
                self.brand_icon.id(),
                badge_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            ui.label(bold_text("GXNU 校园网守护", 12.5, p.text));
            ui.label(body_text(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                11.0,
                p.text_muted,
            ));

            // 右侧窗口控制按钮
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
                    // 关闭界面进程以彻底释放 OpenGL 驱动与字体内存，后台守护进程仍在托盘轻量运行。
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
                let (theme_icon, theme_tip) = match self.theme_mode {
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

                ui.add_space(2.0);
                let panel_tip = if self.settings_collapsed {
                    "展开左侧设置面板"
                } else {
                    "收起左侧设置面板（宽屏看图表）"
                };
                if icon_button(
                    ui,
                    p,
                    Icon::PanelLeft,
                    Vec2::new(28.0, 26.0),
                    14.0,
                    false,
                    panel_tip,
                )
                .clicked()
                {
                    self.settings_collapsed = !self.settings_collapsed;
                }
            });
        });
    }

    fn draw_top_header_card(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let state = self.state();
        let running = self.monitoring();
        let dirty = self.form.is_dirty(&self.settings);
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
            // 第一行：主状态信息 + 常驻主操作按钮（统一中线对齐）
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 40.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    pulsing_dot(ui, status_fg, 26.0, pulse);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.allocate_ui_with_layout(
                            Vec2::new(420.0, 22.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.label(bold_text(status_title(state), 15.5, p.text));
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
                                if running && state == Some(MonitorState::Online) {
                                    if let Some(last_ts) = self.last_check_unix {
                                        let interval = self.settings.check_interval.max(1) as u64;
                                        let elapsed = (now_unix - last_ts).max(0) as u64;
                                        let rem = interval.saturating_sub(elapsed);
                                        badge(
                                            ui,
                                            p.text_muted,
                                            p.subtle,
                                            p.divider,
                                            &format!("{rem}s 后巡检"),
                                        );
                                    }
                                }
                            },
                        );

                        let msg = if self.ipc.connection_lost.load(Ordering::Acquire) {
                            "与守护进程的连接已断开，请重新打开主界面"
                        } else {
                            self.latest
                                .as_ref()
                                .map(|s| s.message.as_str())
                                .filter(|m| !m.is_empty())
                                .unwrap_or("正在初始化网络诊断…")
                        };
                        ui.label(body_text(msg, 11.5, p.text_secondary));
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        if !self.settings_collapsed || dirty || self.needs_setup {
                            let saving = self
                                .pending
                                .values()
                                .any(|p| matches!(p.command, UiToDaemon::ReloadSettings { .. }));
                            let response = ui
                                .add_enabled_ui(!saving, |ui| {
                                    if self.needs_setup {
                                        primary_button(ui, p, Some(Icon::Check), "保存并开始守护")
                                    } else {
                                        save_button(ui, p, dirty)
                                    }
                                })
                                .inner;
                            if response.clicked() {
                                self.save();
                            }
                        }
                        let settings_label = if self.settings_collapsed {
                            "设置"
                        } else {
                            "收起设置"
                        };
                        if secondary_button(ui, p, Some(Icon::Sliders), settings_label).clicked() {
                            self.settings_collapsed = !self.settings_collapsed;
                        }
                        if primary_button(ui, p, Some(Icon::Refresh), "立即检测").clicked() {
                            self.pending_manual_check = true;
                            self.show_toast("正在检测校园网连接…", false);
                            self.request(UiToDaemon::ForceCheck, None);
                        }
                        if running {
                            if secondary_button(ui, p, Some(Icon::Pause), "暂停监控").clicked()
                            {
                                self.request(UiToDaemon::Pause, None);
                            }
                        } else if !self.needs_setup
                            && secondary_button(ui, p, Some(Icon::Play), "开始监控").clicked()
                        {
                            self.request(UiToDaemon::Start, None);
                        }
                    });
                },
            );

            // 细分隔线 + 当前账号与运营商摘要条
            ui.add_space(6.0);
            let sep = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
            ui.painter().line_segment(
                [sep.left_center(), sep.right_center()],
                Stroke::new(1.0_f32, p.divider),
            );
            ui.add_space(5.0);

            open_account_settings = draw_account_info_strip(
                ui,
                p,
                &self.settings.student_id,
                carrier_label(&self.settings.carrier),
                &dur,
                &last_at_str,
            );
        });

        if open_account_settings {
            self.settings_collapsed = false;
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

        // 若守护进程已退出（例如从托盘点「退出」），界面进程自动关闭。
        if self.ipc.daemon_closed.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        // 最小化到任务栏时停止重绘并裁剪工作集，保持 0% 后台 CPU。
        let minimized = ctx.input(|input| input.viewport().minimized.unwrap_or(false));
        if minimized {
            if !self.trimmed_while_idle {
                self.trimmed_while_idle = true;
                trim_working_set_win32();
            }
            return;
        }
        self.trimmed_while_idle = false;

        // 窗口可见时按需低频刷新（平时 1s 刷一次在线计时；活跃检测/Toast 时 250ms）。
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

        let p = Palette::of(self.theme_mode);
        handle_window_resize(ctx);

        // 1. 顶部无边框自定义标题栏
        egui::TopBottomPanel::top("custom_titlebar")
            .exact_height(TITLEBAR_HEIGHT)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.draw_titlebar(ctx, ui, &p));

        // 2. 底部极简提示条（增加清晰上下呼吸边距，避免贴紧左卡底部）
        egui::TopBottomPanel::bottom("footer_hint")
            .exact_height(28.0)
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(egui::Margin { left: 14, right: 14, top: 2, bottom: 4 }),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(body_text(
                        "拖动分隔线可调整图表与日志比例 · 关闭窗口立即释放界面内存，守护程序仍在系统托盘轻量运行",
                        10.5,
                        p.text_muted,
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(body_text(
                            format!("遥测样本 {} 条", self.telemetry.len()),
                            10.5,
                            p.text_muted,
                        ));
                    });
                });
            });

        // 3. 主单屏工作台（上：状态操作栏；下：左设置 + 右图表/日志可拖分栏）
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 8,
                bottom: 4,
            }))
            .show(ctx, |ui| {
                // 统一卡片间纵向间距为精确 8px（与下方图表和日志之间的 8px 分隔条完全一致）
                ui.spacing_mut().item_spacing.y = 0.0;
                self.draw_top_header_card(ui, &p);
                ui.add_space(8.0);

                let work_h = ui.available_height().max(340.0);
                let total_w = ui.available_width();

                let max_left = (total_w - 430.0).max(260.0);
                self.left_pane_width = self.left_pane_width.clamp(256.0, max_left.min(380.0));

                let mut test_toast_clicked = false;

                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;

                    let right_w = if !self.settings_collapsed {
                        let lw = self.left_pane_width.floor();
                        ui.allocate_ui_with_layout(
                            Vec2::new(lw, work_h),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                let act = settings_page::draw_panel(
                                    ui,
                                    &p,
                                    &mut self.form,
                                    &self.settings,
                                    &self.store.app_dir,
                                    self.credential_account.as_deref(),
                                    work_h,
                                );
                                if act.test_toast {
                                    test_toast_clicked = true;
                                }
                            },
                        );

                        let dx = vertical_splitter(ui, &p, work_h);
                        if dx.abs() > 0.01 {
                            self.left_pane_width =
                                (self.left_pane_width + dx).clamp(256.0, max_left.min(380.0));
                        }
                        (total_w - lw - 8.0).max(360.0)
                    } else {
                        total_w
                    };

                    // 右侧主监控区：上监控图表 + 横向分隔线 + 下实时日志
                    ui.allocate_ui_with_layout(
                        Vec2::new(right_w, work_h),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let usable_h = (work_h - 8.0).max(300.0);
                            let min_chart = 220.0_f32;
                            let min_log = 150.0_f32;
                            let top_h = (usable_h * self.chart_ratio)
                                .clamp(min_chart, (usable_h - min_log).max(min_chart))
                                .floor();
                            let bottom_h = (usable_h - top_h).max(min_log);

                            charts::draw_monitoring_card(
                                ui,
                                &p,
                                &self.telemetry,
                                &mut self.time_range,
                                top_h,
                            );

                            let dy = horizontal_splitter(ui, &p, right_w);
                            if dy.abs() > 0.01 {
                                let new_top = (top_h + dy)
                                    .clamp(min_chart, (usable_h - min_log).max(min_chart));
                                self.chart_ratio = (new_top / usable_h).clamp(0.36, 0.72);
                            }

                            logs_page::draw_card(
                                ui,
                                &p,
                                &self.log_lines,
                                &mut self.logs_state,
                                &self.log_path,
                                bottom_h,
                            );
                        },
                    );
                });

                if test_toast_clicked {
                    self.request(
                        UiToDaemon::TestToast {
                            body: "这是一条测试通知：断网或恢复时会弹出类似提醒".to_string(),
                        },
                        None,
                    );
                }
            });

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

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {}
}

/// 顶部卡片下方：展示当前登录账号与运营商出口（点击可快速打开账号设置），右侧显示最近检测时间与延迟。
fn draw_account_info_strip(
    ui: &mut egui::Ui,
    p: &Palette,
    student_id: &str,
    carrier: &str,
    dur: &str,
    last_at: &str,
) -> bool {
    let mut open_settings = false;
    let has_account = !student_id.trim().is_empty();
    let account_val = if has_account { student_id } else { "未设置" };

    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 24.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 12.0;

            // 1) 当前账号
            if draw_info_chip(
                ui,
                p,
                Icon::Key,
                if has_account { p.accent } else { p.warn },
                "当前账号",
                account_val,
                if has_account { p.text } else { p.warn },
                "点击修改校园网学号与密码",
            ) {
                open_settings = true;
            }

            // 2) 运营商出口
            if draw_info_chip(
                ui,
                p,
                Icon::Wifi,
                p.ok,
                "运营商",
                carrier,
                p.text,
                "点击切换校园网运营商出口",
            ) {
                open_settings = true;
            }

            // 右侧检测摘要
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut parts: Vec<String> = Vec::new();
                if dur != "—" {
                    parts.push(format!("延迟 {dur}"));
                }
                parts.push(format!("上次检测 {last_at}"));
                ui.label(body_text(parts.join(" · "), 11.0, p.text_muted));
            });
        },
    );

    open_settings
}

#[allow(clippy::too_many_arguments)]
fn draw_info_chip(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: Icon,
    icon_color: egui::Color32,
    label: &str,
    value: &str,
    value_color: egui::Color32,
    tip: &str,
) -> bool {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        label,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(11.5),
            color: p.text_muted,
            ..Default::default()
        },
    );
    job.append(
        value,
        5.0,
        egui::TextFormat {
            font_id: egui::FontId::new(11.5, crate::ui::theme::bold_family()),
            color: value_color,
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    let chip_w = 6.0 + 12.0 + 5.0 + galley.size().x + 6.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(chip_w, 24.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            p.subtle,
            Stroke::new(1.0_f32, p.border),
            StrokeKind::Inside,
        );
    }
    let cy = rect.center().y;
    let mut x = rect.left() + 6.0;
    let icon_rect = Rect::from_center_size(Pos2::new(x + 6.0, cy), Vec2::splat(11.5));
    icons::paint(ui.painter(), icon_rect, icon, icon_color);
    x += 17.0;
    ui.painter().galley(
        Pos2::new(x, cy - galley.size().y * 0.5),
        galley,
        value_color,
    );
    let clicked = resp.clicked();
    resp.on_hover_text(tip);
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
