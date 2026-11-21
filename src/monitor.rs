//! 监控状态机。
//!
//! 行为契约保持一致：连续 confirm_failures 次失败才判离线；只有可认证的
//! 结论才提交凭据；认证首次被拒只静默重试，连续 2 次才上报失败。

mod transition;
use transition::NetworkTransition;

use std::hash::{BuildHasher, RandomState};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::authenticator::CampusAuthenticator;
use crate::credentials::StoredCredential;
use crate::diagnostics::{log_diagnostic, NetworkDiagnostics, RoutePath};
use crate::logging;
use crate::metrics_store::{MetricsStore, TelemetryPoint};
use crate::models::{
    DiagnosticPipeline, DiagnosticResult, DisconnectReason, MonitorMetrics, MonitorSnapshot,
    MonitorState, Settings, StepState,
};

pub const RETRY_DELAYS: [u64; 5] = [5, 15, 30, 60, 300];
const AUTH_FAILURES_BEFORE_ALERT: u32 = 2;
/// 认证通过后的快速两段复验等待（先等 1s 立即验证，若门户尚未放行完再隔 1.5s 复验一次）。
const VERIFY_DELAYS_MS: [u64; 2] = [1000, 1500];

pub type CredentialProvider = Arc<dyn Fn() -> Option<StoredCredential> + Send + Sync>;
pub type SnapshotSink = Arc<dyn Fn(MonitorSnapshot) + Send + Sync>;

pub(crate) trait NetworkCheck: Send + Sync {
    fn route(&self) -> RoutePath;
    fn check_on_route(&self, route: RoutePath) -> DiagnosticResult;
    fn portal_latency(&self) -> Option<u64>;
}

impl NetworkCheck for NetworkDiagnostics {
    fn route(&self) -> RoutePath {
        self.route()
    }
    fn check_on_route(&self, route: RoutePath) -> DiagnosticResult {
        self.check_on_route(route)
    }
    fn portal_latency(&self) -> Option<u64> {
        self.measure_portal_latency_ms()
    }
}

#[derive(Default)]
struct ControlState {
    enabled: bool,
    stopped: bool,
    manual: bool,
    wake: bool,
    generation: u64,
    session: u64,
    event_pending: bool,
    transition: Option<NetworkTransition>,
    last_route: Option<RoutePath>,
    latest: Option<MonitorSnapshot>,
    last_state: Option<MonitorState>,
    metrics: MonitorMetrics,
    pending_latency: Option<TelemetryPoint>,
}

impl ControlState {
    /// Admit only a fresh check started on the settled route. A long-running
    /// provisional scan cannot certify itself just because its deadline elapsed.
    fn sample_kind(
        &mut self,
        cycle: Cycle,
        now: Instant,
        result: &DiagnosticResult,
        confirmed: bool,
    ) -> SampleKind {
        if let Some(t) = &mut self.transition {
            if result.online {
                if cycle.started >= t.quiet_at() {
                    self.transition = None;
                }
            } else {
                let unknown_auth =
                    result.can_authenticate && evidence_value(result, "campus_status").is_none();
                if t.defer_failure(cycle.started, now, unknown_auth) || !cycle.enabled {
                    return SampleKind::Switching;
                }
                self.transition = None;
            }
        }
        if confirmed {
            SampleKind::ConfirmedOffline
        } else {
            SampleKind::Check
        }
    }
}

#[derive(Copy, Clone)]
struct Cycle {
    session: u64,
    switching: bool,
    started: Instant,
    generation: u64,
    manual: bool,
    enabled: bool,
}

struct Shared {
    diagnostics: Box<dyn NetworkCheck>,
    authenticator: CampusAuthenticator,
    credentials: CredentialProvider,
    metrics_store: Arc<MetricsStore>,
    settings: Mutex<Settings>,
    sink: SnapshotSink,
    control: Mutex<ControlState>,
    wake_cv: Condvar,
    latency_jobs: mpsc::SyncSender<(u64, u64)>,
    sample_prefix: String,
    now: Box<dyn Fn() -> Instant + Send + Sync>,
}

impl Shared {
    fn next_cycle(&self) -> Option<Cycle> {
        let mut state = self.control.lock().unwrap();
        loop {
            if state.stopped {
                return None;
            }
            if !state.enabled && !state.manual && !state.event_pending {
                state = self.wake_cv.wait(state).unwrap();
                continue;
            }
            if !state.manual {
                if let Some(t) = &state.transition {
                    let delay = t.next_check.saturating_duration_since((self.now)());
                    if !delay.is_zero() {
                        state = self.wake_cv.wait_timeout(state, delay).unwrap().0;
                        continue;
                    }
                }
            }
            break;
        }
        let started = (self.now)();
        if state
            .transition
            .as_ref()
            .is_none_or(|t| started >= t.quiet_at())
        {
            state.event_pending = false;
        }
        self.flush_latency(&mut state);
        state.wake = false;
        Some(Cycle {
            session: state.session,
            generation: state.generation,
            enabled: state.enabled,
            manual: std::mem::take(&mut state.manual),
            switching: state.transition.is_some(),
            started,
        })
    }

    fn mark_network_change(&self, mode: &mut ControlState, different_route: bool) {
        let now = (self.now)();
        let first = mode.transition.is_none();
        if let Some(t) = &mut mode.transition {
            if !t.changed(now, different_route) {
                return;
            }
        } else {
            mode.transition = Some(NetworkTransition::new(now));
        }
        self.flush_latency(mode);
        mode.generation += 1;
        mode.event_pending = true;
        mode.wake = true;
        if first || different_route {
            let cause = if different_route {
                "出网路由改变"
            } else {
                "接口变化或系统唤醒"
            };
            logging::info(&format!("[NETWORK_CHANGE] {cause}，开始只读复查"));
        }
        if first && mode.enabled {
            self.publish_locked(
                mode,
                MonitorState::Checking,
                None,
                "网络切换中，正在复查连接",
                None,
            );
        }
        self.wake_cv.notify_all();
    }

    fn observe_route(&self, cycle: Cycle, route: RoutePath) -> bool {
        let mut mode = self.control.lock().unwrap();
        if mode.stopped || mode.generation != cycle.generation {
            return false;
        }
        if mode
            .last_route
            .replace(route)
            .is_some_and(|old| old != route)
        {
            self.mark_network_change(&mut mode, true);
            return false;
        }
        true
    }

    fn route_current(&self, cycle: Cycle) -> bool {
        self.observe_route(cycle, self.diagnostics.route())
    }

    fn checked_scan(&self, cycle: Cycle) -> Option<DiagnosticResult> {
        let route = self.diagnostics.route();
        if !self.observe_route(cycle, route) {
            return None;
        }
        let result = safe_check(self, route);
        self.route_current(cycle).then_some(result)
    }

    fn current(&self, cycle: Cycle) -> bool {
        let state = self.control.lock().unwrap();
        !state.stopped && state.generation == cycle.generation
    }

    fn wait_duration(&self, duration: Duration) {
        let state = self.control.lock().unwrap();
        let _ = self
            .wake_cv
            .wait_timeout_while(state, duration, |s| {
                !s.wake && !s.stopped && s.transition.is_none()
            })
            .unwrap();
    }

    fn wait(&self, seconds: u64) {
        self.wait_duration(Duration::from_secs(seconds));
    }

    fn publish(
        &self,
        generation: u64,
        state: MonitorState,
        reason: Option<DisconnectReason>,
        message: &str,
    ) {
        self.publish_retry(generation, state, reason, message, None);
    }

    fn publish_retry(
        &self,
        generation: u64,
        state: MonitorState,
        reason: Option<DisconnectReason>,
        message: &str,
        retry: Option<Duration>,
    ) {
        let mut mode = self.control.lock().unwrap();
        if mode.generation != generation || (mode.stopped && state != MonitorState::Stopped) {
            return;
        }
        self.publish_locked(&mut mode, state, reason, message, retry);
    }

    fn publish_locked(
        &self,
        mode: &mut ControlState,
        state: MonitorState,
        reason: Option<DisconnectReason>,
        message: &str,
        retry: Option<Duration>,
    ) {
        if state != MonitorState::Checking && Some(state) != mode.last_state {
            let previous = mode
                .last_state
                .map(MonitorState::as_str)
                .unwrap_or("INITIAL");
            let reason_text = reason
                .map(|value| format!("；原因={}", value.as_str()))
                .unwrap_or_default();
            logging::info(&format!(
                "[STATE_CHANGE] {} → {}；{}{}",
                previous,
                state.as_str(),
                message,
                reason_text
            ));
            mode.last_state = Some(state);
        }
        let mut snapshot = MonitorSnapshot::new(state, reason, message, mode.enabled)
            .with_metrics(mode.metrics.clone());
        snapshot.retry_after_secs = retry.map(|d| d.as_secs() + u64::from(d.subsec_nanos() > 0));
        mode.latest = Some(snapshot.clone());
        (self.sink)(snapshot);
    }
}

pub struct NetworkMonitor {
    shared: Arc<Shared>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// 可跨线程共享的轻量控制句柄（供托盘后台监听线程直接操作，不依赖 UI 渲染帧）。
#[derive(Clone)]
pub struct MonitorControl {
    shared: Arc<Shared>,
}

impl MonitorControl {
    pub fn start(&self) -> Result<(), String> {
        let mut mode = self.shared.control.lock().unwrap();
        ensure_running(&mode)?;
        if !mode.enabled {
            self.shared.flush_latency(&mut mode);
            mode.generation += 1;
            mode.session += 1;
            mode.enabled = true;
            self.shared.publish_locked(
                &mut mode,
                MonitorState::Checking,
                None,
                "正在检查网络",
                None,
            );
            mode.wake = true;
            self.shared.wake_cv.notify_all();
        }
        Ok(())
    }

    pub fn pause(&self) -> Result<(), String> {
        let mut mode = self.shared.control.lock().unwrap();
        ensure_running(&mode)?;
        self.shared.flush_latency(&mut mode);
        mode.event_pending = false;
        mode.generation += 1;
        mode.manual = false;
        mode.enabled = false;
        mode.wake = true;
        self.shared
            .publish_locked(&mut mode, MonitorState::Paused, None, "监控已暂停", None);
        self.shared.wake_cv.notify_all();
        Ok(())
    }

    pub fn network_changed(&self) {
        let mut mode = self.shared.control.lock().unwrap();
        if !mode.stopped {
            self.shared.mark_network_change(&mut mode, false);
        }
    }

    pub fn force_check(&self) -> Result<(), String> {
        let mut mode = self.shared.control.lock().unwrap();
        ensure_running(&mode)?;
        mode.manual = true;
        mode.wake = true;
        self.shared.wake_cv.notify_all();
        Ok(())
    }

    pub fn signal_stop(&self) -> u64 {
        let mut mode = self.shared.control.lock().unwrap();
        if !mode.stopped {
            self.shared.flush_latency(&mut mode);
            mode.stopped = true;
            mode.enabled = false;
            mode.manual = false;
            mode.generation += 1;
            self.shared.wake_cv.notify_all();
        }
        mode.generation
    }

    pub fn update_settings(
        &self,
        settings: Settings,
        password_changed: bool,
    ) -> Result<(), String> {
        let mut mode = self.shared.control.lock().unwrap();
        ensure_running(&mode)?;
        let settings = settings.normalized();
        let mut current = self.shared.settings.lock().unwrap();
        let rescan = password_changed
            || current.student_id != settings.student_id
            || current.carrier != settings.carrier
            || current.check_interval != settings.check_interval
            || current.offline_interval != settings.offline_interval
            || current.confirm_failures != settings.confirm_failures;
        *current = settings;
        if !rescan {
            return Ok(());
        }
        self.shared.flush_latency(&mut mode);
        mode.generation += 1;
        mode.wake = true;
        self.shared.wake_cv.notify_all();
        Ok(())
    }
}

fn ensure_running(mode: &ControlState) -> Result<(), String> {
    if mode.stopped {
        Err("守护进程正在退出".into())
    } else {
        Ok(())
    }
}

impl NetworkMonitor {
    pub fn new(
        diagnostics: impl NetworkCheck + 'static,
        authenticator: CampusAuthenticator,
        credentials: CredentialProvider,
        metrics_store: Arc<MetricsStore>,
        settings: Settings,
        sink: SnapshotSink,
    ) -> Self {
        Self::with_clock(
            diagnostics,
            authenticator,
            credentials,
            metrics_store,
            settings,
            sink,
            Instant::now,
        )
    }

    fn with_clock(
        diagnostics: impl NetworkCheck + 'static,
        authenticator: CampusAuthenticator,
        credentials: CredentialProvider,
        metrics_store: Arc<MetricsStore>,
        settings: Settings,
        sink: SnapshotSink,
        now: impl Fn() -> Instant + Send + Sync + 'static,
    ) -> Self {
        let (latency_jobs, jobs) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            now: Box::new(now),
            latency_jobs,
            sample_prefix: format!(
                "{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ),
            diagnostics: Box::new(diagnostics),
            authenticator,
            credentials,
            metrics_store,
            settings: Mutex::new(settings.normalized()),
            sink,
            control: Mutex::new(ControlState::default()),
            wake_cv: Condvar::new(),
        });
        let weak = Arc::downgrade(&shared);
        std::thread::Builder::new()
            .name("portal-latency".into())
            .spawn(move || {
                while let Ok((generation, seq)) = jobs.recv() {
                    let Some(shared) = weak.upgrade() else { break };
                    if shared.is_pending(generation, seq) {
                        let ms = shared.diagnostics.portal_latency();
                        shared.finish_latency(generation, seq, ms);
                    }
                }
            })
            .expect("无法创建门户延迟采样线程");
        let monitor = Self {
            shared: Arc::clone(&shared),
            handle: Mutex::new(None),
        };
        let thread_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("network-monitor".to_string())
            .spawn(move || run(thread_shared))
            .expect("无法创建监控线程");
        *monitor.handle.lock().unwrap() = Some(handle);
        monitor
            .shared
            .publish(0, MonitorState::Paused, None, "监控已暂停");
        monitor
    }

    pub fn control(&self) -> MonitorControl {
        MonitorControl {
            shared: Arc::clone(&self.shared),
        }
    }

    pub fn shutdown(&mut self) {
        let generation = self.control().signal_stop();
        if let Ok(mut guard) = self.handle.lock() {
            if let Some(handle) = guard.take() {
                // 避免网络探测/HTTP 请求阻塞退出流程：最多等待 800ms。
                let deadline = Instant::now() + Duration::from_millis(800);
                while !handle.is_finished() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                if handle.is_finished() {
                    let _ = handle.join();
                }
            }
        }
        self.shared
            .publish(generation, MonitorState::Stopped, None, "监控已停止");
    }
}

fn safe_check(shared: &Shared, route: RoutePath) -> DiagnosticResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        shared.diagnostics.check_on_route(route)
    })) {
        Ok(result) => result,
        Err(_) => {
            logging::error("[DIAGNOSTIC_ERROR] 网络诊断发生异常");
            DiagnosticResult::new(false, DisconnectReason::InternetTimeout, "诊断异常")
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SampleKind {
    Check,
    ConfirmedOffline,
    Switching,
}

impl Shared {
    // Only one check can own the pending disk entry. A newer cycle or a mode
    // change finalizes it as missing before any later sample can be appended.
    fn flush_latency(&self, mode: &mut ControlState) {
        if let Some(point) = mode.pending_latency.take() {
            self.metrics_store.append(&point);
        }
    }

    fn is_pending(&self, generation: u64, seq: u64) -> bool {
        let mode = self.control.lock().unwrap();
        mode.generation == generation
            && mode.pending_latency.is_some()
            && mode.metrics.check_seq == seq
    }

    fn measure_latency(&self, cycle: Cycle, seq: u64) {
        if self.latency_jobs.try_send((cycle.generation, seq)).is_err() {
            self.finish_latency(cycle.generation, seq, None);
        }
    }

    fn finish_latency(&self, generation: u64, seq: u64, ms: Option<u64>) {
        let mut mode = self.control.lock().unwrap();
        if mode.generation != generation || mode.metrics.check_seq != seq {
            return;
        }
        let Some(mut point) = mode.pending_latency.take() else {
            return;
        };
        point.ms = ms.unwrap_or(0);
        self.metrics_store.append(&point);
        if point.ms > 0 {
            mode.metrics.last_check_ms = point.ms;
            mode.metrics.last_point = Some(point);
            let metrics = mode.metrics.clone();
            if let Some(snapshot) = mode.latest.as_mut() {
                snapshot.metrics = metrics;
                (self.sink)(snapshot.clone());
            }
        }
    }

    fn record_metrics(
        &self,
        cycle: Cycle,
        result: &DiagnosticResult,
        confirmed: bool,
    ) -> Option<(u64, SampleKind)> {
        let mut mode = self.control.lock().unwrap();
        if mode.stopped || mode.generation != cycle.generation {
            return None;
        }
        let kind = mode.sample_kind(cycle, (self.now)(), result, confirmed);
        self.flush_latency(&mut mode);
        let now = chrono::Local::now();
        let metrics = &mut mode.metrics;
        metrics.check_seq += 1;
        let seq = metrics.check_seq;
        metrics.local_ip = result.local_ip.clone();
        metrics.gateway = result.gateway.clone();
        metrics.last_check_ms = 0;
        metrics.last_check_text = now.format("%H:%M:%S").to_string();
        metrics.pipeline = build_pipeline(result);
        if kind == SampleKind::Switching {
            metrics.last_point = None;
            return Some((seq, kind));
        }
        let mut point =
            TelemetryPoint::check(now.timestamp(), 0, result.online, result.reason.as_str());
        point.sample_id = Some(format!("{}-{seq}", self.sample_prefix));
        point.confirmed_offline = Some(kind == SampleKind::ConfirmedOffline);
        metrics.last_point = Some(point.clone());
        if result.online {
            mode.pending_latency = Some(point);
        } else {
            self.metrics_store.append(&point);
        }
        Some((seq, kind))
    }
}

fn record_auth_event(shared: &Shared, success: bool, reason: DisconnectReason) {
    let point =
        TelemetryPoint::auth_event(chrono::Local::now().timestamp(), success, reason.as_str());
    shared.metrics_store.append(&point);
}

fn evidence_value<'a>(result: &'a DiagnosticResult, key: &str) -> Option<&'a str> {
    result
        .evidence
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn shorten_host(host: &str) -> String {
    let trimmed = host.trim().trim_start_matches("www.");
    if trimmed.is_empty() {
        "公网可达".to_string()
    } else if trimmed.len() > 18 {
        format!("{}…", &trimmed[..17])
    } else {
        trimmed.to_string()
    }
}

fn build_pipeline(result: &DiagnosticResult) -> DiagnosticPipeline {
    let first_adapter = evidence_value(result, "adapters")
        .and_then(|raw| raw.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "none")
        .unwrap_or("网络适配器");
    let adapter = if result.reason == DisconnectReason::LinkDown {
        (StepState::Fail, "链路断开".to_string())
    } else {
        let label = if first_adapter.chars().count() > 12 {
            let short: String = first_adapter.chars().take(11).collect();
            format!("{short}…")
        } else {
            first_adapter.to_string()
        };
        (StepState::Ok, label)
    };

    let ip_gateway = match result.reason {
        DisconnectReason::LinkDown => (StepState::Skipped, "未执行".to_string()),
        DisconnectReason::NoIp => (StepState::Fail, "未获取 IPv4".to_string()),
        DisconnectReason::NoGateway => (StepState::Fail, "无默认网关".to_string()),
        DisconnectReason::GatewayUnreachable => (StepState::Fail, "网关无响应".to_string()),
        _ => {
            let label = if !result.local_ip.is_empty() {
                result.local_ip.clone()
            } else {
                "路由正常".to_string()
            };
            (StepState::Ok, label)
        }
    };

    let internet = match result.reason {
        DisconnectReason::LinkDown
        | DisconnectReason::NoIp
        | DisconnectReason::NoGateway
        | DisconnectReason::GatewayUnreachable => (StepState::Skipped, "未执行".to_string()),
        DisconnectReason::Online => {
            let host = evidence_value(result, "probe").unwrap_or("");
            (StepState::Ok, shorten_host(host))
        }
        DisconnectReason::DnsFailure => (StepState::Fail, "DNS 解析失败".to_string()),
        DisconnectReason::AuthExpired => (StepState::Warn, "被重定向至门户".to_string()),
        DisconnectReason::UplinkUnreachable => (StepState::Fail, "上游出口不可达".to_string()),
        DisconnectReason::InternetTimeout => (StepState::Fail, "公网探测超时".to_string()),
        _ => (StepState::Warn, result.reason.label().to_string()),
    };

    let portal = match evidence_value(result, "campus_status") {
        Some("online") => (StepState::Ok, "门户在线".to_string()),
        Some("offline") => (StepState::Fail, "会话已下线".to_string()),
        _ => match result.reason {
            DisconnectReason::Online => (StepState::Ok, "会话有效".to_string()),
            DisconnectReason::AuthExpired => (StepState::Fail, "需要重新认证".to_string()),
            DisconnectReason::LoginRejected => (StepState::Fail, "认证被拒绝".to_string()),
            DisconnectReason::PortalUnavailable => (StepState::Fail, "门户不可用".to_string()),
            DisconnectReason::CredentialsMissing => (StepState::Warn, "未保存凭据".to_string()),
            _ => (StepState::Skipped, "无需查询".to_string()),
        },
    };

    DiagnosticPipeline {
        adapter,
        ip_gateway,
        internet,
        portal,
    }
}

#[derive(Default)]
struct AuthBackoff {
    identity: Option<u64>,
    deadline: Option<Instant>,
    retry_index: usize,
    refusals: u32,
    reported: Option<DisconnectReason>,
}

impl AuthBackoff {
    fn select(&mut self, identity: u64) {
        if self.identity != Some(identity) {
            *self = Self {
                identity: Some(identity),
                ..Self::default()
            };
        }
    }

    fn remaining(&self, now: Instant) -> Option<Duration> {
        self.deadline?
            .checked_duration_since(now)
            .filter(|d| !d.is_zero())
    }

    fn after_response(
        &mut self,
        auth: &crate::authenticator::AuthenticationResult,
        now: Instant,
    ) -> Duration {
        let rejected = !auth.success && auth.reason == DisconnectReason::LoginRejected;
        self.refusals = if rejected {
            self.refusals.saturating_add(1)
        } else {
            0
        };
        let silent = rejected && self.refusals < AUTH_FAILURES_BEFORE_ALERT;
        self.reported = (!auth.success && !silent).then_some(auth.reason);
        let delay = Duration::from_secs(if silent {
            RETRY_DELAYS[0]
        } else {
            RETRY_DELAYS[self.retry_index]
        });
        self.retry_index = (self.retry_index + 1).min(RETRY_DELAYS.len() - 1);
        self.deadline = Some(now + delay);
        delay
    }

    fn recovered(&mut self) {
        self.retry_index = 0;
        self.refusals = 0;
        self.reported = None;
    }
}

fn run(shared: Arc<Shared>) {
    let mut failures: u32 = 0;
    let mut backoff = AuthBackoff::default();
    // Compare effective credentials, including same-account password changes,
    // without retaining a second plaintext password or persisting an identifier.
    let identity_key = RandomState::new();
    let mut last_heartbeat: Option<Instant> = None;
    let mut session = 0;
    while let Some(cycle) = shared.next_cycle() {
        if session != cycle.session {
            failures = 0;
            last_heartbeat = None;
            session = cycle.session;
        }
        if cycle.switching {
            failures = 0;
        }
        let was_manual = cycle.manual;
        let publish =
            |state, reason, message: &str| shared.publish(cycle.generation, state, reason, message);
        if was_manual {
            publish(MonitorState::Checking, None, "正在执行手动检测…");
        }

        let settings = shared
            .settings
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let started = Instant::now();
        let Some(result) = shared.checked_scan(cycle) else {
            continue;
        };
        let check_duration = started.elapsed();
        let confirmed =
            cycle.enabled && !result.online && i64::from(failures) + 1 >= settings.confirm_failures;
        let Some((seq, kind)) = shared.record_metrics(cycle, &result, confirmed) else {
            continue;
        };
        if !shared.current(cycle) {
            continue;
        }
        if kind == SampleKind::Switching {
            logging::info(&format!(
                "[NETWORK_SWITCH] 只读复查：{}，{}",
                result.reason.as_str(),
                result.evidence_text()
            ));
            if cycle.enabled {
                publish(MonitorState::Checking, None, "网络切换中，等待再次确认连接");
            } else {
                publish(MonitorState::Paused, None, "监控已暂停，网络切换中");
            }
            continue;
        }
        if !cycle.enabled {
            logging::info(&format!(
                "[FORCE_CHECK_RESULT] {}（监控已暂停）",
                result.message
            ));
            publish(
                MonitorState::Paused,
                Some(result.reason),
                &format!("监控已暂停，本次检测：{}", result.message),
            );
            if result.online {
                shared.measure_latency(cycle, seq);
            }
            continue;
        }

        if result.online {
            if failures > 0 {
                logging::info(&format!(
                    "[NETWORK_RECOVERED] 网络已恢复，IP={}",
                    result.local_ip
                ));
            }
            failures = 0;
            backoff.recovered();
            publish(
                MonitorState::Online,
                Some(DisconnectReason::Online),
                &result.message,
            );
            shared.measure_latency(cycle, seq);
            if was_manual {
                logging::info(&format!(
                    "[FORCE_CHECK_RESULT] 校园网正常；IP={}",
                    result.local_ip
                ));
            }
            let heartbeat_due = last_heartbeat
                .map(|at| at.elapsed() >= Duration::from_secs(settings.heartbeat_interval as u64))
                .unwrap_or(true);
            if heartbeat_due {
                logging::info(&format!(
                    "[HEALTH_CHECK] 网络正常；IP={}；网关={}；检测耗时={:.2}s；心跳间隔={}分钟",
                    if result.local_ip.is_empty() {
                        "unknown"
                    } else {
                        &result.local_ip
                    },
                    if result.gateway.is_empty() {
                        "unknown"
                    } else {
                        &result.gateway
                    },
                    check_duration.as_secs_f64(),
                    (settings.heartbeat_interval / 60).max(1)
                ));
                last_heartbeat = Some(Instant::now());
            }
            shared.wait(settings.check_interval as u64);
            continue;
        }

        failures = failures.saturating_add(1);
        log_diagnostic(&result, failures);
        if was_manual {
            logging::warn(&format!(
                "[FORCE_CHECK_RESULT] 检测异常：{}（{}）",
                result.reason.label(),
                result.message
            ));
        }
        if i64::from(failures) < settings.confirm_failures {
            publish(
                MonitorState::Suspect,
                Some(result.reason),
                &format!("检测到异常，正在复核：{}", result.message),
            );
            shared.wait(3);
            continue;
        }

        if !result.can_authenticate {
            publish(MonitorState::Offline, Some(result.reason), &result.message);
            shared.wait(settings.offline_interval as u64);
            continue;
        }

        let cooling = |backoff: &AuthBackoff, remaining: Duration| {
            let reason = backoff.reported.unwrap_or(result.reason);
            let secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
            shared.publish_retry(
                cycle.generation,
                MonitorState::Offline,
                Some(reason),
                &format!("{}，距下次自动登录还有 {secs} 秒", reason.label()),
                Some(remaining),
            );
            shared.wait_duration(remaining);
        };
        let credential = (shared.credentials)();
        if !shared.route_current(cycle) {
            continue;
        }
        let Some(credential) = credential.filter(|cred| cred.matches_account(&settings.student_id))
        else {
            logging::warn("[CREDENTIALS_MISSING] 请保存当前学号对应的凭据");
            publish(
                MonitorState::Offline,
                Some(DisconnectReason::CredentialsMissing),
                "请打开设置并保存当前学号的密码",
            );
            shared.wait(60);
            continue;
        };

        backoff.select(identity_key.hash_one((
            &credential.username,
            &credential.password,
            &settings.carrier,
        )));
        if let Some(remaining) = backoff.remaining((shared.now)()) {
            cooling(&backoff, remaining);
            continue;
        }
        if backoff.retry_index == 0 {
            publish(
                MonitorState::Offline,
                Some(result.reason),
                "准备自动登录校园网",
            );
        }
        publish(
            MonitorState::Authenticating,
            Some(result.reason),
            "正在进行校园网认证",
        );
        let Some(auth) = shared.authenticator.authenticate_if(
            &credential.username,
            &credential.password,
            &settings.carrier,
            result.auth_ip(),
            || shared.route_current(cycle) && backoff.remaining((shared.now)()).is_none(),
        ) else {
            continue;
        };
        // A request already sent still counts if pause or reload raced its response.
        let previous_refusals = backoff.refusals;
        let delay = backoff.after_response(&auth, (shared.now)());
        if !shared.route_current(cycle) {
            continue;
        }

        if auth.success {
            if previous_refusals > 0 {
                logging::info(&format!(
                    "[AUTH_RETRY_OK] 认证被接受（此前连续被拒 {} 次）",
                    previous_refusals
                ));
            }
            logging::info(&format!(
                "[LOGIN_ACCEPTED] {}；门户返回码={}",
                auth.message, auth.code
            ));
            publish(
                MonitorState::Verifying,
                Some(result.reason),
                "认证响应成功，正在验证网络",
            );
            let mut last_verified: Option<DiagnosticResult> = None;
            let mut aborted = false;
            for &delay_ms in &VERIFY_DELAYS_MS {
                shared.wait_duration(Duration::from_millis(delay_ms));
                if !shared.current(cycle) {
                    aborted = true;
                    break;
                }
                let Some(verified) = shared.checked_scan(cycle) else {
                    aborted = true;
                    break;
                };
                if verified.online {
                    last_verified = Some(verified);
                    break;
                }
                last_verified = Some(verified);
            }
            if aborted {
                continue;
            }
            if let Some(verified) = last_verified {
                if verified.online {
                    failures = 0;
                    let Some((seq, _)) = shared.record_metrics(cycle, &verified, false) else {
                        continue;
                    };
                    backoff = AuthBackoff::default();
                    record_auth_event(&shared, true, DisconnectReason::Online);
                    logging::info("[LOGIN_VERIFIED] 校园网登录成功，公网已恢复");
                    publish(
                        MonitorState::Online,
                        Some(DisconnectReason::Online),
                        "校园网登录成功，公网已恢复",
                    );
                    shared.measure_latency(cycle, seq);
                    shared.wait(settings.check_interval as u64);
                    continue;
                }
                logging::warn(&format!(
                    "[LOGIN_NOT_VERIFIED] 认证后仍离线：{} {}",
                    verified.reason.as_str(),
                    verified.message
                ));
            }
        } else {
            let detail = format!(
                "{}；门户返回码={}",
                auth.message,
                if auth.code.is_empty() {
                    "-"
                } else {
                    &auth.code
                }
            );
            if backoff.reported.is_none() {
                logging::info(&format!("[AUTH_RETRY] 首次拒绝，稍后复核；{detail}"));
            } else {
                record_auth_event(&shared, false, auth.reason);
                logging::warn(&format!("[{}] {detail}", auth.reason.as_str()));
            }
        }

        backoff.deadline = Some((shared.now)() + delay);
        let reason = backoff.reported.unwrap_or(result.reason);
        let message = if backoff.reported.is_some() {
            &auth.message
        } else {
            &result.message
        };
        shared.publish_retry(
            cycle.generation,
            MonitorState::Offline,
            Some(reason),
            message,
            Some(delay),
        );
        logging::info(&format!("[RETRY_BACKOFF] {} 秒后再次检测", delay.as_secs()));
        shared.wait_duration(delay);
    }
}

#[cfg(test)]
mod tests {
    mod switching;

    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};

    fn fixture_route() -> RoutePath {
        RoutePath::Ipv4 {
            interface: 1,
            source: [192, 0, 2, 2].into(),
            next_hop: [192, 0, 2, 1].into(),
        }
    }

    struct FixedCheck(bool);
    impl NetworkCheck for FixedCheck {
        fn route(&self) -> RoutePath {
            fixture_route()
        }
        fn check_on_route(&self, _route: RoutePath) -> DiagnosticResult {
            if self.0 {
                // A missing RTT must stay missing even when the scan took measurable time.
                std::thread::sleep(Duration::from_millis(15));
                DiagnosticResult::new(true, DisconnectReason::Online, "online")
            } else {
                DiagnosticResult::new(false, DisconnectReason::AuthExpired, "offline")
                    .authenticatable(true)
            }
        }
        fn portal_latency(&self) -> Option<u64> {
            None
        }
    }

    fn fake_credential() -> StoredCredential {
        StoredCredential {
            username: "fixture-a".into(),
            password: "dummy-password".into(),
        }
    }

    struct Fixture {
        monitor: NetworkMonitor,
        snapshots: Receiver<MonitorSnapshot>,
        portal: TcpListener,
        notices: Arc<Mutex<Vec<String>>>,
        elapsed: Arc<std::sync::atomic::AtomicU64>,
        _dir: tempfile::TempDir,
    }
    impl Fixture {
        fn new(online: bool, account: &str, credentials: CredentialProvider) -> Self {
            Self::with_check(FixedCheck(online), account, credentials)
        }

        fn with_check(
            check: impl NetworkCheck + 'static,
            account: &str,
            credentials: CredentialProvider,
        ) -> Self {
            let portal = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let auth = CampusAuthenticator::with_url(format!(
                "http://{}/login",
                portal.local_addr().unwrap()
            ));
            let (tx, snapshots) = mpsc::channel();
            let notices = Arc::new(Mutex::new(Vec::new()));
            let sent = Arc::clone(&notices);
            let notifier = Mutex::new(crate::notify::ToastNotifier::new(true));
            let dir = tempfile::tempdir().unwrap();
            let zero = Instant::now();
            let elapsed = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let clock = Arc::clone(&elapsed);
            let monitor = NetworkMonitor::with_clock(
                check,
                auth,
                credentials,
                Arc::new(MetricsStore::new(dir.path())),
                Settings {
                    confirm_failures: 1,
                    student_id: account.into(),
                    ..Settings::default()
                },
                Arc::new(move |snapshot| {
                    if let Some(body) = notifier.lock().unwrap().notification_for(&snapshot) {
                        sent.lock().unwrap().push(body);
                    }
                    let _ = tx.send(snapshot);
                }),
                move || {
                    zero + Duration::from_secs(clock.load(std::sync::atomic::Ordering::Acquire))
                },
            );
            Self {
                monitor,
                snapshots,
                portal,
                _dir: dir,
                elapsed,
                notices,
            }
        }

        fn until(&self, ready: impl Fn(&MonitorSnapshot) -> bool) -> MonitorSnapshot {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let s = self
                    .snapshots
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap();
                if ready(&s) {
                    return s;
                }
            }
        }

        fn advance(&self, seconds: u64) {
            let _mode = self.monitor.shared.control.lock().unwrap();
            self.elapsed
                .fetch_add(seconds, std::sync::atomic::Ordering::Release);
            self.monitor.shared.wake_cv.notify_all();
        }

        fn assert_no_submission(&self) {
            self.portal.set_nonblocking(true).unwrap();
            assert!(
                matches!(self.portal.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
            );
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.monitor.shutdown();
        }
    }

    #[test]
    fn pause_during_credential_read_cancels_the_pending_request() {
        let (entered, wait_entered) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let resumed = Mutex::new(resumed);
        let f = Fixture::new(
            false,
            "fixture-a",
            Arc::new(move || {
                entered.send(()).unwrap();
                resumed
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
                Some(fake_credential())
            }),
        );
        f.monitor.control().start().unwrap();
        wait_entered.recv_timeout(Duration::from_secs(3)).unwrap();
        let control = f.monitor.control();
        control.pause().unwrap();
        resume.send(()).unwrap();
        // Finishing a subsequent read-only cycle proves the cancelled cycle has completed.
        control.force_check().unwrap();
        let last = f.until(|s| s.state == MonitorState::Paused && s.reason.is_some());
        assert!(!last.monitoring_enabled);
        f.assert_no_submission();
    }

    #[test]
    fn exiting_the_tray_cancels_pending_auth_before_ui_cleanup() {
        let (entered, wait_entered) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let resumed = Mutex::new(resumed);
        let mut f = Fixture::new(
            false,
            "fixture-a",
            Arc::new(move || {
                entered.send(()).unwrap();
                resumed
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
                Some(fake_credential())
            }),
        );
        f.monitor.control().start().unwrap();
        wait_entered.recv_timeout(Duration::from_secs(3)).unwrap();
        f.monitor.control().signal_stop();
        resume.send(()).unwrap();
        f.monitor.shutdown();
        f.assert_no_submission();
    }

    #[test]
    fn immediate_resume_does_not_reauthorize_a_previous_cycle() {
        let (entered, first_read) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let resumed = Mutex::new(Some(resumed));
        let f = Fixture::new(
            false,
            "fixture-a",
            Arc::new(move || {
                let gate = resumed.lock().unwrap().take()?;
                entered.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(3)).unwrap();
                Some(fake_credential())
            }),
        );
        f.monitor.control().start().unwrap();
        first_read.recv_timeout(Duration::from_secs(3)).unwrap();
        let control = f.monitor.control();
        control.pause().unwrap();
        control.start().unwrap();
        resume.send(()).unwrap();
        f.until(|s| s.reason == Some(DisconnectReason::CredentialsMissing));
        f.assert_no_submission();
    }

    #[test]
    fn mismatched_configured_id_never_uses_another_accounts_password() {
        let f = Fixture::new(false, "fixture-b", Arc::new(|| Some(fake_credential())));
        f.monitor.control().start().unwrap();
        f.until(|s| s.reason == Some(DisconnectReason::CredentialsMissing));
        f.assert_no_submission();
    }

    #[test]
    fn paused_manual_check_publishes_result_and_keeps_monitoring_disabled() {
        for online in [true, false] {
            let f = Fixture::new(
                online,
                "",
                Arc::new(|| panic!("read-only scan read credentials")),
            );
            f.monitor.control().force_check().unwrap();
            let s = f.until(|s| s.metrics.check_seq == 1);
            assert_eq!(s.state, MonitorState::Paused);
            assert!(!s.monitoring_enabled);
            let point = s.metrics.last_point.unwrap();
            assert_eq!(point.online, online);
            assert_eq!(point.ms, 0);
            assert_eq!(point.confirmed_offline, Some(false));
            f.assert_no_submission();
        }
    }

    fn reply_once(listener: &TcpListener, body: &str) {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("mock portal: {e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = std::io::BufReader::new(&stream);
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                std::io::BufRead::read_line(&mut reader, &mut line).unwrap(),
                0
            );
            if line == "\r\n" {
                break;
            }
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }

    #[test]
    fn first_refusal_is_silent_and_checks_and_settings_respect_every_retry_deadline() {
        use std::sync::atomic::Ordering;
        let credential = Arc::new(Mutex::new(fake_credential()));
        let stored = Arc::clone(&credential);
        let f = Fixture::new(
            false,
            &fake_credential().username,
            Arc::new(move || Some(stored.lock().unwrap().clone())),
        );
        let ctrl = f.monitor.control();
        let refused = r#"dr1004({"result":0,"msg":1,"msga":"fake refusal"})"#;
        f.monitor.control().start().unwrap();
        let reload = |password_changed| {
            let mut settings = f.monitor.shared.settings.lock().unwrap().clone();
            settings.notify_toast = !settings.notify_toast;
            ctrl.update_settings(settings, password_changed).unwrap();
        };
        for (i, delay) in RETRY_DELAYS.into_iter().enumerate() {
            reply_once(&f.portal, refused);
            let latest = f.until(|s| s.retry_after_secs == Some(delay));
            let reason = if i == 0 {
                DisconnectReason::AuthExpired
            } else {
                DisconnectReason::LoginRejected
            };
            assert_eq!(latest.reason, Some(reason));
            if i == 1 {
                assert_eq!(latest.message, "fake refusal");
            }
            let mut seq = latest.metrics.check_seq;
            for trigger in 0..3 {
                match trigger {
                    1 => {
                        let generation = f.monitor.shared.control.lock().unwrap().generation;
                        reload(false);
                        assert_eq!(
                            generation,
                            f.monitor.shared.control.lock().unwrap().generation
                        );
                        ctrl.force_check().unwrap();
                    }
                    _ => ctrl.force_check().unwrap(),
                }
                let next =
                    f.until(|s| s.metrics.check_seq > seq && s.retry_after_secs == Some(delay));
                assert_eq!(next.reason, Some(reason));
                seq = next.metrics.check_seq;
                f.assert_no_submission();
            }
            if delay == 300 {
                ctrl.network_changed();
                f.advance(1);
                f.until(|s| s.state == MonitorState::Checking && s.metrics.check_seq > seq);
                f.advance(4);
                f.until(|s| s.retry_after_secs == Some(295) && s.reason == Some(reason));
                f.assert_no_submission();
                break;
            }
            f.elapsed.fetch_add(delay, Ordering::Release);
            ctrl.force_check().unwrap();
        }
        // Changing just the password deliberately starts a fresh refusal sequence.
        credential.lock().unwrap().password = "different-fake-password".into();
        reload(true);
        reply_once(&f.portal, refused);
        let fresh = f.until(|s| s.retry_after_secs == Some(RETRY_DELAYS[0]));
        assert_eq!(fresh.reason, Some(DisconnectReason::AuthExpired));
    }

    #[test]
    fn network_event_checks_are_read_only_while_running_and_paused() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let reads = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&reads);
        let f = Fixture::new(
            false,
            "",
            Arc::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                None
            }),
        );
        let ctrl = f.monitor.control();
        ctrl.network_changed();
        f.advance(1);
        let paused = f.until(|s| s.metrics.check_seq == 1);
        assert!(!paused.monitoring_enabled);
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        f.advance(4);
        ctrl.start().unwrap();
        f.until(|s| s.reason == Some(DisconnectReason::CredentialsMissing));
        ctrl.network_changed();
        f.advance(1);
        let enabled = f.until(|s| s.metrics.check_seq == 3);
        assert!(enabled.monitoring_enabled);
        assert_eq!(enabled.state, MonitorState::Checking);
        assert!(enabled.metrics.last_point.is_none());
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        f.assert_no_submission();
    }

    #[test]
    fn a_network_change_cancels_a_login_prepared_on_the_old_route() {
        let (entered, wait_entered) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let gate = Mutex::new(gate);
        let f = Fixture::new(
            false,
            "",
            Arc::new(move || {
                entered.send(()).unwrap();
                gate.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
                Some(fake_credential())
            }),
        );
        f.monitor.control().start().unwrap();
        wait_entered.recv_timeout(Duration::from_secs(3)).unwrap();
        f.monitor.control().network_changed();
        release.send(()).unwrap();
        f.advance(1);
        f.until(|s| s.metrics.check_seq == 2 && s.state == MonitorState::Checking);
        f.assert_no_submission();
    }

    struct SlowLatency {
        online_after: usize,
        checks: std::sync::atomic::AtomicUsize,
        entered: mpsc::Sender<()>,
        release: Mutex<Receiver<()>>,
    }
    impl NetworkCheck for SlowLatency {
        fn route(&self) -> RoutePath {
            fixture_route()
        }
        fn check_on_route(&self, route: RoutePath) -> DiagnosticResult {
            FixedCheck(
                self.checks
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    >= self.online_after,
            )
            .check_on_route(route)
        }
        fn portal_latency(&self) -> Option<u64> {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
            Some(73)
        }
    }

    #[test]
    fn online_is_published_before_slow_latency_including_after_auth_verification() {
        for after in [0, 1] {
            let (entered, probing) = mpsc::channel();
            let (release, gate) = mpsc::channel();
            let f = Fixture::with_check(
                SlowLatency {
                    online_after: after,
                    checks: 0.into(),
                    entered,
                    release: Mutex::new(gate),
                },
                "",
                Arc::new(|| Some(fake_credential())),
            );
            let ctrl = f.monitor.control();
            f.monitor.control().start().unwrap();
            if after == 1 {
                reply_once(&f.portal, r#"dr1004({"result":1})"#);
            }
            probing.recv_timeout(Duration::from_secs(3)).unwrap();
            let online = f.until(|s| s.state == MonitorState::Online);
            assert_eq!(online.metrics.last_check_ms, 0);
            let id = online.metrics.last_point.unwrap().sample_id;
            if after == 0 {
                release.send(()).unwrap();
                let measured = f.until(|s| s.metrics.last_check_ms == 73);
                assert_eq!(measured.metrics.check_seq, online.metrics.check_seq);
                assert_eq!(measured.metrics.last_point.unwrap().sample_id, id);
            } else {
                // A newer paused scan must complete while the old RTT is still blocked.
                ctrl.pause().unwrap();
                ctrl.force_check().unwrap();
                let newer = f.until(|s| s.metrics.check_seq > online.metrics.check_seq);
                assert_eq!(newer.state, MonitorState::Paused);
                assert_eq!(newer.metrics.last_check_ms, 0);
                release.send(()).unwrap();
                probing.recv_timeout(Duration::from_secs(3)).unwrap();
                release.send(()).unwrap();
                let measured = f.until(|s| s.metrics.last_check_ms > 0);
                assert_eq!(measured.state, MonitorState::Paused);
                assert_eq!(measured.metrics.check_seq, newer.metrics.check_seq);
            }
            let points: Vec<_> = MetricsStore::history(f._dir.path())
                .into_iter()
                .filter(|p| p.event == "check")
                .collect();
            assert_eq!(points.len(), if after == 0 { 1 } else { 3 });
            let matching: Vec<_> = points.iter().filter(|p| p.sample_id == id).collect();
            assert_eq!(matching.len(), 1);
            assert_eq!(matching[0].ms, if after == 0 { 73 } else { 0 });
        }
    }

    #[test]
    fn existing_paused_daemon_applies_headless_start_and_acknowledges_strict_reload() {
        use crate::ipc::{self, IpcClient, IpcServer, UiToDaemon};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let f = Fixture::new(true, "", Arc::new(|| None));
        let dir = f._dir.path();
        let path = dir.join("config.json");
        let (server, listener) = IpcServer::bind(dir).unwrap();
        let opened = Arc::new(AtomicUsize::new(0));
        let shows = Arc::clone(&opened);
        let commands = crate::DaemonCommands {
            store: crate::config::SettingsStore {
                app_dir: dir.into(),
                path: path.clone(),
            },
            control: f.monitor.control(),
            notifier: Arc::new(Mutex::new(crate::notify::ToastNotifier::new(false))),
            active_account_key: Arc::new(Mutex::new(crate::traffic::account_key_of(""))),
            traffic_tracker: Arc::new(Mutex::new(crate::traffic::TrafficTracker::new(dir))),
            latest_traffic: Arc::new(Mutex::new(crate::models::TrafficSummary::default())),
            show_ui: Box::new(move || {
                shows.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        };
        std::thread::scope(|scope| {
            let launch = scope.spawn(|| ipc::forward_launch(dir, true, true));
            // The daemon endpoint may already exist when its controller is still initializing.
            server
                .listen(listener, move |cmd| commands.apply(cmd))
                .unwrap();
            launch.join().unwrap().unwrap();
        });
        assert!(f.monitor.shared.control.lock().unwrap().enabled);
        assert_eq!(opened.load(Ordering::SeqCst), 0);
        ipc::forward_launch(dir, true, false).unwrap();
        assert_eq!(opened.load(Ordering::SeqCst), 1);

        let ep = server.endpoint();
        let (mut client, _snapshots, _logs) = IpcClient::connect(ep.port, &ep.token).unwrap();
        let mut request = |cmd| {
            let id = client.send(cmd).unwrap();
            let (reply_id, result) = client.replies.recv_timeout(ipc::COMMAND_TIMEOUT).unwrap();
            assert_eq!(reply_id, id);
            result
        };
        request(UiToDaemon::Pause).unwrap();
        let reload = || UiToDaemon::ReloadSettings {
            start: true,
            password_changed: false,
            expected: Settings {
                check_interval: 123,
                notify_toast: false,
                ..Settings::default()
            },
        };
        std::fs::write(&path, r#"{"check_interval":123,"notify_toast":false}"#).unwrap();
        request(reload()).unwrap();
        assert!(f.monitor.shared.control.lock().unwrap().enabled);
        assert_eq!(
            f.monitor.shared.settings.lock().unwrap().check_interval,
            123
        );
        request(UiToDaemon::Pause).unwrap();
        std::fs::write(&path, r#"{"check_interval":50}"#).unwrap();
        assert!(request(reload()).is_err());
        assert_eq!(
            f.monitor.shared.settings.lock().unwrap().check_interval,
            123
        );
        for missing in [false, true] {
            if missing {
                std::fs::remove_file(&path).unwrap();
            } else {
                std::fs::write(&path, "{").unwrap();
            }
            assert!(request(reload()).is_err());
            assert_eq!(
                f.monitor.shared.settings.lock().unwrap().check_interval,
                123
            );
            assert!(!f.monitor.shared.control.lock().unwrap().enabled);
        }
        f.monitor.control().signal_stop();
        assert!(ipc::forward_launch(dir, true, true).is_err());
    }
}
