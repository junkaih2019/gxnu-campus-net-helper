//! 主窗口双视图架构（极简薄顶栏 + 主页 / 成熟版 Windows 11 Fluent 设置中心）：
//! - 顶部：36px 无边框自定义标题栏（主页右侧提供「设置」入口，设置页左上角提供「← 返回主页」）
//! - 主页：状态主条 → 账号流量（单圆环可切换运营商/校园网）+ 在线设备双卡 → 纤薄连接稳定性时间轴 → 通栏运行日志
//! - 设置页：左侧分组目录（账号管理 / 监控与自启 / 外观与显示 / 通知与数据）+ 右侧分组卡片 + 底部常驻保存栏

mod charts;
mod icons;
mod logs;
mod settings;
mod theme;
mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use eframe::egui::{
    self, CornerRadius, CursorIcon, Pos2, Rect, ResizeDirection, Sense, Stroke, StrokeKind, Vec2,
    ViewportCommand,
};

use crate::config::SettingsStore;
use crate::credentials;
use crate::ipc::{IpcClient, SnapshotEnvelope, UiToDaemon};
use crate::metrics_store::{TelemetryPoint, TimeRange};
use crate::models::{
    MonitorSnapshot, MonitorState, SelfServiceState, Settings, TrafficSummary, UpdateState,
    UpdateStatus,
};
use crate::startup;

use charts::TrafficViewMode;
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreverifyOutcome {
    /// 门户或自助服务当场核验通过。
    Verified,
    /// 非校园内网、触发图形验证码或并发已满等非账密错误：允许直接落盘保存并附带温和提示。
    SkippedWithNote(String),
    /// 明确的学号不存在、密码错误或未绑定所选运营商：拦截保存并停留在设置页提示重改。
    Rejected(String),
}

struct AccountPreverifyJob {
    rx: Receiver<PreverifyOutcome>,
    also_save_global: bool,
}

/// 选择或切换到「校园网」出口时的 VPN 防封禁居中模态提醒框。
#[derive(Debug, Clone)]
enum CampusVpnModal {
    /// 在设置页点击选择「校园网」出口时的即时提醒。
    CarrierSelected,
    /// 在主页或设置列表点击切换到某个「校园网」账号前的确认拦截。
    ConfirmSwitch {
        account_id: String,
        display_name: String,
    },
    /// 保存并启用某个「校园网」账号前的确认拦截（若点选出口时已确认过则不重复弹出）。
    ConfirmSave {
        also_save_global: bool,
        force_skip_verify: bool,
    },
}

/// 保存账号前的零断网风险当场验密：
/// - 若将设为活跃账号且当前校园网明确离线：直接请求 `yc.gxnu.edu.cn` 实测登录（验对顺带联网）；
/// - 若当前校园网已在线（绝不调 `logout` 踢断好端端的网！）或仅添加备用账号：改用 `172.16.4.2/Self/` 独立会话无损验密；
/// - 仅对明确的账密/运营商错误拦截，离校/超时/验证码/并发满额均平滑放行。
fn preverify_account_credentials(
    student_id: &str,
    password: &str,
    carrier: &str,
    make_active: bool,
    fallback_local_ip: &str,
) -> PreverifyOutcome {
    if make_active {
        if let Some(status) = crate::campus_status::CampusStatusProbe::new().check_detail() {
            if !status.online {
                let auth_ip = status.portal_ip.as_deref().unwrap_or(fallback_local_ip);
                let auth = crate::authenticator::CampusAuthenticator::new()
                    .authenticate(student_id, password, carrier, auth_ip);
                return classify_portal_auth_for_preverify(
                    auth.success,
                    auth.reason,
                    &auth.message,
                );
            }
        }
    }

    let cred = credentials::StoredCredential {
        username: student_id.to_string(),
        password: password.to_string(),
    };
    classify_self_service_for_preverify(crate::self_service::verify_credentials_only(&cred))
}

fn classify_portal_auth_for_preverify(
    success: bool,
    reason: crate::models::DisconnectReason,
    message: &str,
) -> PreverifyOutcome {
    if success {
        return PreverifyOutcome::Verified;
    }
    if reason == crate::models::DisconnectReason::LoginRejected
        && crate::authenticator::is_definite_credential_error(message)
    {
        PreverifyOutcome::Rejected(message.to_string())
    } else {
        PreverifyOutcome::SkippedWithNote(message.to_string())
    }
}

fn classify_self_service_for_preverify(
    res: Result<(), crate::self_service::PortalError>,
) -> PreverifyOutcome {
    use crate::self_service::PortalError;
    match res {
        Ok(()) => PreverifyOutcome::Verified,
        Err(PortalError::AuthFailed(msg)) => {
            if msg.contains("先修改初始密码") {
                PreverifyOutcome::SkippedWithNote(msg)
            } else {
                PreverifyOutcome::Rejected(msg)
            }
        }
        Err(PortalError::CaptchaRequired) => {
            PreverifyOutcome::SkippedWithNote("自助服务当前需图形验证码，已跳过预检直接保存".into())
        }
        Err(PortalError::UnsupportedPassword) => {
            PreverifyOutcome::SkippedWithNote("密码含特殊字符，已跳过自助服务预检并保存".into())
        }
        Err(PortalError::Unreachable(_)) | Err(PortalError::ParseError(_)) => {
            PreverifyOutcome::SkippedWithNote("当前不在校园内网，暂未联网核验密码".into())
        }
    }
}

pub struct GuardApp {
    store: SettingsStore,
    settings: Settings,
    ipc: IpcClient,
    snapshots: Receiver<SnapshotEnvelope>,
    latest: Option<MonitorSnapshot>,
    logs: Receiver<String>,
    log_lines: logs_page::LogLines,
    log_path: PathBuf,
    ui_prefs: UiPrefs,
    window_size_dirty_since: Option<Instant>,
    view: AppView,
    form: settings_page::SettingsForm,
    account_creds: HashMap<String, bool>,
    account_menu_open: bool,
    account_menu_anchor: Option<Rect>,
    account_chip_clicked: bool,
    logs_state: logs_page::LogsState,
    telemetry: Vec<TelemetryPoint>,
    traffic: TrafficSummary,
    self_service: SelfServiceState,
    time_range: TimeRange,
    traffic_view_mode: TrafficViewMode,
    traffic_view_user_override: bool,
    preverify_job: Option<AccountPreverifyJob>,
    campus_vpn_modal: Option<CampusVpnModal>,
    campus_vpn_ack_for_draft: bool,
    kicking_ips: HashSet<String>,
    local_kick_confirm_ip: Option<String>,
    recent_sessions_open: bool,
    recent_sessions_anchor: Option<Rect>,
    recent_sessions_clicked: bool,
    update_state: UpdateState,
    update_modal_open: bool,
    pending_manual_update_check: bool,
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
    #[cfg(debug_assertions)]
    qa_shot_out: Option<PathBuf>,
    #[cfg(debug_assertions)]
    qa_shot_frames: u32,
}

impl GuardApp {
    pub fn new(cc: &eframe::CreationContext<'_>, deps: GuardDeps) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let ui_prefs = theme::load_ui_prefs(&deps.store.app_dir);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        cc.egui_ctx.set_zoom_factor(ui_prefs.zoom);
        let palette = Palette::of(ui_prefs.theme);
        theme::apply_theme(&cc.egui_ctx, &palette);

        #[cfg(debug_assertions)]
        let mock_creds = std::env::var_os("GXNU_SCREENSHOT_OUT").is_some();
        #[cfg(not(debug_assertions))]
        let mock_creds = false;

        let mut account_creds = HashMap::new();
        for acc in &deps.settings.accounts {
            account_creds.insert(
                acc.id.clone(),
                mock_creds || credentials::has_credential_for(&acc.id, &acc.student_id),
            );
        }
        let legacy_cred_account = if !mock_creds && deps.settings.accounts.is_empty() {
            credentials::load(credentials::LEGACY_CREDENTIAL_TARGET)
                .ok()
                .flatten()
                .filter(|stored| !stored.password.is_empty())
                .map(|stored| stored.username)
        } else {
            None
        };
        let mut form = settings_page::SettingsForm::from_settings(&deps.settings);
        if form.student_id.is_empty() {
            form.student_id = legacy_cred_account.clone().unwrap_or_default();
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

        let needs_setup = if let Some(active) = deps.settings.active_account() {
            !account_creds.get(&active.id).copied().unwrap_or(false)
        } else {
            needs_account_setup(&deps.settings.student_id, legacy_cred_account.as_deref())
        };
        let env_view = std::env::var("GXNU_VIEW").ok();
        let env_force_home = env_view
            .as_deref()
            .is_some_and(|v| v.eq_ignore_ascii_case("home"));
        let env_open_settings = env_view
            .as_deref()
            .is_some_and(|v| v.eq_ignore_ascii_case("settings"))
            || env_tab.is_some();
        let view = if env_force_home {
            AppView::Home
        } else if needs_setup || env_open_settings {
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

        let open_update_from_env = std::env::var_os("GXNU_OPEN_UPDATE").is_some();
        let just_updated_from_env = std::env::var_os("GXNU_JUST_UPDATED").is_some();
        let initial_traffic_view = TrafficViewMode::default_for_carrier(&deps.settings.carrier);
        let mut app = Self {
            store: deps.store,
            settings: deps.settings,
            ipc: deps.ipc,
            snapshots: deps.snapshots,
            latest: None,
            logs: deps.logs,
            log_lines: deps.initial_logs.into(),
            log_path: deps.log_path,
            ui_prefs,
            window_size_dirty_since: None,
            view,
            form,
            account_creds,
            account_menu_open: false,
            account_menu_anchor: None,
            account_chip_clicked: false,
            logs_state: logs_page::LogsState::default(),
            telemetry: deps.initial_telemetry,
            traffic: deps.initial_traffic,
            self_service: SelfServiceState::default(),
            time_range: TimeRange::H24,
            traffic_view_mode: initial_traffic_view,
            traffic_view_user_override: false,
            preverify_job: None,
            campus_vpn_modal: None,
            campus_vpn_ack_for_draft: false,
            kicking_ips: HashSet::new(),
            local_kick_confirm_ip: None,
            recent_sessions_open: false,
            recent_sessions_anchor: None,
            recent_sessions_clicked: false,
            update_state: UpdateState {
                current_version: crate::VERSION.to_string(),
                ..Default::default()
            },
            update_modal_open: open_update_from_env,
            pending_manual_update_check: false,
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
            #[cfg(debug_assertions)]
            qa_shot_out: std::env::var_os("GXNU_SCREENSHOT_OUT").map(PathBuf::from),
            #[cfg(debug_assertions)]
            qa_shot_frames: 0,
        };
        if just_updated_from_env {
            app.show_toast(format!("已成功原位更新至 v{}", crate::VERSION), false);
        }
        if !app.needs_setup {
            app.request(UiToDaemon::RefreshSelfService { force: false }, None);
        }
        app
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

    fn refresh_account_creds(&mut self) {
        #[cfg(debug_assertions)]
        let mock_creds = self.qa_shot_out.is_some();
        #[cfg(not(debug_assertions))]
        let mock_creds = false;

        self.account_creds.clear();
        for acc in &self.settings.accounts {
            self.account_creds.insert(
                acc.id.clone(),
                mock_creds || credentials::has_credential_for(&acc.id, &acc.student_id),
            );
        }
    }

    fn save(&mut self, ctx: &egui::Context) {
        if self.form.is_account_draft_dirty(&self.settings) || self.needs_setup {
            self.begin_save_account_profile(true, false, ctx);
        } else {
            self.save_global_settings();
        }
    }

    fn save_global_settings(&mut self) {
        let next = self.form.to_global_settings(&self.settings);
        if let Err(err) = self.store.save_settings(&next) {
            self.show_toast(format!("保存设置失败：{err}"), true);
            return;
        }
        let startup_error = if next.auto_boot != self.settings.auto_boot {
            startup::set_enabled(next.auto_boot).err()
        } else {
            None
        };
        let command = UiToDaemon::ReloadSettings {
            start: false,
            password_changed: false,
            expected: next.clone(),
        };
        self.settings = next;
        if startup_error.is_some() {
            self.settings.auto_boot = startup::is_enabled();
            self.form.auto_boot = self.settings.auto_boot;
        }
        self.show_toast("设置已写入磁盘，等待守护进程确认", false);
        self.request(command, startup_error);
    }

    fn load_reusable_peer_credential(
        &self,
        student_id: &str,
        exclude_id: &str,
    ) -> Option<credentials::StoredCredential> {
        let sid = student_id.trim();
        if sid.is_empty() {
            return None;
        }
        for acc in &self.settings.accounts {
            if acc.id != exclude_id && acc.student_id.trim() == sid {
                if let Ok(Some(cred)) = credentials::load_for_account(&acc.id, sid) {
                    if cred.matches_account(sid) && !cred.password.is_empty() {
                        return Some(cred);
                    }
                }
            }
        }
        None
    }

    fn begin_save_account_profile(
        &mut self,
        also_save_global: bool,
        force_skip_verify: bool,
        ctx: &egui::Context,
    ) {
        if self.preverify_job.is_some() {
            return;
        }
        let student_id = self.form.student_id.trim().to_string();
        if student_id.is_empty() {
            self.show_toast("请输入校园网学号", true);
            return;
        }
        let is_new = self.form.editing_account_id.is_none();
        let account_id = self.form.editing_account_id.clone().unwrap_or_else(|| {
            if self.settings.accounts.is_empty() {
                "default".to_string()
            } else {
                credentials::generate_account_id()
            }
        });
        let carrier_str = self.form.carrier.to_string();
        if self.settings.accounts.iter().any(|a| {
            a.id != account_id && a.student_id.trim() == student_id && a.carrier == carrier_str
        }) {
            self.show_toast(
                format!(
                    "该学号下已存在「{}」出口配置，无需重复添加",
                    carrier_label(&carrier_str)
                ),
                true,
            );
            return;
        }

        let previous = self.settings.find_account(&account_id).cloned();
        let previous_student_id = previous
            .as_ref()
            .map(|a| a.student_id.clone())
            .unwrap_or_default();
        let previous_carrier = previous
            .as_ref()
            .map(|a| a.carrier.clone())
            .unwrap_or_default();
        let lookup_sid = if previous_student_id.is_empty() {
            student_id.as_str()
        } else {
            previous_student_id.as_str()
        };

        let stored = match credentials::load_for_account(&account_id, lookup_sid) {
            Ok(s) => s,
            Err(err) => {
                self.show_toast(format!("读取凭据失败：{err}"), true);
                return;
            }
        };
        let reused_peer = if self.form.password.is_empty() {
            self.load_reusable_peer_credential(&student_id, &account_id)
        } else {
            None
        };
        let has_matching = stored
            .as_ref()
            .is_some_and(|c| c.matches_account(&student_id))
            || reused_peer.is_some();
        if (self.needs_setup || is_new) && self.form.password.is_empty() && !has_matching {
            self.show_toast("请输入该账号的校园网密码", true);
            return;
        }
        if self.form.password.is_empty()
            && !has_matching
            && (student_id != previous_student_id || stored.is_some())
        {
            self.show_toast("更换学号时请同时输入该账号的密码", true);
            return;
        }

        let make_active = self.settings.accounts.is_empty()
            || self.needs_setup
            || account_id == self.settings.active_account_id;

        if make_active
            && self.form.carrier == 0
            && !self.campus_vpn_ack_for_draft
            && self.settings.carrier != "0"
        {
            self.campus_vpn_modal = Some(CampusVpnModal::ConfirmSave {
                also_save_global,
                force_skip_verify,
            });
            return;
        }

        let identity_or_pwd_changed = self.needs_setup
            || is_new
            || !self.form.password.is_empty()
            || student_id != previous_student_id
            || carrier_str != previous_carrier;

        if force_skip_verify || !identity_or_pwd_changed {
            self.commit_account_profile(also_save_global, None);
            return;
        }

        let effective_password = if !self.form.password.is_empty() {
            self.form.password.clone()
        } else if let Some(c) = stored.filter(|c| c.matches_account(&student_id)) {
            c.password
        } else if let Some(peer_cred) = reused_peer {
            peer_cred.password
        } else {
            self.commit_account_profile(also_save_global, None);
            return;
        };

        let fallback_ip = self
            .latest
            .as_ref()
            .map(|s| s.metrics.effective_campus_ip().to_string())
            .unwrap_or_default();

        self.form.verify_error = None;
        self.show_toast("正在核验校园网账号与密码…", false);

        let (tx, rx) = std::sync::mpsc::channel();
        let repaint_ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("account-preverify".to_string())
            .spawn(move || {
                let outcome = preverify_account_credentials(
                    &student_id,
                    &effective_password,
                    &carrier_str,
                    make_active,
                    &fallback_ip,
                );
                let _ = tx.send(outcome);
                repaint_ctx.request_repaint();
            });
        self.preverify_job = Some(AccountPreverifyJob {
            rx,
            also_save_global,
        });
    }

    fn commit_account_profile(&mut self, also_save_global: bool, verify_note: Option<String>) {
        let student_id = self.form.student_id.trim().to_string();
        if student_id.is_empty() {
            self.show_toast("请输入校园网学号", true);
            return;
        }
        let is_new = self.form.editing_account_id.is_none();
        let account_id = self.form.editing_account_id.clone().unwrap_or_else(|| {
            if self.settings.accounts.is_empty() {
                "default".to_string()
            } else {
                credentials::generate_account_id()
            }
        });
        let previous_student_id = self
            .settings
            .find_account(&account_id)
            .map(|a| a.student_id.clone())
            .unwrap_or_default();

        let mut next = if also_save_global {
            self.form.to_global_settings(&self.settings)
        } else {
            self.settings.clone()
        };
        let profile = self.form.draft_profile(account_id.clone());
        let make_active = self.settings.accounts.is_empty()
            || self.needs_setup
            || account_id == self.settings.active_account_id;
        next.upsert_account(profile, make_active);

        let lookup_sid = if previous_student_id.is_empty() {
            student_id.as_str()
        } else {
            previous_student_id.as_str()
        };
        let start = self.needs_setup;
        let entered_password = self.form.password.clone();
        let reused_peer_pwd = if entered_password.is_empty() {
            self.load_reusable_peer_credential(&student_id, &account_id)
                .map(|c| c.password)
        } else {
            None
        };

        let stored = match credentials::load_for_account(&account_id, lookup_sid) {
            Ok(s) => s,
            Err(err) => {
                self.show_toast(format!("保存账号失败：{err}"), true);
                return;
            }
        };
        let stored_matching_pwd = stored
            .as_ref()
            .filter(|c| c.matches_account(&student_id))
            .map(|c| c.password.clone());
        let pwd_to_write = if !entered_password.is_empty() {
            entered_password.clone()
        } else if stored_matching_pwd.is_none() {
            reused_peer_pwd.clone().unwrap_or_default()
        } else {
            String::new()
        };
        if (start || is_new) && pwd_to_write.is_empty() && stored_matching_pwd.is_none() {
            self.show_toast("保存账号失败：请输入该账号的校园网密码", true);
            return;
        }

        // 若勾选了「同时生成校园网快捷切换项」，一并写入配置与凭据
        let companion_pwd = if !pwd_to_write.is_empty() {
            Some(pwd_to_write.clone())
        } else {
            stored_matching_pwd.clone()
        };
        let mut created_companion_id: Option<String> = None;
        if is_new
            && self.form.carrier != 0
            && self.form.also_create_campus_exit
            && !next.has_carrier_for_student(&student_id, "0")
            && companion_pwd.is_some()
        {
            let mut cid = credentials::generate_account_id();
            while cid == account_id || next.find_account(&cid).is_some() {
                cid = credentials::generate_account_id();
            }
            next.upsert_account(
                crate::models::AccountProfile {
                    id: cid.clone(),
                    remark: String::new(),
                    student_id: student_id.clone(),
                    carrier: "0".to_string(),
                    monthly_quota_gb: 0,
                },
                false,
            );
            created_companion_id = Some(cid);
        }

        let target_id = account_id.clone();
        let password_changed = match self.store.save_profile_credential(
            &next,
            &account_id,
            &previous_student_id,
            &pwd_to_write,
            stored.as_ref(),
            |value| match value {
                Some(cred) => {
                    credentials::save_for_account(&target_id, &cred.username, &cred.password)
                }
                None => credentials::delete_for_account(&target_id),
            },
        ) {
            Ok(changed) => changed,
            Err(err) => {
                self.show_toast(format!("保存账号失败：{err}"), true);
                return;
            }
        };

        if let (Some(cid), Some(pwd)) = (&created_companion_id, &companion_pwd) {
            let _ = credentials::save_for_account(cid, &student_id, pwd);
        }

        // 同一学号改密自动联动：当用户输入了新密码时，同步刷新该学号下其他出口条目的加密凭据
        let mut synced_peer_count = 0usize;
        let mut peer_active_updated = false;
        let mut synced_peer_ids = Vec::new();
        if !entered_password.is_empty() {
            for peer in &next.accounts {
                if peer.id != account_id
                    && Some(peer.id.as_str()) != created_companion_id.as_deref()
                    && peer.student_id.trim() == student_id
                    && credentials::save_for_account(&peer.id, &student_id, &entered_password)
                        .is_ok()
                {
                    synced_peer_count += 1;
                    synced_peer_ids.push(peer.id.clone());
                    if peer.id == next.active_account_id {
                        peer_active_updated = true;
                    }
                }
            }
        }

        let display_name = next
            .find_account(&account_id)
            .map(|a| next.account_display_name(a))
            .unwrap_or_else(|| student_id.clone());

        let wrote_primary_pwd = !pwd_to_write.is_empty();
        self.form.password.clear();
        self.form.show_password = false;
        self.form.also_create_campus_exit = false;
        self.form.verify_error = None;
        self.form.editing_account_id = Some(account_id.clone());

        let startup_error = if also_save_global && next.auto_boot != self.settings.auto_boot {
            startup::set_enabled(next.auto_boot).err()
        } else {
            None
        };
        let affects_active = next.active_account_id == account_id;
        let command = UiToDaemon::ReloadSettings {
            start,
            password_changed: (password_changed && affects_active) || peer_active_updated,
            expected: next.clone(),
        };
        self.settings = next;
        if startup_error.is_some() {
            self.settings.auto_boot = startup::is_enabled();
            self.form.auto_boot = self.settings.auto_boot;
        }
        self.refresh_account_creds();
        if wrote_primary_pwd {
            self.account_creds.insert(account_id.clone(), true);
        }
        if let Some(cid) = created_companion_id.as_ref() {
            self.account_creds.insert(cid.clone(), true);
        }
        for pid in synced_peer_ids {
            self.account_creds.insert(pid, true);
        }
        if let Some(active) = self.settings.active_account() {
            if self.account_creds.get(&active.id).copied().unwrap_or(false) {
                self.needs_setup = false;
            }
        }
        let extra_suffix = if created_companion_id.is_some() {
            "，已同步生成「校园网」切换项".to_string()
        } else if synced_peer_count > 0 {
            format!("，已同步更新同号其他 {synced_peer_count} 个出口密码")
        } else {
            String::new()
        };
        let toast_msg = match verify_note {
            Some(note) => format!("已保存账号「{display_name}」{extra_suffix}（{note}）"),
            None if is_new => format!("账号核验通过，已添加「{display_name}」{extra_suffix}"),
            None => format!("账号核验通过，已保存「{display_name}」{extra_suffix}"),
        };
        self.show_toast(toast_msg, false);
        self.request(command, startup_error);
    }

    fn switch_account_to(&mut self, account_id: &str) {
        self.account_menu_open = false;
        if account_id == self.settings.active_account_id {
            return;
        }
        let Some(target) = self.settings.find_account(account_id).cloned() else {
            self.show_toast("未找到目标账号", true);
            return;
        };
        let has_cred = self
            .account_creds
            .get(account_id)
            .copied()
            .unwrap_or_else(|| credentials::has_credential_for(&target.id, &target.student_id));
        let target_label = self.settings.account_display_name(&target);
        if !has_cred {
            self.view = AppView::Settings;
            self.form.tab = settings_page::SettingsTab::Account;
            self.form.load_account_draft(&target);
            self.show_toast(
                format!("请先为「{target_label}」输入并保存密码后再切换"),
                true,
            );
            return;
        }

        if target.carrier == "0" {
            self.campus_vpn_modal = Some(CampusVpnModal::ConfirmSwitch {
                account_id: account_id.to_string(),
                display_name: target_label,
            });
            return;
        }

        self.perform_switch_account_to(account_id);
    }

    fn perform_switch_account_to(&mut self, account_id: &str) {
        let Some(target) = self.settings.find_account(account_id).cloned() else {
            self.show_toast("未找到目标账号", true);
            return;
        };
        let target_label = self.settings.account_display_name(&target);

        let mut next = self.settings.clone();
        if let Err(err) = next.switch_active_account(account_id) {
            self.show_toast(err, true);
            return;
        }
        if let Err(err) = self.store.save_settings(&next) {
            self.show_toast(format!("保存切换配置失败：{err}"), true);
            return;
        }
        self.settings = next.clone();
        self.traffic_view_user_override = false;
        self.traffic_view_mode = TrafficViewMode::default_for_carrier(&self.settings.carrier);
        if !self.form.is_account_draft_dirty(&self.settings) {
            self.form.load_account_draft(&target);
        }
        self.show_toast(format!("正在切换至「{target_label}」…"), false);
        self.request(
            UiToDaemon::SwitchAccount {
                account_id: account_id.to_string(),
                expected: next,
            },
            None,
        );
    }

    fn delete_account_profile(&mut self, account_id: &str) {
        let mut next = self.settings.clone();
        let Some(removed) = next.remove_account(account_id) else {
            return;
        };
        if let Err(err) = self.store.save_settings(&next) {
            self.show_toast(format!("删除账号失败：{err}"), true);
            return;
        }
        let _ = credentials::delete_for_account(account_id);
        self.settings = next.clone();
        self.refresh_account_creds();
        if self.form.editing_account_id.as_deref() == Some(account_id) {
            if let Some(active) = self.settings.active_account().cloned() {
                self.form.load_account_draft(&active);
            } else {
                self.form.start_new_account();
            }
        }
        if self.settings.accounts.is_empty() {
            self.needs_setup = true;
        }
        self.show_toast(format!("已删除账号「{}」", removed.display_name()), false);
        self.request(
            UiToDaemon::ReloadSettings {
                start: false,
                password_changed: false,
                expected: next,
            },
            None,
        );
    }

    fn drain_channels(&mut self) {
        if let Some(job) = &self.preverify_job {
            match job.rx.try_recv() {
                Ok(outcome) => {
                    let also_global = job.also_save_global;
                    self.preverify_job = None;
                    match outcome {
                        PreverifyOutcome::Verified => {
                            self.commit_account_profile(also_global, None);
                        }
                        PreverifyOutcome::SkippedWithNote(note) => {
                            self.commit_account_profile(also_global, Some(note));
                        }
                        PreverifyOutcome::Rejected(err_msg) => {
                            self.form.verify_error = Some(err_msg.clone());
                            self.show_toast(format!("登录校验未通过：{err_msg}"), true);
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let also_global = job.also_save_global;
                    self.preverify_job = None;
                    self.commit_account_profile(also_global, None);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }

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
        while let Ok((ip, is_local, result)) = self.ipc.kick_results.try_recv() {
            self.kicking_ips.remove(&ip);
            match result {
                Ok(()) => {
                    let msg = if is_local {
                        format!("已注销本机连接（{ip}）并暂停自动重连守护")
                    } else {
                        format!("已将终端 {ip} 注销下线")
                    };
                    self.show_toast(msg, false);
                }
                Err(err) => {
                    self.show_toast(format!("注销 {ip} 失败：{err}"), true);
                }
            }
        }

        while let Ok(upd) = self.ipc.update_states.try_recv() {
            if self.pending_manual_update_check && upd.status != UpdateStatus::Checking {
                self.pending_manual_update_check = false;
                match upd.status {
                    UpdateStatus::Available => {
                        self.update_modal_open = true;
                    }
                    UpdateStatus::UpToDate => {
                        self.show_toast(format!("当前已是最新版本（v{}）", crate::VERSION), false);
                    }
                    UpdateStatus::Error => {
                        self.show_toast(format!("检查更新失败：{}", upd.message), true);
                    }
                    _ => {}
                }
            }
            self.update_state = upd;
        }
        if self.ipc.open_update_modal.swap(false, Ordering::AcqRel) {
            self.update_modal_open = true;
        }

        if self.ipc.open_account_settings.swap(false, Ordering::AcqRel) {
            self.settings = self.store.load();
            self.refresh_account_creds();
            self.account_menu_open = false;
            self.view = AppView::Settings;
            self.form.tab = settings_page::SettingsTab::Account;
            if !self.form.is_account_draft_dirty(&self.settings) {
                if let Some(active) = self.settings.active_account().cloned() {
                    self.form.load_account_draft(&active);
                }
            }
        }

        while let Ok(env) = self.snapshots.try_recv() {
            let snapshot = env.snapshot;
            self.online_since_unix = env.online_since_unix;
            self.last_check_unix = env.last_check_unix;
            self.traffic = env.traffic;
            self.self_service = env.self_service;
            if env.update.status != UpdateStatus::Idle
                || self.update_state.status == UpdateStatus::Idle
            {
                self.update_state = env.update;
            }
            if !env.active_account_id.is_empty()
                && env.active_account_id != self.settings.active_account_id
            {
                if self
                    .settings
                    .switch_active_account(&env.active_account_id)
                    .is_err()
                {
                    self.settings = self.store.load();
                }
                self.traffic_view_user_override = false;
                self.traffic_view_mode =
                    TrafficViewMode::default_for_carrier(&self.settings.carrier);
                if !self.form.is_account_draft_dirty(&self.settings) {
                    if let Some(active) = self.settings.active_account().cloned() {
                        self.form.load_account_draft(&active);
                    }
                }
            } else if !self.traffic_view_user_override {
                self.traffic_view_mode =
                    TrafficViewMode::default_for_carrier(&self.settings.carrier);
            }

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

        while let Ok(line) = self.logs.try_recv() {
            self.log_lines.push(line);
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

        let has_upd = self
            .update_state
            .has_visible_update(&self.settings.skipped_update_version);
        let left_skip = if self.view == AppView::Settings {
            110.0
        } else {
            0.0
        };
        let controls_w = if has_upd { 306.0 } else { 186.0 };
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
                let title_g = ui.painter().layout_no_wrap(
                    crate::APP_NAME.to_string(),
                    egui::FontId::new(12.5, theme::bold_family()),
                    p.text,
                );
                let ver_g = ui.painter().layout_no_wrap(
                    format!("v{}", crate::VERSION),
                    egui::FontId::proportional(10.5),
                    p.text_muted,
                );
                let gap = 6.0_f32;
                let total_w = title_g.size().x + gap + ver_g.size().x;
                let (text_rect, _) =
                    ui.allocate_exact_size(Vec2::new(total_w, TITLEBAR_HEIGHT), Sense::hover());
                let cy = text_rect.center().y;
                let title_y = cy - title_g.size().y * 0.5;
                // 让版本号底部与左侧中文标题的最底边缘严格齐平（补偿汉字全字身相对拉丁基线的下探）
                let ver_y = title_y + (title_g.size().y - ver_g.size().y) + 1.15;
                let ver_x = text_rect.left() + title_g.size().x + gap;
                ui.painter()
                    .galley(Pos2::new(text_rect.left(), title_y), title_g, p.text);
                ui.painter()
                    .galley(Pos2::new(ver_x, ver_y), ver_g, p.text_muted);
            }

            // 右侧窗口控制区：更新提醒胶囊 + 设置入口 + 主题 + 最小化 + 关闭
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
                    self.account_menu_open = false;
                    self.view = if in_settings {
                        AppView::Home
                    } else {
                        AppView::Settings
                    };
                }

                if has_upd {
                    ui.add_space(6.0);
                    let pill_lbl = match self.update_state.status {
                        UpdateStatus::Downloading => {
                            let pct = self
                                .update_state
                                .progress_ratio()
                                .map(|r| format!(" {}%", (r * 100.0).round() as i32))
                                .unwrap_or_default();
                            format!("更新中{pct}")
                        }
                        UpdateStatus::Applying => "正在重启…".to_string(),
                        _ => {
                            let ver = self
                                .update_state
                                .release
                                .as_ref()
                                .map(|r| r.version.as_str())
                                .unwrap_or("");
                            format!("新版本 v{ver}")
                        }
                    };
                    if draw_titlebar_update_pill(ui, p, &pill_lbl).clicked() {
                        self.update_modal_open = true;
                    }
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
        let campus_ip = self.effective_campus_ip();
        self.account_chip_clicked = false;
        let display_name = self.settings.active_display_name();
        let has_account =
            !self.settings.accounts.is_empty() || !self.settings.student_id.is_empty();
        let mut chip_clicked = false;
        let mut chip_rect = Rect::NOTHING;

        accent_card(ui, p, status_fg, |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 44.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    pulsing_dot(ui, status_fg, 26.0, pulse);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        // 第一行：连接状态 + 真实在线时长（优先由门户校准） + 一键切号胶囊
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 7.0;
                            ui.label(bold_text(status_title(state), 15.0, p.text));
                            let now_unix = Local::now().timestamp();
                            if state == Some(MonitorState::Online) {
                                let portal_since = self
                                    .self_service
                                    .data
                                    .as_ref()
                                    .and_then(|d| d.matched_local_since_unix(&campus_ip));
                                if let Some(since_ts) = portal_since.or(self.online_since_unix) {
                                    let elapsed = (now_unix - since_ts).max(0) as u64;
                                    let tip = if portal_since.is_some() {
                                        "已由校园网自助服务会话上线时间校准"
                                    } else {
                                        "本地守护进程记录的连续在线时长"
                                    };
                                    badge(
                                        ui,
                                        status_fg,
                                        status_soft,
                                        status_border,
                                        &format!("在线 {}", format_uptime(elapsed)),
                                    )
                                    .on_hover_text(tip);
                                }
                            }
                            let (clicked, rect) = draw_account_chip(
                                ui,
                                p,
                                &display_name,
                                carrier_label(&self.settings.carrier),
                                has_account,
                                self.account_menu_open,
                            );
                            chip_clicked = clicked;
                            chip_rect = rect;
                        });

                        // 第二行：本机真实校园网 IP + 巡检倒计时与最近检测，异常时直接显示可读故障说明
                        let meta_text = if self.ipc.connection_lost.load(Ordering::Acquire) {
                            "与守护进程的连接已断开，请重新打开主界面".to_string()
                        } else if state == Some(MonitorState::Online) {
                            let mut parts: Vec<String> = Vec::new();
                            if !campus_ip.is_empty() {
                                parts.push(format!("本机 {campus_ip}"));
                            }
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

        self.account_menu_anchor = Some(chip_rect);
        if chip_clicked {
            self.account_chip_clicked = true;
            if self.settings.accounts.is_empty() {
                self.account_menu_open = false;
                self.view = AppView::Settings;
                self.form.tab = settings_page::SettingsTab::Account;
            } else {
                self.account_menu_open = !self.account_menu_open;
            }
        }
    }

    fn draw_account_switcher_popover(&mut self, ctx: &egui::Context, p: &Palette) {
        if self.view != AppView::Home || !self.account_menu_open {
            return;
        }
        let Some(anchor) = self.account_menu_anchor else {
            return;
        };

        let pop_w = 258.0_f32;
        let screen = ctx.screen_rect();
        let x = anchor
            .left()
            .min(screen.right() - pop_w - 10.0)
            .max(screen.left() + 10.0);
        let y = anchor.bottom() + 5.0;

        let mut switch_target: Option<String> = None;
        let mut open_manage = false;
        let mut close_now = false;

        let shadow = match p.mode {
            ThemeMode::Light => egui::Shadow {
                offset: [0, 6],
                blur: 18,
                spread: 0,
                color: with_alpha(egui::Color32::BLACK, 34),
            },
            ThemeMode::Dark => egui::Shadow {
                offset: [0, 8],
                blur: 22,
                spread: 0,
                color: with_alpha(egui::Color32::BLACK, 120),
            },
        };

        let area_resp = egui::Area::new(egui::Id::new("account_switcher_popover"))
            .fixed_pos(Pos2::new(x, y))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0_f32, p.border_strong))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(8))
                    .shadow(shadow)
                    .show(ui, |ui| {
                        let inner_w = pop_w - 16.0;
                        ui.set_width(inner_w);
                        ui.spacing_mut().item_spacing.y = 4.0;

                        // 顶栏标题
                        ui.allocate_ui_with_layout(
                            Vec2::new(inner_w, 20.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add_space(4.0);
                                ui.label(bold_text("切换校园网账号", 11.5, p.text_secondary));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(4.0);
                                        ui.label(body_text(
                                            format!("共 {} 个", self.settings.accounts.len()),
                                            10.5,
                                            p.text_muted,
                                        ));
                                    },
                                );
                            },
                        );

                        let sep1 = ui.allocate_space(Vec2::new(inner_w, 1.0)).1;
                        ui.painter().line_segment(
                            [sep1.left_center(), sep1.right_center()],
                            Stroke::new(1.0_f32, p.divider),
                        );

                        egui::ScrollArea::vertical()
                            .id_salt("account_switcher_list")
                            .max_height(240.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.set_width(inner_w);
                                ui.spacing_mut().item_spacing.y = 3.0;
                                for acc in &self.settings.accounts {
                                    let is_active = acc.id == self.settings.active_account_id;
                                    let has_cred =
                                        self.account_creds.get(&acc.id).copied().unwrap_or(false);
                                    let (rect, resp) = ui.allocate_exact_size(
                                        Vec2::new(inner_w, 42.0),
                                        Sense::click(),
                                    );
                                    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                                    let (bg, border) = if is_active {
                                        (p.accent_soft, Stroke::new(1.0_f32, p.accent_border))
                                    } else if resp.hovered() {
                                        (p.card_hover, Stroke::new(1.0_f32, p.border))
                                    } else {
                                        (egui::Color32::TRANSPARENT, Stroke::NONE)
                                    };
                                    if bg != egui::Color32::TRANSPARENT {
                                        ui.painter().rect(
                                            rect,
                                            CornerRadius::same(7),
                                            bg,
                                            border,
                                            StrokeKind::Inside,
                                        );
                                    }

                                    let icon_c = if is_active {
                                        p.ok
                                    } else if resp.hovered() {
                                        p.accent
                                    } else {
                                        p.text_muted
                                    };
                                    let ic_r = Rect::from_center_size(
                                        Pos2::new(rect.left() + 16.0, rect.center().y),
                                        Vec2::splat(13.0),
                                    );
                                    icons::paint(
                                        ui.painter(),
                                        ic_r,
                                        if is_active { Icon::Check } else { Icon::User },
                                        icon_c,
                                    );

                                    let disp = self.settings.account_display_name(acc);
                                    let carrier_lbl = carrier_label(&acc.carrier);
                                    let tx = rect.left() + 31.0;
                                    let title_font =
                                        egui::FontId::new(12.0, crate::ui::theme::bold_family());
                                    ui.painter().text(
                                        Pos2::new(tx, rect.top() + 13.5),
                                        egui::Align2::LEFT_CENTER,
                                        &disp,
                                        title_font,
                                        p.text,
                                    );
                                    let sub = if !acc.remark.trim().is_empty() {
                                        format!("{} · {}", acc.masked_student_id(), carrier_lbl)
                                    } else if disp == carrier_lbl {
                                        acc.masked_student_id()
                                    } else {
                                        carrier_lbl.to_string()
                                    };
                                    ui.painter().text(
                                        Pos2::new(tx, rect.bottom() - 12.0),
                                        egui::Align2::LEFT_CENTER,
                                        sub,
                                        egui::FontId::proportional(10.5),
                                        p.text_muted,
                                    );

                                    // 右侧状态标签
                                    let right_tag = if is_active {
                                        Some(("当前", p.ok, p.ok_soft, p.ok_border))
                                    } else if !has_cred {
                                        Some(("缺密码", p.warn, p.warn_soft, p.warn_border))
                                    } else if resp.hovered() {
                                        Some(("切换", p.accent, p.accent_soft, p.accent_border))
                                    } else {
                                        None
                                    };
                                    if let Some((lbl, fg, sbg, sb)) = right_tag {
                                        let tf = egui::FontId::new(
                                            10.0,
                                            crate::ui::theme::bold_family(),
                                        );
                                        let g =
                                            ui.painter().layout_no_wrap(lbl.to_string(), tf, fg);
                                        let pr = Rect::from_center_size(
                                            Pos2::new(
                                                rect.right() - 8.0 - (g.size().x + 10.0) * 0.5,
                                                rect.center().y,
                                            ),
                                            Vec2::new(g.size().x + 10.0, 18.0),
                                        );
                                        ui.painter().rect(
                                            pr,
                                            CornerRadius::same(4),
                                            sbg,
                                            Stroke::new(1.0_f32, sb),
                                            StrokeKind::Inside,
                                        );
                                        ui.painter().galley(
                                            Pos2::new(
                                                pr.center().x - g.size().x * 0.5,
                                                pr.center().y - g.size().y * 0.5,
                                            ),
                                            g,
                                            fg,
                                        );
                                    }

                                    if resp.clicked() {
                                        if is_active {
                                            close_now = true;
                                        } else {
                                            switch_target = Some(acc.id.clone());
                                        }
                                    }
                                }
                            });

                        let sep2 = ui.allocate_space(Vec2::new(inner_w, 1.0)).1;
                        ui.painter().line_segment(
                            [sep2.left_center(), sep2.right_center()],
                            Stroke::new(1.0_f32, p.divider),
                        );

                        // 底部「管理 / 添加账号…」入口
                        let (btn_rect, btn_resp) =
                            ui.allocate_exact_size(Vec2::new(inner_w, 30.0), Sense::click());
                        let btn_resp = btn_resp.on_hover_cursor(CursorIcon::PointingHand);
                        if btn_resp.hovered() {
                            ui.painter()
                                .rect_filled(btn_rect, CornerRadius::same(6), p.subtle);
                        }
                        let plus_r = Rect::from_center_size(
                            Pos2::new(btn_rect.left() + 16.0, btn_rect.center().y),
                            Vec2::splat(11.5),
                        );
                        let lbl_c = if btn_resp.hovered() {
                            p.accent
                        } else {
                            p.text_secondary
                        };
                        icons::paint(ui.painter(), plus_r, Icon::Plus, lbl_c);
                        ui.painter().text(
                            Pos2::new(btn_rect.left() + 31.0, btn_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            "管理 / 添加账号…",
                            egui::FontId::new(11.5, crate::ui::theme::bold_family()),
                            lbl_c,
                        );
                        if btn_resp.clicked() {
                            open_manage = true;
                        }
                    });
            });

        if let Some(id) = switch_target {
            self.switch_account_to(&id);
        } else if open_manage {
            self.account_menu_open = false;
            self.view = AppView::Settings;
            self.form.tab = settings_page::SettingsTab::Account;
        } else if close_now {
            self.account_menu_open = false;
        } else if !self.account_chip_clicked {
            let area_rect = area_resp.response.rect;
            let dismiss = ctx.input(|i| {
                i.key_pressed(egui::Key::Escape)
                    || (i.pointer.primary_pressed()
                        && i.pointer
                            .interact_pos()
                            .is_some_and(|pos| !area_rect.contains(pos) && !anchor.contains(pos)))
            });
            if dismiss {
                self.account_menu_open = false;
            }
        }
    }

    fn draw_recent_sessions_popover(&mut self, ctx: &egui::Context, p: &Palette) {
        if self.view != AppView::Home || !self.recent_sessions_open {
            return;
        }
        let Some(anchor) = self.recent_sessions_anchor else {
            self.recent_sessions_open = false;
            return;
        };
        let Some(data) = &self.self_service.data else {
            self.recent_sessions_open = false;
            return;
        };
        if data.recent_sessions.is_empty() {
            self.recent_sessions_open = false;
            return;
        }

        let pop_w = 336.0_f32;
        let est_h = 58.0 + (data.recent_sessions.len() as f32) * 26.0;
        let screen = ctx.screen_rect();
        let x = (anchor.right() - pop_w)
            .min(screen.right() - pop_w - 10.0)
            .max(screen.left() + 10.0);
        let y = if anchor.bottom() + 6.0 + est_h <= screen.bottom() - 10.0 {
            anchor.bottom() + 6.0
        } else {
            (anchor.top() - est_h - 6.0).max(screen.top() + 38.0)
        };

        let shadow = match p.mode {
            ThemeMode::Light => egui::Shadow {
                offset: [0, 6],
                blur: 18,
                spread: 0,
                color: with_alpha(egui::Color32::BLACK, 34),
            },
            ThemeMode::Dark => egui::Shadow {
                offset: [0, 8],
                blur: 22,
                spread: 0,
                color: with_alpha(egui::Color32::BLACK, 125),
            },
        };

        let mut close_clicked = false;
        let sessions = data.recent_sessions.clone();
        let local_ip = self.effective_campus_ip();

        let area_resp = egui::Area::new(egui::Id::new("recent_sessions_popover"))
            .fixed_pos(Pos2::new(x, y))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0_f32, p.border_strong))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .shadow(shadow)
                    .show(ui, |ui| {
                        let inner_w = pop_w - 24.0;
                        ui.set_width(inner_w);
                        ui.spacing_mut().item_spacing.y = 5.0;

                        ui.allocate_ui_with_layout(
                            Vec2::new(inner_w, 22.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.label(bold_text("近期上网记录", 12.0, p.text));
                                ui.label(body_text(
                                    format!("最近 {} 条 · 实际流量", sessions.len()),
                                    10.2,
                                    p.text_muted,
                                ));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if widgets::icon_button(
                                            ui,
                                            p,
                                            Icon::Close,
                                            Vec2::splat(20.0),
                                            10.5,
                                            false,
                                            "关闭",
                                        )
                                        .clicked()
                                        {
                                            close_clicked = true;
                                        }
                                    },
                                );
                            },
                        );

                        let sep = ui.allocate_space(Vec2::new(inner_w, 1.0)).1;
                        ui.painter().line_segment(
                            [sep.left_center(), sep.right_center()],
                            Stroke::new(1.0_f32, p.divider),
                        );

                        for rec in &sessions {
                            let start_str = Local
                                .timestamp_opt(rec.login_unix_ms / 1000, 0)
                                .single()
                                .map(|dt| dt.format("%m-%d %H:%M").to_string())
                                .unwrap_or_else(|| "--:--".to_string());
                            let end_str = rec
                                .logout_unix_ms
                                .and_then(|ms| Local.timestamp_opt(ms / 1000, 0).single())
                                .map(|dt| dt.format("%H:%M").to_string())
                                .unwrap_or_else(|| "在线".to_string());
                            let is_local_ip = !local_ip.is_empty() && rec.ip == local_ip;

                            let (row_rect, _) =
                                ui.allocate_exact_size(Vec2::new(inner_w, 22.0), Sense::hover());
                            let cy = row_rect.center().y;
                            let time_txt = format!("{start_str}–{end_str}");
                            let tg = ui.painter().layout_no_wrap(
                                time_txt,
                                egui::FontId::proportional(10.5),
                                p.text_secondary,
                            );
                            ui.painter().galley(
                                Pos2::new(row_rect.left(), cy - tg.size().y * 0.5),
                                tg.clone(),
                                p.text_secondary,
                            );

                            let ip_c = if is_local_ip { p.ok } else { p.text };
                            let ig = ui.painter().layout_no_wrap(
                                rec.ip.clone(),
                                egui::FontId::new(10.5, crate::ui::theme::bold_family()),
                                ip_c,
                            );
                            ui.painter().galley(
                                Pos2::new(
                                    row_rect.left() + tg.size().x + 8.0,
                                    cy - ig.size().y * 0.5,
                                ),
                                ig,
                                ip_c,
                            );

                            let right_txt = format!(
                                "{} · {}",
                                charts::format_duration_short(rec.duration_secs),
                                charts::format_mb_compact(rec.actual_total_mb())
                            );
                            let rg = ui.painter().layout_no_wrap(
                                right_txt,
                                egui::FontId::proportional(10.5),
                                p.text_muted,
                            );
                            ui.painter().galley(
                                Pos2::new(row_rect.right() - rg.size().x, cy - rg.size().y * 0.5),
                                rg,
                                p.text_muted,
                            );
                        }
                    });
            });

        if close_clicked {
            self.recent_sessions_open = false;
        } else if !self.recent_sessions_clicked {
            let area_rect = area_resp.response.rect;
            let dismiss = ctx.input(|i| {
                i.key_pressed(egui::Key::Escape)
                    || (i.pointer.primary_pressed()
                        && i.pointer
                            .interact_pos()
                            .is_some_and(|pos| !area_rect.contains(pos) && !anchor.contains(pos)))
            });
            if dismiss {
                self.recent_sessions_open = false;
            }
        }
    }

    fn effective_campus_ip(&self) -> String {
        if let Some(snap) = &self.latest {
            let ip = snap.metrics.effective_campus_ip();
            if !ip.is_empty() {
                return ip.to_string();
            }
        }
        if self.state() == Some(MonitorState::Online) {
            if let Some(data) = &self.self_service.data {
                if data.online_devices.len() == 1 {
                    return data.online_devices[0].ip.clone();
                }
            }
        }
        String::new()
    }

    fn draw_campus_vpn_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(modal) = self.campus_vpn_modal.clone() else {
            return;
        };

        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("campus_vpn_modal_scrim"))
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
                let alpha = if p.mode == ThemeMode::Dark { 145 } else { 105 };
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::ZERO,
                    egui::Color32::from_black_alpha(alpha),
                );
            });

        let mut confirm = false;
        let mut cancel = false;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                cancel = true;
            } else if i.key_pressed(egui::Key::Enter) {
                confirm = true;
            }
        });

        let (title, body, primary_lbl, has_cancel) = match &modal {
            CampusVpnModal::CarrierSelected => (
                "「校园网」出口防封提醒",
                "您已选择「校园网」出口。使用校园网出口联网时，请务必先关闭本机 VPN、代理软件（如 Clash / Mihomo / v2rayN）或游戏加速器，避免触发校园网安全监测导致账号被封禁！".to_string(),
                "我知道了",
                false,
            ),
            CampusVpnModal::ConfirmSwitch { display_name, .. } => {
                let target_desc = if display_name.trim() == "校园网" {
                    "「校园网」出口".to_string()
                } else {
                    format!("「{display_name}」（校园网出口）")
                };
                (
                    "切换至校园网出口安全提醒",
                    format!(
                        "即将切换至{target_desc}。切换前请确认已关闭本机 VPN、代理软件（如 Clash / Mihomo / v2rayN）或游戏加速器，避免触发校园网安全监测导致账号被封禁！"
                    ),
                    "已关闭 VPN，继续切换",
                    true,
                )
            }
            CampusVpnModal::ConfirmSave { .. } => (
                "启用校园网出口安全提醒",
                "即将保存并启用「校园网」出口。连接前请确认已关闭本机 VPN、代理软件（如 Clash / Mihomo / v2rayN）或游戏加速器，避免触发校园网安全监测导致账号被封禁！".to_string(),
                "已关闭 VPN，继续保存",
                true,
            ),
        };

        egui::Area::new(egui::Id::new("campus_vpn_modal_dialog"))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                let card_w = 396.0_f32.min((screen.width() - 36.0).max(280.0));
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.2_f32, p.warn_border))
                    .corner_radius(CornerRadius::same(12))
                    .shadow(egui::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(90),
                    })
                    .inner_margin(egui::Margin::symmetric(18, 16))
                    .show(ui, |ui| {
                        ui.set_width(card_w);
                        ui.spacing_mut().item_spacing.y = 10.0;

                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 9.0;
                            let (badge_r, _) =
                                ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                            ui.painter().rect(
                                badge_r,
                                CornerRadius::same(8),
                                p.warn_soft,
                                Stroke::new(1.0_f32, p.warn_border),
                                StrokeKind::Inside,
                            );
                            icons::paint(ui.painter(), badge_r.shrink(6.0), Icon::Shield, p.warn);
                            ui.label(bold_text(title, 14.0, p.text));
                        });

                        ui.add(egui::Label::new(body_text(body, 12.0, p.text_secondary)).wrap());

                        ui.add_space(4.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            if primary_button(ui, p, Some(Icon::Check), primary_lbl).clicked() {
                                confirm = true;
                            }
                            if has_cancel && secondary_button(ui, p, None, "取消").clicked() {
                                cancel = true;
                            }
                        });
                    });
            });

        if cancel {
            if matches!(modal, CampusVpnModal::CarrierSelected) {
                self.campus_vpn_ack_for_draft = true;
            }
            self.campus_vpn_modal = None;
        } else if confirm {
            self.campus_vpn_modal = None;
            match modal {
                CampusVpnModal::CarrierSelected => {
                    self.campus_vpn_ack_for_draft = true;
                }
                CampusVpnModal::ConfirmSwitch { account_id, .. } => {
                    self.perform_switch_account_to(&account_id);
                }
                CampusVpnModal::ConfirmSave {
                    also_save_global,
                    force_skip_verify,
                } => {
                    self.campus_vpn_ack_for_draft = true;
                    self.begin_save_account_profile(also_save_global, force_skip_verify, ctx);
                }
            }
        }
    }

    fn draw_local_kick_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(ip) = self.local_kick_confirm_ip.clone() else {
            return;
        };

        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("local_kick_modal_scrim"))
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
                let alpha = if p.mode == ThemeMode::Dark { 145 } else { 105 };
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::ZERO,
                    egui::Color32::from_black_alpha(alpha),
                );
            });

        let mut confirm = false;
        let mut cancel = false;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                cancel = true;
            } else if i.key_pressed(egui::Key::Enter) {
                confirm = true;
            }
        });

        egui::Area::new(egui::Id::new("local_kick_modal_dialog"))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                let card_w = 384.0_f32.min((screen.width() - 36.0).max(280.0));
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.2_f32, p.warn_border))
                    .corner_radius(CornerRadius::same(12))
                    .shadow(egui::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(90),
                    })
                    .inner_margin(egui::Margin::symmetric(18, 16))
                    .show(ui, |ui| {
                        ui.set_width(card_w);
                        ui.spacing_mut().item_spacing.y = 10.0;

                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 9.0;
                            let (badge_r, _) =
                                ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                            ui.painter().rect(
                                badge_r,
                                CornerRadius::same(8),
                                p.warn_soft,
                                Stroke::new(1.0_f32, p.warn_border),
                                StrokeKind::Inside,
                            );
                            icons::paint(ui.painter(), badge_r.shrink(6.0), Icon::Pause, p.warn);
                            ui.label(bold_text("确认注销本机校园网连接？", 14.0, p.text));
                        });

                        ui.add(
                            egui::Label::new(body_text(
                                format!(
                                    "即将注销本机在线会话（{ip}）。为避免几秒后又被自动重连回来，后台自动重连守护将同步暂停；后续点击主页「开始」即可恢复联网。"
                                ),
                                12.0,
                                p.text_secondary,
                            ))
                            .wrap(),
                        );

                        ui.add_space(4.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            if primary_button(ui, p, Some(Icon::Check), "暂停守护并注销")
                                .clicked()
                            {
                                confirm = true;
                            }
                            if secondary_button(ui, p, None, "取消").clicked() {
                                cancel = true;
                            }
                        });
                    });
            });

        if cancel {
            self.local_kick_confirm_ip = None;
        } else if confirm {
            self.local_kick_confirm_ip = None;
            self.kicking_ips.insert(ip.clone());
            self.show_toast(format!("正在注销本机连接（{ip}）并暂停守护…"), false);
            self.request(UiToDaemon::KickDevice { ip, is_local: true }, None);
        }
    }

    fn draw_update_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.update_modal_open {
            return;
        }
        let Some(rel) = self.update_state.release.clone() else {
            self.update_modal_open = false;
            return;
        };

        let busy = matches!(
            self.update_state.status,
            UpdateStatus::Downloading | UpdateStatus::Applying
        );
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("auto_update_modal_scrim"))
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
                let alpha = if p.mode == ThemeMode::Dark { 145 } else { 105 };
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::ZERO,
                    egui::Color32::from_black_alpha(alpha),
                );
            });

        let mut do_install = false;
        let mut do_close = false;
        let mut do_skip = false;
        let mut do_open_web = false;
        if !busy {
            ctx.input(|i| {
                if i.key_pressed(egui::Key::Escape) {
                    do_close = true;
                }
            });
        }

        egui::Area::new(egui::Id::new("auto_update_modal_dialog"))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                let card_w = 432.0_f32.min((screen.width() - 36.0).max(296.0));
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.2_f32, p.accent_border))
                    .corner_radius(CornerRadius::same(12))
                    .shadow(egui::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(95),
                    })
                    .inner_margin(egui::Margin::symmetric(18, 16))
                    .show(ui, |ui| {
                        ui.set_width(card_w);
                        ui.spacing_mut().item_spacing.y = 10.0;

                        // 标题栏
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 9.0;
                            let (badge_r, _) =
                                ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                            ui.painter().rect(
                                badge_r,
                                CornerRadius::same(8),
                                p.accent_soft,
                                Stroke::new(1.0_f32, p.accent_border),
                                StrokeKind::Inside,
                            );
                            icons::paint(
                                ui.painter(),
                                badge_r.shrink(6.0),
                                Icon::Download,
                                p.accent,
                            );
                            ui.label(bold_text(
                                format!("发现新版本 v{}", rel.version),
                                14.5,
                                p.text,
                            ));
                            badge(
                                ui,
                                p.text_muted,
                                p.subtle,
                                p.border,
                                &format!("当前 v{}", crate::VERSION),
                            );
                        });

                        // 更新说明框
                        let notes_txt = if rel.notes.trim().is_empty() {
                            "包含功能体验优化与稳定性修复，可前往 GitHub Release 查看完整变更。"
                        } else {
                            rel.notes.trim()
                        };
                        egui::Frame::new()
                            .fill(p.subtle)
                            .stroke(Stroke::new(1.0_f32, p.divider))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(12, 9))
                            .show(ui, |ui| {
                                ui.set_width(card_w - 24.0);
                                egui::ScrollArea::vertical()
                                    .id_salt("update_release_notes_scroll")
                                    .max_height(145.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        ui.set_width(card_w - 28.0);
                                        ui.add(
                                            egui::Label::new(body_text(
                                                notes_txt,
                                                11.5,
                                                p.text_secondary,
                                            ))
                                            .wrap(),
                                        );
                                    });
                            });

                        // 原位覆盖路径说明
                        let exe_hint = std::env::current_exe()
                            .ok()
                            .map(|path| format!("将原位覆盖更新：{}", path.display()))
                            .unwrap_or_else(|| {
                                "将原位覆盖当前程序并自动重启（保留全部账号与自启配置）".into()
                            });
                        ui.add(
                            egui::Label::new(body_text(exe_hint, 10.5, p.text_muted)).truncate(),
                        );

                        // 下载进度条 / 错误提示框
                        if busy {
                            let ratio = self.update_state.progress_ratio().unwrap_or(0.0);
                            let bar_w = card_w;
                            let (bar_r, _) =
                                ui.allocate_exact_size(Vec2::new(bar_w, 7.0), Sense::hover());
                            ui.painter()
                                .rect_filled(bar_r, CornerRadius::same(4), p.subtle);
                            let fill_w = (bar_w * ratio).max(if ratio > 0.0 { 8.0 } else { 0.0 });
                            if fill_w > 0.0 {
                                let fill_r = Rect::from_min_size(bar_r.min, Vec2::new(fill_w, 7.0));
                                ui.painter()
                                    .rect_filled(fill_r, CornerRadius::same(4), p.accent);
                            }
                            let dl_mb = self.update_state.downloaded_bytes as f64 / 1_048_576.0;
                            let prog_txt = if self.update_state.status == UpdateStatus::Applying {
                                "校验通过，正在原位替换并重启程序…".to_string()
                            } else if let Some(tot) =
                                self.update_state.total_bytes.filter(|&t| t > 0)
                            {
                                let tot_mb = tot as f64 / 1_048_576.0;
                                format!(
                                    "正在下载 {} · {:.2} / {:.2} MB ({}%)",
                                    rel.asset_name,
                                    dl_mb,
                                    tot_mb,
                                    (ratio * 100.0).round() as i32
                                )
                            } else {
                                format!("正在下载 {} · 已接收 {:.2} MB…", rel.asset_name, dl_mb)
                            };
                            ui.label(body_text(prog_txt, 11.0, p.accent));
                        } else if self.update_state.status == UpdateStatus::Error
                            && !self.update_state.message.is_empty()
                        {
                            egui::Frame::new()
                                .fill(p.danger_soft)
                                .stroke(Stroke::new(1.0_f32, p.danger_border))
                                .corner_radius(CornerRadius::same(7))
                                .inner_margin(egui::Margin::symmetric(10, 7))
                                .show(ui, |ui| {
                                    ui.set_width(card_w - 22.0);
                                    ui.add(
                                        egui::Label::new(body_text(
                                            &self.update_state.message,
                                            11.0,
                                            p.danger,
                                        ))
                                        .wrap(),
                                    );
                                });
                        }

                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if secondary_button(ui, p, Some(Icon::ExternalLink), "GitHub 页面")
                                .on_hover_text("在浏览器打开 GitHub Release 页面手动查看或下载")
                                .clicked()
                            {
                                do_open_web = true;
                            }
                            if !busy
                                && rel.version != self.settings.skipped_update_version
                                && secondary_button(ui, p, None, "跳过此版").clicked()
                            {
                                do_skip = true;
                            }

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    if busy {
                                        badge(
                                            ui,
                                            p.accent,
                                            p.accent_soft,
                                            p.accent_border,
                                            "更新执行中…",
                                        );
                                    } else {
                                        let btn_lbl =
                                            if self.update_state.status == UpdateStatus::Error {
                                                "重试自动更新"
                                            } else {
                                                "立即下载并更新"
                                            };
                                        if primary_button(ui, p, Some(Icon::Download), btn_lbl)
                                            .clicked()
                                        {
                                            do_install = true;
                                        }
                                        if secondary_button(ui, p, None, "稍后").clicked() {
                                            do_close = true;
                                        }
                                    }
                                },
                            );
                        });
                    });
            });

        if do_open_web {
            let url = if rel.html_url.is_empty() {
                crate::updater::GITHUB_RELEASES_PAGE
            } else {
                rel.html_url.as_str()
            };
            crate::updater::open_browser_url(url);
        }
        if do_skip {
            let mut next = self.settings.clone();
            next.skipped_update_version = rel.version.clone();
            if self.store.save_settings(&next).is_ok() {
                self.settings = next.clone();
                self.request(
                    UiToDaemon::ReloadSettings {
                        start: false,
                        password_changed: false,
                        expected: next,
                    },
                    None,
                );
                self.show_toast(format!("已跳过 v{} 的自动提醒", rel.version), false);
            }
            self.update_modal_open = false;
        } else if do_close {
            self.update_modal_open = false;
        } else if do_install {
            self.request(UiToDaemon::StartUpdate, None);
        }
    }

    fn draw_toast(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some((message, is_error, at)) = self.toast.as_ref() else {
            return;
        };
        let ttl = if *is_error {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(3)
        };
        if at.elapsed() > ttl {
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

fn draw_titlebar_update_pill(ui: &mut egui::Ui, p: &Palette, label: &str) -> egui::Response {
    let font = egui::FontId::new(10.8, theme::bold_family());
    let g = ui.painter().layout_no_wrap(label.to_string(), font, p.ok);
    let w = g.size().x + 28.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 23.0), Sense::click());
    let resp = resp
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("发现 GitHub Release 新版本，点击查看更新内容并一键覆盖安装");
    let (bg, bd, fg) = if resp.hovered() {
        (p.ok, Stroke::NONE, egui::Color32::WHITE)
    } else {
        (p.ok_soft, Stroke::new(1.0_f32, p.ok_border), p.ok)
    };
    ui.painter()
        .rect(rect, CornerRadius::same(11), bg, bd, StrokeKind::Inside);
    let ic_r = Rect::from_center_size(
        Pos2::new(rect.left() + 11.5, rect.center().y),
        Vec2::splat(11.0),
    );
    icons::paint(ui.painter(), ic_r, Icon::Download, fg);
    let g_draw = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::new(10.8, theme::bold_family()),
        fg,
    );
    ui.painter().galley(
        Pos2::new(rect.left() + 20.0, rect.center().y - g_draw.size().y * 0.5),
        g_draw,
        fg,
    );
    resp
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
            || !self.kicking_ips.is_empty()
            || self.preverify_job.is_some()
            || self.toast.is_some()
            || matches!(
                self.update_state.status,
                UpdateStatus::Checking | UpdateStatus::Downloading | UpdateStatus::Applying
            )
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

                        // Tier 1: 精简状态主条（含本机真实校园网 IP 与门户校准在线时长）
                        self.draw_status_ribbon(ui, &p);
                        ui.add_space(10.0);

                        let show_self_service = self.ui_prefs.show_home_traffic;
                        let campus_ip = self.effective_campus_ip();
                        let is_online = self.state() == Some(MonitorState::Online);
                        let login_rejected = self.latest.as_ref().and_then(|s| s.reason)
                            == Some(crate::models::DisconnectReason::LoginRejected);

                        // Tier 2: 账号流量 + 在线设备双卡（可左右拖拽分栏，上下可由底部分隔条调节高度）
                        self.recent_sessions_clicked = false;
                        let mut ss_h_opt: Option<(f32, f32, f32)> = None;
                        if show_self_service {
                            let avail_h = ui.available_height().max(340.0);
                            let stability_reserved = 78.0_f32;
                            let min_log = 140.0_f32;
                            let min_ss = 136.0_f32;
                            let default_ss = 140.0_f32;
                            let max_ss =
                                (avail_h - stability_reserved - min_log - 18.0).max(min_ss);
                            let ss_h = self
                                .ui_prefs
                                .home_mid_height
                                .unwrap_or(default_ss)
                                .clamp(min_ss, max_ss)
                                .floor();
                            ss_h_opt = Some((ss_h, min_ss, max_ss));

                            let actions = charts::draw_self_service_row(
                                ui,
                                &p,
                                &self.self_service,
                                &self.traffic,
                                &mut self.traffic_view_mode,
                                &campus_ip,
                                is_online,
                                login_rejected,
                                &self.kicking_ips,
                                self.recent_sessions_open,
                                &mut self.ui_prefs.home_traffic_split,
                                ss_h,
                            );
                            self.recent_sessions_anchor = actions.recent_sessions_anchor;
                            if actions.toggle_recent_sessions {
                                self.recent_sessions_clicked = true;
                                self.recent_sessions_open = !self.recent_sessions_open;
                            }
                            if actions.split_changed {
                                self.window_size_dirty_since = Some(Instant::now());
                            }
                            if actions.traffic_view_changed {
                                self.traffic_view_user_override = true;
                            }
                            if actions.refresh_self_service {
                                self.show_toast("正在刷新校园网自助服务数据…", false);
                                self.request(UiToDaemon::RefreshSelfService { force: true }, None);
                            }
                            if let Some((ip, is_local_dev)) = actions.kick_device {
                                if is_local_dev {
                                    self.local_kick_confirm_ip = Some(ip);
                                } else if self.kicking_ips.insert(ip.clone()) {
                                    self.show_toast(format!("正在请求注销 {ip}…"), false);
                                    self.request(
                                        UiToDaemon::KickDevice {
                                            ip,
                                            is_local: false,
                                        },
                                        None,
                                    );
                                }
                            }
                            ui.add_space(10.0);
                        }

                        // Tier 3: 通栏紧凑「连接稳定性」胶囊时间轴（位于流量/设备双卡之下、运行日志之上）
                        charts::draw_stability_strip(ui, &p, &self.telemetry, &mut self.time_range);

                        // 中下可自由拖拽的横向分隔条（双击恢复默认）
                        if let Some((ss_h, min_ss, max_ss)) = ss_h_opt {
                            let (dy, dbl) = horizontal_splitter(ui, &p, ui.available_width());
                            if dbl {
                                self.ui_prefs.home_mid_height = None;
                                self.window_size_dirty_since = Some(Instant::now());
                                self.show_toast("已恢复默认面板高度比例", false);
                            } else if dy.abs() > 0.01 {
                                let next_h = (ss_h + dy).clamp(min_ss, max_ss).round();
                                self.ui_prefs.home_mid_height = Some(next_h);
                                self.window_size_dirty_since = Some(Instant::now());
                            }
                        } else {
                            ui.add_space(10.0);
                        }

                        // Tier 4: 通栏静谧运行日志（严格取当前剩余可用高度，杜绝底部圆角被窗口边缘裁切）
                        let log_h = ui.available_height().floor().max(110.0);
                        logs_page::draw_card(
                            ui,
                            &p,
                            &self.log_lines,
                            &mut self.logs_state,
                            &self.log_path,
                            log_h,
                        );
                    });
                self.draw_account_switcher_popover(ctx, &p);
                self.draw_recent_sessions_popover(ctx, &p);
            }
            AppView::Settings => {
                let saving = self
                    .pending
                    .values()
                    .any(|p| matches!(p.command, UiToDaemon::ReloadSettings { .. }));
                let verifying_account = self.preverify_job.is_some();
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
                            &self.account_creds,
                            &self.update_state,
                            self.needs_setup,
                            saving,
                            verifying_account,
                            total_h,
                        );
                    });
                if self.form.carrier != 0 {
                    self.campus_vpn_ack_for_draft = false;
                }
                if actions.selected_campus_carrier {
                    self.campus_vpn_modal = Some(CampusVpnModal::CarrierSelected);
                }
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
                if actions.save_profile {
                    self.begin_save_account_profile(self.needs_setup, false, ctx);
                }
                if actions.force_save_profile {
                    self.begin_save_account_profile(self.needs_setup, true, ctx);
                }
                if let Some(id) = actions.switch_account {
                    self.switch_account_to(&id);
                }
                if let Some(id) = actions.delete_account {
                    self.delete_account_profile(&id);
                }
                if actions.save {
                    self.save(ctx);
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
                if actions.check_update {
                    self.pending_manual_update_check = true;
                    self.show_toast("正在连接 GitHub 检查新版本…", false);
                    self.request(UiToDaemon::CheckUpdate { manual: true }, None);
                }
                if actions.open_update_modal {
                    self.update_modal_open = true;
                }
            }
        }

        self.draw_campus_vpn_modal(ctx, &p);
        self.draw_local_kick_modal(ctx, &p);
        self.draw_update_modal(ctx, &p);
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

        #[cfg(debug_assertions)]
        if let Some(out_path) = self.qa_shot_out.clone() {
            let mut saved = false;
            ctx.input(|i| {
                for ev in &i.events {
                    if let egui::Event::Screenshot { image, .. } = ev {
                        let [w, h] = image.size;
                        let mut buf = Vec::with_capacity(8 + w * h * 4);
                        buf.extend_from_slice(&(w as u32).to_le_bytes());
                        buf.extend_from_slice(&(h as u32).to_le_bytes());
                        for c in &image.pixels {
                            buf.extend_from_slice(&c.to_array());
                        }
                        let _ = std::fs::write(&out_path, buf);
                        saved = true;
                        break;
                    }
                }
            });
            if saved {
                self.qa_shot_out = None;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            } else {
                self.qa_shot_frames += 1;
                if self.qa_shot_frames == 8 {
                    ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
                }
                ctx.request_repaint();
            }
        }
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

/// 顶部状态条内的一键切号胶囊（优先展示账号备注名，点击弹出切换浮层）。
fn draw_account_chip(
    ui: &mut egui::Ui,
    p: &Palette,
    display_name: &str,
    carrier: &str,
    has_account: bool,
    open: bool,
) -> (bool, Rect) {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        display_name,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::new(11.5, crate::ui::theme::bold_family()),
            color: if has_account { p.text } else { p.warn },
            ..Default::default()
        },
    );
    if has_account && display_name.trim() != carrier {
        job.append(
            &format!(" · {carrier}"),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(11.0),
                color: p.text_muted,
                ..Default::default()
            },
        );
    }
    let galley = ui.painter().layout_job(job);
    let w = 8.0 + 12.0 + 5.0 + galley.size().x + 5.0 + 10.0 + 7.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 23.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let (fill, stroke) = if open {
        (p.accent_soft, Stroke::new(1.1_f32, p.accent))
    } else if resp.hovered() {
        (p.card_hover, Stroke::new(1.0_f32, p.accent))
    } else {
        (p.subtle, Stroke::new(1.0_f32, p.border))
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        fill,
        stroke,
        StrokeKind::Inside,
    );
    let cy = rect.center().y;
    let user_r = Rect::from_center_size(Pos2::new(rect.left() + 13.0, cy), Vec2::splat(11.5));
    icons::paint(
        ui.painter(),
        user_r,
        Icon::User,
        if open {
            p.accent
        } else if has_account {
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
    resp.on_hover_text(if has_account {
        "点击一键切换校园网账号或进入账号管理"
    } else {
        "点击添加校园网账号与运营商出口"
    });
    (clicked, rect)
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

    #[test]
    fn preverify_blocks_definite_credential_errors_and_allows_environmental_limits() {
        use crate::self_service::PortalError;

        assert_eq!(
            classify_portal_auth_for_preverify(true, DisconnectReason::AuthExpired, "认证成功"),
            PreverifyOutcome::Verified
        );
        assert_eq!(
            classify_portal_auth_for_preverify(
                false,
                DisconnectReason::LoginRejected,
                "密码错误，请在设置中重新核对并保存密码"
            ),
            PreverifyOutcome::Rejected("密码错误，请在设置中重新核对并保存密码".into())
        );
        // 并发已满或网关超时不阻塞保存
        assert!(matches!(
            classify_portal_auth_for_preverify(
                false,
                DisconnectReason::LoginRejected,
                "在线设备数量已达上限（该账号正在其他终端登录）"
            ),
            PreverifyOutcome::SkippedWithNote(_)
        ));
        assert!(matches!(
            classify_portal_auth_for_preverify(
                false,
                DisconnectReason::PortalUnavailable,
                "认证服务器请求失败：Timeout"
            ),
            PreverifyOutcome::SkippedWithNote(_)
        ));

        // 自助服务：账密错误拦截，验证码/离校不可达/初始密码改密提示放行
        assert_eq!(
            classify_self_service_for_preverify(Ok(())),
            PreverifyOutcome::Verified
        );
        assert!(matches!(
            classify_self_service_for_preverify(Err(PortalError::AuthFailed(
                "自助服务登录失败：账号或密码错误".into()
            ))),
            PreverifyOutcome::Rejected(_)
        ));
        assert!(matches!(
            classify_self_service_for_preverify(Err(PortalError::CaptchaRequired)),
            PreverifyOutcome::SkippedWithNote(_)
        ));
        assert!(matches!(
            classify_self_service_for_preverify(Err(PortalError::Unreachable("连接超时".into()))),
            PreverifyOutcome::SkippedWithNote(_)
        ));
    }
}
