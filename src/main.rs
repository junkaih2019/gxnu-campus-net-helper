//! GXNU 校园网守护助手（轻守护 + 按需界面双模式单文件架构）。
//!
//! - **守护模式（默认 / `--minimized`）**：纯 Win32 托盘 + 后台网络监控，不加载 OpenGL 与中文字库，
//!   常驻内存仅约 5–10 MB、后台 CPU 0%。
//! - **界面模式（`--ui`）**：点击托盘图标或双击程序时按需拉起完整图形界面；关闭窗口即退出界面进程，
//!   立即 100% 释放 OpenGL 驱动与字体内存。

// release 版不弹控制台窗口（debug 版保留，方便看输出）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(dead_code)]

mod app_icon;
mod authenticator;
mod campus_status;
mod config;
mod credentials;
mod diagnostics;
mod http;
mod ipc;
mod logging;
mod metrics_store;
mod models;
mod monitor;
mod network_events;
mod notify;
mod portal_gateway;
mod self_service;
mod single_instance;
mod startup;
mod traffic;
mod tray;
mod tray_flyout;
mod tray_menu;
mod ui;
mod updater;

use std::path::PathBuf;
use std::process::{Child, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use chrono::Local;
use eframe::egui;

const VERSION: &str = updater::CURRENT_VERSION;
/// 守护进程和设置界面使用的 Windows 凭据目标。
const CREDENTIAL_TARGET: &str = "GXNUCampusLogin/v4";
/// 守护进程单实例互斥体。
const MUTEX_NAME: &str = "Local\\GXNUCampusLoginRS";
/// 按需界面进程单实例互斥体，确保同时只有一个主窗口。
const UI_MUTEX_NAME: &str = "Local\\GXNUCampusLoginRS_UI";
const APP_NAME: &str = env!("CARGO_PKG_DESCRIPTION");

const WM_TRAY_REFRESH: u32 = 0x8000 + 1;
const WM_DAEMON_QUIT: u32 = 0x8000 + 2;
const WM_TRAY_SHOW_MENU: u32 = 0x8000 + 3;
// 注：`0x8000 + 4` 已被 `network_events::CHANGED` 占用
const WM_TRAY_TOGGLE_FLYOUT: u32 = 0x8000 + 5;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // ── 模式 0：受保护目录 UAC 提权覆盖辅助模式（`--apply-update <target_exe> <old_pid>`）──
    if let Some(idx) = args.iter().position(|arg| arg == "--apply-update") {
        let Some(target) = args.get(idx + 1).map(PathBuf::from) else {
            return ExitCode::FAILURE;
        };
        let wait_pid = args
            .get(idx + 2)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        return match updater::run_elevated_apply_update(&target, wait_pid) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        };
    }

    // ── 模式 1：按需拉起的图形界面子进程（关闭即彻底释放 OpenGL 与字库内存）────
    if args.iter().any(|arg| arg == "--ui") {
        return run_ui_process(&args);
    }

    // 若由自动更新原位覆盖后拉起（`--after-update <old_pid>`），先等待旧守护进程彻底退出并释放单实例互斥体
    let mut just_updated = false;
    if let Some(idx) = args.iter().position(|arg| arg == "--after-update") {
        let wait_pid = args
            .get(idx + 1)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        updater::wait_for_pid_exit(wait_pid, Duration::from_secs(12));
        just_updated = true;
    }

    let once = args.iter().any(|arg| arg == "--once");
    let test_toast = args.iter().any(|arg| arg == "--test-toast");
    let force_start = args.iter().any(|arg| arg == "--start");
    let minimized = args.iter().any(|arg| arg == "--minimized");

    let store = config::SettingsStore::new();
    if let Err(err) = std::fs::create_dir_all(&store.app_dir) {
        eprintln!("无法创建应用目录 {}：{err}", store.app_dir.display());
        return ExitCode::FAILURE;
    }
    updater::cleanup_old_artifacts(&store.app_dir);

    if test_toast {
        let _ = logging::init(&store.app_dir.join("logs"), false);
        startup::ensure_app_id(notify::AUMID);
        let notifier = notify::ToastNotifier::new(true);
        notifier.send_test("这是一条测试通知：断网/恢复时也会这样弹出");
        println!("已请求发送测试通知");
        return ExitCode::SUCCESS;
    }

    // --autostart on|off|status：写入/删除 HKCU Run 项并退出。
    if let Some(index) = args.iter().position(|arg| arg == "--autostart") {
        let _ = logging::init(&store.app_dir.join("logs"), false);
        let value = args.get(index + 1).map(String::as_str).unwrap_or("status");
        let result = match value {
            "on" => startup::set_enabled(true),
            "off" => startup::set_enabled(false),
            _ => {
                println!(
                    "autostart = {}",
                    if startup::is_enabled() { "on" } else { "off" }
                );
                return ExitCode::SUCCESS;
            }
        };
        match result {
            Ok(()) => {
                logging::info(&format!(
                    "[STARTUP] 开机自启已{}",
                    if value == "on" { "开启" } else { "关闭" }
                ));
                println!("autostart = {value}");
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                logging::error(&format!("[STARTUP] 设置自启失败：{err}"));
                eprintln!("{err}");
                return ExitCode::FAILURE;
            }
        }
    }

    // --once 是只读自检，不占用互斥体，方便与守护实例并存。
    if once {
        let _ = logging::init(&store.app_dir.join("logs"), false);
        logging::info("[SELF_CHECK] 一次性只读检测（不提交凭据）");
        let once_cfg = store.load();
        let once_resolver = Arc::new(portal_gateway::PortalGatewayResolver::new(
            once_cfg.campus_zone,
            &once_cfg.custom_portal_url,
        ));
        let diagnostics = diagnostics::NetworkDiagnostics::new().with_campus_status(
            campus_status::CampusStatusProbe::with_resolver(once_resolver),
        );
        let Some(result) = diagnostics.check() else {
            eprintln!("网络出口正在切换，两次只读检测均未取得稳定结果");
            return ExitCode::from(3);
        };
        println!(
            "online={} reason={} message={}",
            result.online,
            result.reason.as_str(),
            result.message
        );
        println!("ip={} gateway={}", result.local_ip, result.gateway);
        println!("evidence: {}", result.evidence_text());
        return if result.online {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(2)
        };
    }

    // 单实例检查：若守护已在运行，直接通知它拉起/前置主窗口并退出，不再重复启动。
    let _instance = match single_instance::InstanceGuard::acquire(MUTEX_NAME) {
        Ok((_guard, true)) => {
            return match ipc::forward_launch(&store.app_dir, force_start, minimized) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("{err}");
                    ExitCode::FAILURE
                }
            };
        }
        Ok((guard, false)) => Some(guard),
        Err(err) => {
            eprintln!("{err}");
            None
        }
    };

    let log_path = match logging::init(&store.app_dir.join("logs"), true) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("无法初始化日志：{err}");
            return ExitCode::FAILURE;
        }
    };
    logging::info(&format!(
        "[APP_STARTED] {APP_NAME} {VERSION} 守护模式启动，日志路径：{}",
        log_path.display()
    ));
    logging::info(
        "[NET_POLICY] 探测与认证直连（不吃系统代理）；门户说在线则不重复登录；认证首次被拒只静默重试",
    );

    let mut settings = store.load();
    settings.auto_boot = startup::is_enabled();
    if force_start {
        settings.auto_start_monitoring = true;
    }
    startup::ensure_app_id(notify::AUMID);
    logging::info(&format!(
        "[CONFIG] 学号={}；运营商={}；校区={}；检测间隔={}s；复核={}次；离线复检={}s；开机自启={}",
        mask_id(&settings.student_id),
        settings.carrier,
        settings.campus_zone.short_label(),
        settings.check_interval,
        settings.confirm_failures,
        settings.offline_interval,
        if settings.auto_boot { "开" } else { "关" }
    ));

    if just_updated {
        logging::info(&format!("[UPDATER] 已成功原位覆盖升级并重启至 v{VERSION}"));
    }

    run_daemon(store, settings, log_path, minimized, just_updated)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiOpenTarget {
    Normal,
    AccountSettings,
    UpdateModal,
    AfterUpdateToast,
}

/// ── 模式 2：超轻量常驻守护进程（无 OpenGL、无字库、纯 Win32 托盘消息循环）──
fn run_daemon(
    store: config::SettingsStore,
    settings: models::Settings,
    log_path: PathBuf,
    minimized: bool,
    just_updated: bool,
) -> ExitCode {
    tray_menu::enable_dpi_awareness();
    let main_tid = current_thread_id();
    let mut tray = match tray::TrayUi::new() {
        Ok(tray) => tray,
        Err(err) => {
            logging::error(&format!("[TRAY_ERROR] {err}"));
            return ExitCode::FAILURE;
        }
    };

    let notifier = Arc::new(Mutex::new(notify::ToastNotifier::new(
        settings.notify_toast,
    )));
    let metrics_store = Arc::new(metrics_store::MetricsStore::new(&store.app_dir));
    let traffic_tracker = Arc::new(Mutex::new(traffic::TrafficTracker::new(&store.app_dir)));
    let active_account_id = Arc::new(Mutex::new(settings.active_account_id.clone()));
    let active_account_label = Arc::new(Mutex::new(settings.active_display_name()));
    let active_carrier = Arc::new(Mutex::new(settings.carrier.clone()));
    let active_account_key = Arc::new(Mutex::new(traffic::account_key_of(&settings.student_id)));
    let latest_traffic = Arc::new(Mutex::new(traffic_tracker.lock().unwrap().summary(
        &active_account_key.lock().unwrap(),
        Local::now().date_naive(),
    )));
    let is_online = Arc::new(AtomicBool::new(false));
    let latest_snapshot: Arc<Mutex<Option<models::MonitorSnapshot>>> = Arc::new(Mutex::new(None));
    let online_since_unix: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));
    let last_check_unix: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));
    let last_check_seq: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let latest_self_service: Arc<Mutex<models::SelfServiceState>> =
        Arc::new(Mutex::new(models::SelfServiceState::default()));
    let latest_update: Arc<Mutex<models::UpdateState>> =
        Arc::new(Mutex::new(models::UpdateState {
            current_version: VERSION.to_string(),
            ..Default::default()
        }));
    let skipped_update_ver = Arc::new(Mutex::new(settings.skipped_update_version.clone()));
    let ui_child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));

    let (ipc_server, listener) = match ipc::IpcServer::bind(&store.app_dir) {
        Ok((server, listener)) => (Arc::new(server), listener),
        Err(err) => {
            logging::error(&format!("[IPC_ERROR] {err}"));
            return ExitCode::FAILURE;
        }
    };

    // 自助服务低频按需同步管理器（按学号共享 5 分钟缓存，绝不跟随 10s 巡检轮询）
    let initial_acct_id = settings
        .active_account()
        .map(|a| a.id.clone())
        .unwrap_or_else(|| settings.active_account_id.clone());
    let self_service_mgr = Arc::new(self_service::SelfServiceManager::new(
        initial_acct_id,
        settings.student_id.clone(),
        settings.carrier.clone(),
        Arc::new(|acct_id, student_id| {
            credentials::load_for_account(acct_id, student_id)
                .ok()
                .flatten()
        }),
        {
            let shared_ss = Arc::clone(&latest_self_service);
            let latest_snap = Arc::clone(&latest_snapshot);
            let online_since = Arc::clone(&online_since_unix);
            let last_check = Arc::clone(&last_check_unix);
            let shared_traffic = Arc::clone(&latest_traffic);
            let acct_id_ref = Arc::clone(&active_account_id);
            let shared_upd = Arc::clone(&latest_update);
            let ipc = Arc::clone(&ipc_server);
            Arc::new(move |state| {
                if let Ok(mut g) = shared_ss.lock() {
                    *g = state.clone();
                }
                post_thread_msg(main_tid, WM_TRAY_REFRESH);
                if let Some(snap) = latest_snap.lock().ok().and_then(|g| g.clone()) {
                    ipc.broadcast_snapshot(ipc::SnapshotEnvelope {
                        snapshot: snap,
                        online_since_unix: online_since.lock().ok().and_then(|g| *g),
                        last_check_unix: last_check.lock().ok().and_then(|g| *g),
                        traffic: shared_traffic.lock().map(|g| g.clone()).unwrap_or_default(),
                        active_account_id: acct_id_ref
                            .lock()
                            .map(|g| g.clone())
                            .unwrap_or_default(),
                        self_service: state,
                        update: shared_upd.lock().map(|g| g.clone()).unwrap_or_default(),
                    });
                }
            })
        },
    ));

    // 实时日志转发到已连接的 --ui 进程
    let log_rx = logging::subscribe();
    let ipc_for_logs = Arc::clone(&ipc_server);
    let _ = std::thread::Builder::new()
        .name("ipc-log-forwarder".to_string())
        .spawn(move || {
            while let Ok(line) = log_rx.recv() {
                ipc_for_logs.broadcast_log(line);
            }
        });

    let credential_provider: monitor::CredentialProvider =
        Arc::new(move |cfg: &models::Settings| {
            let acct_id = cfg
                .active_account()
                .map(|a| a.id.as_str())
                .unwrap_or(cfg.active_account_id.as_str());
            match credentials::load_for_account(acct_id, &cfg.student_id) {
                Ok(Some(stored)) => Some(stored),
                Ok(None) => None,
                Err(err) => {
                    logging::error(&format!("[CREDENTIAL_ERROR] 无法读取 Windows 凭据：{err}"));
                    None
                }
            }
        });

    #[derive(Default)]
    struct SelfServiceHookState {
        last_state: Option<models::MonitorState>,
        auth_expired_dropped_at: Option<i64>,
        login_rejected_queried: bool,
    }

    let sink_notifier = Arc::clone(&notifier);
    let sink_latest = Arc::clone(&latest_snapshot);
    let sink_online = Arc::clone(&online_since_unix);
    let sink_last_check = Arc::clone(&last_check_unix);
    let sink_seq = Arc::clone(&last_check_seq);
    let sink_ipc = Arc::clone(&ipc_server);
    let sink_is_online = Arc::clone(&is_online);
    let sink_traffic = Arc::clone(&latest_traffic);
    let sink_acct_id = Arc::clone(&active_account_id);
    let sink_self_service_state = Arc::clone(&latest_self_service);
    let sink_update_state = Arc::clone(&latest_update);
    let sink_self_service_mgr = Arc::clone(&self_service_mgr);
    let sink_hook = Arc::new(Mutex::new(SelfServiceHookState::default()));

    let sink: monitor::SnapshotSink = Arc::new(move |snapshot| {
        if let Ok(mut n) = sink_notifier.lock() {
            n.handle(&snapshot);
        }

        sink_is_online.store(
            snapshot.state == models::MonitorState::Online,
            Ordering::Release,
        );

        let now = Local::now().timestamp();
        let online_ts = if let Ok(mut guard) = sink_online.lock() {
            match snapshot.state {
                models::MonitorState::Online => {
                    if guard.is_none() {
                        *guard = Some(now);
                    }
                }
                models::MonitorState::Offline
                | models::MonitorState::Paused
                | models::MonitorState::Stopped
                | models::MonitorState::Authenticating => {
                    *guard = None;
                }
                _ => {}
            }
            *guard
        } else {
            None
        };

        let check_ts = if let Ok(mut seq) = sink_seq.lock() {
            if snapshot.metrics.check_seq > *seq {
                *seq = snapshot.metrics.check_seq;
                if let Ok(mut ts) = sink_last_check.lock() {
                    *ts = Some(now);
                }
            }
            sink_last_check.lock().ok().and_then(|g| *g)
        } else {
            None
        };

        // 事件驱动异步复核（绝不阻塞重连主链路）
        let maybe_trigger = if let Ok(mut hook) = sink_hook.lock() {
            let prev = hook.last_state;
            hook.last_state = Some(snapshot.state);
            let campus_ip = snapshot.metrics.effective_campus_ip().to_string();
            match snapshot.state {
                models::MonitorState::Offline => {
                    if snapshot.reason == Some(models::DisconnectReason::AuthExpired)
                        && hook.auth_expired_dropped_at.is_none()
                    {
                        hook.auth_expired_dropped_at = Some(now);
                    }
                    if snapshot.reason == Some(models::DisconnectReason::LoginRejected)
                        && !hook.login_rejected_queried
                    {
                        hook.login_rejected_queried = true;
                        Some(self_service::FetchTrigger::LoginRejected {
                            local_campus_ip: campus_ip,
                        })
                    } else {
                        None
                    }
                }
                models::MonitorState::Online => {
                    hook.login_rejected_queried = false;
                    if let Some(dropped_at) = hook.auth_expired_dropped_at.take() {
                        Some(self_service::FetchTrigger::PostReconnect {
                            dropped_at_unix: dropped_at,
                            local_campus_ip: campus_ip,
                        })
                    } else if matches!(
                        prev,
                        Some(models::MonitorState::Verifying)
                            | Some(models::MonitorState::Authenticating)
                    ) {
                        Some(self_service::FetchTrigger::SessionEstablished)
                    } else {
                        None
                    }
                }
                models::MonitorState::Paused | models::MonitorState::Stopped => {
                    hook.auth_expired_dropped_at = None;
                    hook.login_rejected_queried = false;
                    None
                }
                _ => None,
            }
        } else {
            None
        };

        if let Ok(mut guard) = sink_latest.lock() {
            *guard = Some(snapshot.clone());
        }
        post_thread_msg(main_tid, WM_TRAY_REFRESH);

        let traffic_summary = sink_traffic.lock().map(|g| g.clone()).unwrap_or_default();
        let active_id = sink_acct_id.lock().map(|g| g.clone()).unwrap_or_default();
        let ss_state = sink_self_service_state
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        let upd_state = sink_update_state
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        sink_ipc.broadcast_snapshot(ipc::SnapshotEnvelope {
            snapshot,
            online_since_unix: online_ts,
            last_check_unix: check_ts,
            traffic: traffic_summary,
            active_account_id: active_id,
            self_service: ss_state,
            update: upd_state,
        });

        if let Some(trigger) = maybe_trigger {
            sink_self_service_mgr.trigger(trigger);
        }
    });

    // 超轻量后台物理网卡流量采样线程（每 5s 增量采样，退时立即唤醒并落盘）
    let traffic_stop = Arc::new((Mutex::new(false), Condvar::new()));
    let traffic_thread = {
        let stop_pair = Arc::clone(&traffic_stop);
        let tracker = Arc::clone(&traffic_tracker);
        let acct = Arc::clone(&active_account_key);
        let online_flag = Arc::clone(&is_online);
        let latest_snap = Arc::clone(&latest_snapshot);
        let online_since = Arc::clone(&online_since_unix);
        let last_check = Arc::clone(&last_check_unix);
        let shared_traffic = Arc::clone(&latest_traffic);
        let acct_id_ref = Arc::clone(&active_account_id);
        let shared_ss = Arc::clone(&latest_self_service);
        let shared_upd = Arc::clone(&latest_update);
        let ipc = Arc::clone(&ipc_server);
        std::thread::Builder::new()
            .name("traffic-sampler".to_string())
            .spawn(move || {
                let (lock, cv) = &*stop_pair;
                loop {
                    let guard = lock.lock().unwrap();
                    let (guard, _) = cv
                        .wait_timeout_while(guard, Duration::from_secs(5), |stopped| !*stopped)
                        .unwrap();
                    if *guard {
                        break;
                    }
                    drop(guard);

                    let counters = traffic::read_physical_nic_counters();
                    let online = online_flag.load(Ordering::Acquire);
                    let account_key = acct.lock().unwrap().clone();
                    let today = Local::now().date_naive();
                    let (changed, summary) =
                        tracker
                            .lock()
                            .unwrap()
                            .tick(online, &account_key, today, &counters);
                    if let Ok(mut g) = shared_traffic.lock() {
                        *g = summary.clone();
                    }
                    if changed {
                        if let Some(snap) = latest_snap.lock().ok().and_then(|g| g.clone()) {
                            ipc.broadcast_snapshot(ipc::SnapshotEnvelope {
                                snapshot: snap,
                                online_since_unix: online_since.lock().ok().and_then(|g| *g),
                                last_check_unix: last_check.lock().ok().and_then(|g| *g),
                                traffic: summary,
                                active_account_id: acct_id_ref
                                    .lock()
                                    .map(|g| g.clone())
                                    .unwrap_or_default(),
                                self_service: shared_ss
                                    .lock()
                                    .map(|g| g.clone())
                                    .unwrap_or_default(),
                                update: shared_upd.lock().map(|g| g.clone()).unwrap_or_default(),
                            });
                        }
                    }
                }
                tracker.lock().unwrap().flush();
            })
            .ok()
    };

    let gateway_resolver = Arc::new(portal_gateway::PortalGatewayResolver::new(
        settings.campus_zone,
        &settings.custom_portal_url,
    ));
    let diagnostics = diagnostics::NetworkDiagnostics::new().with_campus_status(
        campus_status::CampusStatusProbe::with_resolver(Arc::clone(&gateway_resolver)),
    );
    let authenticator =
        authenticator::CampusAuthenticator::with_resolver(Arc::clone(&gateway_resolver));
    let metrics_for_flyout = Arc::clone(&metrics_store);
    let mut monitor = monitor::NetworkMonitor::new(
        diagnostics,
        authenticator,
        credential_provider,
        metrics_store,
        settings.clone(),
        sink,
    )
    .with_gateway_resolver(Arc::clone(&gateway_resolver));
    let monitor_ctrl = monitor.control();
    let updater_ctrl = monitor_ctrl.clone();
    let updater_mgr = Arc::new(updater::UpdateManager::new(
        store.app_dir.clone(),
        settings.auto_check_updates,
        settings.skipped_update_version.clone(),
        Arc::clone(&is_online),
        {
            let shared_upd = Arc::clone(&latest_update);
            let ipc = Arc::clone(&ipc_server);
            Arc::new(move |upd_state| {
                if let Ok(mut g) = shared_upd.lock() {
                    *g = upd_state.clone();
                }
                ipc.broadcast_update_state(upd_state);
                post_thread_msg(main_tid, WM_TRAY_REFRESH);
            })
        },
        Arc::new(move || {
            updater_ctrl.signal_stop();
            post_thread_msg(main_tid, WM_DAEMON_QUIT);
        }),
    ));
    if let Ok(mut g) = latest_update.lock() {
        *g = updater_mgr.current_state();
    }

    let action_ctrl = monitor_ctrl.clone();
    let action_log_path = log_path.clone();
    let endpoint = ipc_server.endpoint().clone();
    let child = Arc::clone(&ui_child);
    let ipc_for_show = Arc::clone(&ipc_server);
    let app_dir = store.app_dir.clone();
    let commands = Arc::new(Mutex::new(DaemonCommands {
        main_tid,
        control: monitor_ctrl.clone(),
        gateway_resolver: Arc::clone(&gateway_resolver),
        store,
        notifier,
        ipc_server: Arc::clone(&ipc_server),
        is_online: Arc::clone(&is_online),
        active_account_id: Arc::clone(&active_account_id),
        active_account_label: Arc::clone(&active_account_label),
        active_carrier: Arc::clone(&active_carrier),
        active_account_key: Arc::clone(&active_account_key),
        skipped_update_ver: Arc::clone(&skipped_update_ver),
        traffic_tracker: Arc::clone(&traffic_tracker),
        latest_traffic: Arc::clone(&latest_traffic),
        self_service: Arc::clone(&self_service_mgr),
        updater: Arc::clone(&updater_mgr),
        show_ui: Box::new(move |target| show_or_spawn_ui(&child, &endpoint, &ipc_for_show, target)),
    }));
    if settings.auto_start_monitoring {
        if let Err(err) = commands.lock().unwrap().apply(ipc::UiToDaemon::Start) {
            logging::error(&format!("[MONITOR_ERROR] {err}"));
            monitor.shutdown();
            return ExitCode::FAILURE;
        }
    } else {
        logging::info("[MONITOR_PAUSED] 监控处于暂停状态");
    }
    let ipc_commands = Arc::clone(&commands);
    if let Err(err) =
        ipc_server.listen(listener, move |cmd| ipc_commands.lock().unwrap().apply(cmd))
    {
        logging::error(&format!("[IPC_ERROR] {err}"));
        monitor.shutdown();
        return ExitCode::FAILURE;
    }

    let tray_commands = Arc::clone(&commands);
    let dispatch_tray_action = Arc::new(move |action: tray::TrayAction| {
        let command = match action {
            tray::TrayAction::ToggleFlyout => {
                return post_thread_msg(main_tid, WM_TRAY_TOGGLE_FLYOUT)
            }
            tray::TrayAction::Show => ipc::UiToDaemon::ShowUi,
            tray::TrayAction::OpenAccountSettings => {
                if let Err(err) = tray_commands.lock().unwrap().open_account_settings() {
                    logging::warn(&format!("[COMMAND_FAILED] {err}"));
                }
                return;
            }
            tray::TrayAction::OpenUpdate => {
                if let Err(err) = tray_commands.lock().unwrap().open_update_modal() {
                    logging::warn(&format!("[COMMAND_FAILED] {err}"));
                }
                return;
            }
            tray::TrayAction::SwitchAccount(account_id) => {
                if let Err(err) = tray_commands.lock().unwrap().switch_from_tray(&account_id) {
                    logging::warn(&format!("[ACCOUNT_SWITCH_FAILED] {err}"));
                }
                return;
            }
            tray::TrayAction::KickDevice { ip, is_local } => {
                if let Err(err) = tray_commands.lock().unwrap().kick_device(ip, is_local) {
                    logging::warn(&format!("[DEVICE_KICK_FAILED] {err}"));
                }
                return;
            }
            tray::TrayAction::Start => ipc::UiToDaemon::Start,
            tray::TrayAction::Pause => ipc::UiToDaemon::Pause,
            tray::TrayAction::Check => ipc::UiToDaemon::ForceCheck,
            tray::TrayAction::OpenMenu => return post_thread_msg(main_tid, WM_TRAY_SHOW_MENU),
            tray::TrayAction::OpenLog => return logging::reveal_in_explorer(&action_log_path),
            tray::TrayAction::Exit => {
                action_ctrl.signal_stop();
                return post_thread_msg(main_tid, WM_DAEMON_QUIT);
            }
        };
        if let Err(err) = tray_commands.lock().unwrap().apply(command) {
            logging::warn(&format!("[COMMAND_FAILED] {err}"));
        }
    });

    let menu_cb = Arc::clone(&dispatch_tray_action);
    let mut fluent_menu = match tray_menu::FluentTrayMenu::new(
        app_dir.clone(),
        Arc::clone(&latest_snapshot),
        Arc::clone(&active_account_label),
        Arc::clone(&latest_update),
        Arc::clone(&skipped_update_ver),
        move |action| menu_cb(action),
    ) {
        Ok(menu) => Some(menu),
        Err(err) => {
            logging::error(&format!("[TRAY_MENU_ERROR] {err}"));
            None
        }
    };

    let flyout_cb = Arc::clone(&dispatch_tray_action);
    let mut fluent_flyout = match tray_flyout::FluentTrayFlyout::new(
        tray_flyout::FlyoutDataSources {
            app_dir,
            latest_snapshot: Arc::clone(&latest_snapshot),
            online_since_unix: Arc::clone(&online_since_unix),
            latest_traffic: Arc::clone(&latest_traffic),
            latest_self_service: Arc::clone(&latest_self_service),
            active_account_label: Arc::clone(&active_account_label),
            active_carrier: Arc::clone(&active_carrier),
            metrics_store: metrics_for_flyout,
        },
        move |action| flyout_cb(action),
    ) {
        Ok(flyout) => Some(flyout),
        Err(err) => {
            logging::error(&format!("[TRAY_FLYOUT_ERROR] {err}"));
            None
        }
    };

    let tray_cb = Arc::clone(&dispatch_tray_action);
    tray.start_event_listener(move |action| tray_cb(action));

    logging::info("[APP_READY] 轻守护进程已就绪（系统托盘运行中）");

    // 非 --minimized 启动时立即弹出图形界面；关闭窗口后界面进程退出，守护继续留守托盘。
    if !minimized {
        let open_res = if just_updated {
            commands.lock().unwrap().open_after_update()
        } else {
            commands.lock().unwrap().apply(ipc::UiToDaemon::ShowUi)
        };
        if let Err(err) = open_res {
            logging::warn(&format!("[UI_ERROR] {err}"));
        }
    }

    trim_working_set_win32();

    // 零 CPU 原生 Win32 消息循环（仅在托盘点击、浮窗/菜单交互或状态变化时被内核唤醒）
    run_win32_tray_loop(
        &mut tray,
        fluent_menu.as_mut(),
        fluent_flyout.as_mut(),
        &latest_snapshot,
        &monitor_ctrl,
        &self_service_mgr,
    );

    // ── 干净退出流程 ───────────────────────────────────────────────
    updater_mgr.shutdown();
    monitor_ctrl.signal_stop();
    {
        let (lock, cv) = &*traffic_stop;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
    logging::info("[APP_EXIT] 正在退出");
    ipc_server.broadcast_shutdown();
    if let Some(flyout) = fluent_flyout.as_mut() {
        flyout.hide();
    }
    drop(fluent_flyout);
    if let Some(menu) = fluent_menu.as_mut() {
        menu.hide();
    }
    drop(fluent_menu);
    drop(tray);
    stop_ui_child(&ui_child);
    monitor.shutdown();
    if let Some(handle) = traffic_thread {
        let _ = handle.join();
    }
    ExitCode::SUCCESS
}

struct DaemonCommands {
    main_tid: u32,
    control: monitor::MonitorControl,
    gateway_resolver: Arc<portal_gateway::PortalGatewayResolver>,
    store: config::SettingsStore,
    notifier: Arc<Mutex<notify::ToastNotifier>>,
    ipc_server: Arc<ipc::IpcServer>,
    is_online: Arc<AtomicBool>,
    active_account_id: Arc<Mutex<String>>,
    active_account_label: Arc<Mutex<String>>,
    active_carrier: Arc<Mutex<String>>,
    active_account_key: Arc<Mutex<String>>,
    skipped_update_ver: Arc<Mutex<String>>,
    traffic_tracker: Arc<Mutex<traffic::TrafficTracker>>,
    latest_traffic: Arc<Mutex<models::TrafficSummary>>,
    self_service: Arc<self_service::SelfServiceManager>,
    updater: Arc<updater::UpdateManager>,
    show_ui: Box<dyn Fn(UiOpenTarget) -> Result<(), String> + Send + Sync>,
}

impl DaemonCommands {
    fn sync_active_account_state(&self, settings: &models::Settings) {
        let new_key = traffic::account_key_of(&settings.student_id);
        let acct_id = settings
            .active_account()
            .map(|a| a.id.as_str())
            .unwrap_or(settings.active_account_id.as_str());
        *self.active_account_id.lock().unwrap() = settings.active_account_id.clone();
        *self.active_account_label.lock().unwrap() = settings.active_display_name();
        *self.active_carrier.lock().unwrap() = settings.carrier.clone();
        *self.active_account_key.lock().unwrap() = new_key.clone();
        let refreshed = self
            .traffic_tracker
            .lock()
            .unwrap()
            .summary(&new_key, Local::now().date_naive());
        *self.latest_traffic.lock().unwrap() = refreshed;
        self.self_service
            .sync_account(acct_id, &settings.student_id, &settings.carrier);
    }

    fn open_account_settings(&self) -> Result<(), String> {
        (self.show_ui)(UiOpenTarget::AccountSettings)?;
        self.self_service
            .trigger(self_service::FetchTrigger::UiOpen);
        Ok(())
    }

    fn open_update_modal(&self) -> Result<(), String> {
        (self.show_ui)(UiOpenTarget::UpdateModal)?;
        self.self_service
            .trigger(self_service::FetchTrigger::UiOpen);
        Ok(())
    }

    fn open_after_update(&self) -> Result<(), String> {
        (self.show_ui)(UiOpenTarget::AfterUpdateToast)?;
        self.self_service
            .trigger(self_service::FetchTrigger::UiOpen);
        Ok(())
    }

    fn switch_from_tray(&self, account_id: &str) -> Result<(), String> {
        let mut settings = self.store.load();
        if settings.active_account_id == account_id {
            return Ok(());
        }
        let target = settings.switch_active_account(account_id)?;
        if !credentials::has_credential_for(&target.id, &target.student_id) {
            let _ = self.open_account_settings();
            return Err(format!(
                "账号「{}」尚未保存密码，已打开设置页供补充",
                target.display_name()
            ));
        }
        self.store.save_settings(&settings)?;
        let label = settings.active_display_name();
        self.sync_active_account_state(&settings);
        self.self_service.notify_account_switching();
        self.control.switch_account(settings)?;
        if self.main_tid != 0 {
            post_thread_msg(self.main_tid, WM_TRAY_REFRESH);
        }
        logging::info(&format!(
            "[ACCOUNT_SWITCH] 托盘快捷浮窗开始切换至账号「{label}」"
        ));
        Ok(())
    }

    fn kick_device(&self, ip: String, is_local: bool) -> Result<(), String> {
        if is_local {
            self.control.pause()?;
            logging::info(&format!(
                "[MONITOR_PAUSED] 用户请求注销本机（{ip}），已自动暂停自动重连守护"
            ));
        }
        let ss = Arc::clone(&self.self_service);
        let ctrl = self.control.clone();
        let gw_resolver = Arc::clone(&self.gateway_resolver);
        let ipc = Arc::clone(&self.ipc_server);
        let online_flag = Arc::clone(&self.is_online);
        let main_tid = self.main_tid;
        let _ = std::thread::Builder::new()
            .name("kick-online-device".to_string())
            .spawn(move || {
                if is_local {
                    let _ = authenticator::CampusAuthenticator::with_resolver(gw_resolver).logout();
                }
                let res = ss.kick_device(&ip);
                let ui_res = match &res {
                    Ok(_) => {
                        logging::info(&format!(
                            "[SELF_SERVICE] 已通过自助服务将终端 {ip}{}注销下线",
                            if is_local {
                                "（本机，守护已暂停）"
                            } else {
                                " "
                            }
                        ));
                        Ok(())
                    }
                    Err(err) => {
                        logging::warn(&format!("[SELF_SERVICE] 注销终端 {ip} 失败：{err}"));
                        Err(err.clone())
                    }
                };
                ipc.broadcast_kick_result(ip, is_local, ui_res);
                if main_tid != 0 {
                    post_thread_msg(main_tid, WM_TRAY_REFRESH);
                }
                // 若本机此前因并发满额而处于未联网状态，踢掉其他占线设备后自动触发一次重连检测
                if !is_local && res.is_ok() && !online_flag.load(Ordering::Acquire) {
                    let _ = ctrl.force_check();
                }
            });
        Ok(())
    }

    fn apply(&self, cmd: ipc::UiToDaemon) -> Result<(), String> {
        use ipc::UiToDaemon::*;
        match cmd {
            Start => {
                self.control.start()?;
                logging::info("[MONITOR_STARTED] 用户开始监控");
            }
            Pause => {
                self.control.pause()?;
                logging::info("[MONITOR_PAUSED] 用户暂停监控");
            }
            ForceCheck => {
                self.control.force_check()?;
                logging::info("[FORCE_CHECK] 用户要求立即检测");
            }
            RefreshSelfService { force } => {
                self.self_service.trigger(if force {
                    self_service::FetchTrigger::Manual
                } else {
                    self_service::FetchTrigger::UiOpen
                });
            }
            KickDevice { ip, is_local } => {
                self.kick_device(ip, is_local)?;
            }
            CheckUpdate { manual } => {
                self.updater.request_check(manual);
            }
            StartUpdate => {
                self.updater.request_install()?;
            }
            ReloadSettings {
                start,
                password_changed,
                expected,
            } => {
                let settings = self
                    .store
                    .load_strict()
                    .map_err(|e| format!("读取配置失败：{e}"))?;
                if settings != expected {
                    return Err("磁盘设置已被其他写入更新，请重新核对后保存".into());
                }
                let enabled = settings.notify_toast;
                *self.skipped_update_ver.lock().unwrap() = settings.skipped_update_version.clone();
                self.updater.sync_preferences(
                    settings.auto_check_updates,
                    &settings.skipped_update_version,
                );
                self.gateway_resolver
                    .update_config(settings.campus_zone, &settings.custom_portal_url);
                self.sync_active_account_state(&settings);
                if password_changed {
                    self.self_service.notify_credential_updated();
                }
                self.control.update_settings(settings, password_changed)?;
                self.notifier.lock().unwrap().set_enabled(enabled);
                if start {
                    self.control.start()?;
                }
                self.self_service
                    .trigger(self_service::FetchTrigger::UiOpen);
                logging::info("[SETTINGS_SAVED] 守护进程已应用设置");
            }
            SwitchAccount {
                account_id,
                expected,
            } => {
                let settings = self
                    .store
                    .load_strict()
                    .map_err(|e| format!("读取配置失败：{e}"))?;
                if settings != expected || settings.active_account_id != account_id {
                    return Err("磁盘账号配置已变化，请刷新后重试切换".into());
                }
                let label = settings.active_display_name();
                self.sync_active_account_state(&settings);
                self.self_service.notify_account_switching();
                self.control.switch_account(settings)?;
                logging::info(&format!(
                    "[ACCOUNT_SWITCH] 守护进程开始切换至账号「{label}」"
                ));
            }
            ShowUi => {
                (self.show_ui)(UiOpenTarget::Normal)?;
                self.self_service
                    .trigger(self_service::FetchTrigger::UiOpen);
            }
            TestToast { body } => self.notifier.lock().unwrap().send_test(&body),
        }
        Ok(())
    }
}

/// 按需拉起的图形界面进程（关闭即完全退出，释放 OpenGL 驱动与字体内存）。
fn run_ui_process(args: &[String]) -> ExitCode {
    let port = parse_arg(args, "--ipc-port").and_then(|s| s.parse::<u16>().ok());
    let token = parse_arg(args, "--ipc-token");

    let (Some(port), Some(token)) = (port, token) else {
        eprintln!("缺少 --ipc-port / --ipc-token 参数");
        return ExitCode::FAILURE;
    };

    // 确保同时只存在一个界面窗口（仅 debug 截图模式下跳过单实例锁，避免干扰用户运行中的实例）
    #[cfg(debug_assertions)]
    let skip_ui_mutex = std::env::var_os("GXNU_SCREENSHOT_OUT").is_some();
    #[cfg(not(debug_assertions))]
    let skip_ui_mutex = false;

    let _ui_guard = if skip_ui_mutex {
        None
    } else {
        match single_instance::InstanceGuard::acquire(UI_MUTEX_NAME) {
            Ok((_guard, true)) => {
                bring_any_ui_window_to_front();
                return ExitCode::SUCCESS;
            }
            Ok((guard, false)) => Some(guard),
            Err(_) => None,
        }
    };

    let store = config::SettingsStore::new();
    let log_path = store.app_dir.join("logs").join("app.log");
    let mut settings = store.load();
    settings.auto_boot = startup::is_enabled();

    let ipc_client = match ipc::IpcClient::connect(port, &token) {
        Ok(res) => res,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };

    let initial_telemetry = metrics_store::MetricsStore::history(&store.app_dir);
    let initial_traffic = traffic::TrafficTracker::read_summary(
        &store.app_dir,
        &traffic::account_key_of(&settings.student_id),
    );
    let initial_logs = logging::read_tail(&log_path, 800);
    let ui_prefs = ui::load_ui_prefs(&store.app_dir);
    let initial_size = ui_prefs.effective_window_size();

    let deps = ui::GuardDeps {
        store,
        settings,
        ipc: ipc_client,
        log_path,
        initial_logs,
        initial_telemetry,
        initial_traffic,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(initial_size)
            .with_min_inner_size(ui::MIN_WINDOW_SIZE)
            .with_decorations(false)
            .with_title(APP_NAME)
            .with_app_id("GXNUCampusLoginRS")
            .with_icon(window_icon()),
        ..Default::default()
    };

    match eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(ui::GuardApp::new(cc, deps)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("窗口启动失败：{err}");
            ExitCode::FAILURE
        }
    }
}

fn show_or_spawn_ui(
    ui_child: &Arc<Mutex<Option<Child>>>,
    endpoint: &ipc::DaemonEndpoint,
    ipc_server: &ipc::IpcServer,
    target: UiOpenTarget,
) -> Result<(), String> {
    let mut guard = ui_child.lock().map_err(|_| "无法访问界面进程状态")?;
    if let Some(child) = guard.as_mut() {
        match child.try_wait() {
            Ok(None) => {
                match target {
                    UiOpenTarget::AccountSettings => ipc_server.broadcast_open_account_settings(),
                    UiOpenTarget::UpdateModal => ipc_server.broadcast_open_update_modal(),
                    UiOpenTarget::Normal | UiOpenTarget::AfterUpdateToast => {}
                }
                bring_pid_window_to_front(child.id());
                return Ok(());
            }
            _ => {
                *guard = None;
            }
        }
    }

    let exe = std::env::current_exe().map_err(|e| format!("无法定位程序：{e}"))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "--ui",
        "--ipc-port",
        &endpoint.port.to_string(),
        "--ipc-token",
        &endpoint.token,
    ]);
    match target {
        UiOpenTarget::AccountSettings => {
            cmd.env("GXNU_VIEW", "settings")
                .env("GXNU_SETTINGS_TAB", "account");
        }
        UiOpenTarget::UpdateModal => {
            cmd.env("GXNU_OPEN_UPDATE", "1");
        }
        UiOpenTarget::AfterUpdateToast => {
            cmd.env("GXNU_JUST_UPDATED", "1");
        }
        UiOpenTarget::Normal => {}
    }
    let child = cmd.spawn().map_err(|e| format!("打开主界面失败：{e}"))?;
    *guard = Some(child);
    Ok(())
}

fn stop_ui_child(ui_child: &Arc<Mutex<Option<Child>>>) {
    let Ok(mut guard) = ui_child.lock() else {
        return;
    };
    let Some(mut child) = guard.take() else {
        return;
    };
    let deadline = std::time::Instant::now() + Duration::from_millis(600);
    while std::time::Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_win32_tray_loop(
    tray: &mut tray::TrayUi,
    mut fluent_menu: Option<&mut tray_menu::FluentTrayMenu>,
    mut fluent_flyout: Option<&mut tray_flyout::FluentTrayFlyout>,
    latest_snapshot: &Arc<Mutex<Option<models::MonitorSnapshot>>>,
    control: &monitor::MonitorControl,
    self_service_mgr: &Arc<self_service::SelfServiceManager>,
) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    let events = network_events::NetworkEvents::register(current_thread_id());
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if events.handle(&msg, control) {
                continue;
            }
            if msg.message == WM_TRAY_REFRESH {
                if let Some(snap) = latest_snapshot.lock().ok().and_then(|g| g.clone()) {
                    tray.apply(&snap);
                }
                if let Some(flyout) = fluent_flyout.as_deref_mut() {
                    flyout.refresh_if_visible();
                }
            } else if msg.message == WM_TRAY_SHOW_MENU {
                if let Some(flyout) = fluent_flyout.as_deref_mut() {
                    flyout.hide();
                }
                if let Some(menu) = fluent_menu.as_deref_mut() {
                    menu.show_at_cursor();
                }
            } else if msg.message == WM_TRAY_TOGGLE_FLYOUT {
                if let Some(menu) = fluent_menu.as_deref_mut() {
                    menu.hide();
                }
                if let Some(flyout) = fluent_flyout.as_deref_mut() {
                    if flyout.toggle_at_cursor() {
                        self_service_mgr.trigger(self_service::FetchTrigger::UiOpen);
                    }
                }
            } else if msg.message == WM_DAEMON_QUIT {
                break;
            } else {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

fn current_thread_id() -> u32 {
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    unsafe { GetCurrentThreadId() }
}

fn post_thread_msg(tid: u32, msg: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
    unsafe {
        PostThreadMessageW(tid, msg, 0, 0);
    }
}

fn bring_pid_window_to_front(target_pid: u32) {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow,
        SW_RESTORE, SW_SHOW,
    };

    struct Search {
        pid: u32,
        found: HWND,
    }

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam as *mut Search);
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != state.pid {
            return 1;
        }
        let mut buf = [0u16; 128];
        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if len > 0 && String::from_utf16_lossy(&buf[..len as usize]) == APP_NAME {
            state.found = hwnd;
            return 0;
        }
        1
    }

    let mut search = Search {
        pid: target_pid,
        found: std::ptr::null_mut(),
    };
    unsafe {
        EnumWindows(Some(enum_cb), (&mut search as *mut Search) as LPARAM);
        if !search.found.is_null() {
            ShowWindow(search.found, SW_SHOW);
            ShowWindow(search.found, SW_RESTORE);
            SetForegroundWindow(search.found);
        }
    }
}

fn bring_any_ui_window_to_front() {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    unsafe extern "system" fn enum_cb(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        let mut buf = [0u16; 128];
        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if len > 0 && String::from_utf16_lossy(&buf[..len as usize]) == APP_NAME {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
            return 0;
        }
        1
    }

    unsafe {
        EnumWindows(Some(enum_cb), 0);
    }
}

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

fn parse_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|idx| args.get(idx + 1))
        .cloned()
}

fn mask_id(student_id: &str) -> String {
    if student_id.is_empty() {
        return "(未设置)".to_string();
    }
    let head: String = student_id.chars().take(4).collect();
    format!("{head}***")
}

/// 柔和圆角矩形 Wi-Fi 品牌图标（与资源管理器 `.exe` 图标、主窗口左上角图标同源）。
fn window_icon() -> egui::IconData {
    const SIZE: u32 = 64;
    let rgba = app_icon::render_icon_rgba(None, SIZE);
    egui::IconData {
        rgba,
        width: SIZE,
        height: SIZE,
    }
}
