//! 本地回环 IPC（127.0.0.1 临时端口 + 随机会话令牌）：
//! 用于「轻守护进程（~5–8MB）」与「按需拉起的 GUI 进程」之间双向实时同步。

mod inbox;
pub use inbox::{CheckResult, ClientInbox, UpdateCheckResult, MAX_LOG_LINES};

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::models::{MonitorSnapshot, SelfServiceState, Settings, TrafficSummary, UpdateState};

const ENDPOINT_FILE: &str = "daemon.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonEndpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEnvelope {
    pub snapshot: MonitorSnapshot,
    pub online_since_unix: Option<i64>,
    pub last_check_unix: Option<i64>,
    #[serde(default)]
    pub traffic: TrafficSummary,
    #[serde(default)]
    pub active_account_id: String,
    #[serde(default)]
    pub self_service: SelfServiceState,
    #[serde(default)]
    pub update: UpdateState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonToUi {
    Snapshot(Box<SnapshotEnvelope>),
    LogLine {
        line: String,
    },
    CommandResult {
        id: u64,
        result: Result<(), String>,
    },
    KickResult {
        ip: String,
        is_local: bool,
        result: Result<(), String>,
    },
    UpdateState(Box<UpdateState>),
    OpenAccountSettings,
    OpenUpdateModal,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum UiToDaemon {
    ShowUi,
    Start,
    Pause,
    ForceCheck,
    RefreshSelfService {
        force: bool,
    },
    KickDevice {
        ip: String,
        is_local: bool,
    },
    CheckUpdate {
        manual: bool,
    },
    StartUpdate,
    ReloadSettings {
        start: bool,
        password_changed: bool,
        expected: Settings,
    },
    SwitchAccount {
        account_id: String,
        expected: Settings,
    },
    TestToast {
        body: String,
    },
    QuitForTakeover {
        new_version: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "cmd")]
enum ClientFrame {
    Hello { token: String },
    Request { id: u64, command: Box<UiToDaemon> },
}

pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
const WRITE_TIMEOUT: Duration = Duration::from_millis(750);
const CLIENT_QUEUE: usize = 64;

struct Subscriber {
    messages: mpsc::SyncSender<Arc<str>>,
    connection: TcpStream,
}

#[derive(Default)]
struct Subscribers {
    latest: Option<Arc<str>>,
    active: HashMap<u64, Subscriber>,
    next_id: u64,
    closed: bool,
}

impl Subscribers {
    fn subscribe(&mut self, connection: TcpStream) -> Option<(u64, Receiver<Arc<str>>)> {
        if self.closed {
            return None;
        }
        let (messages, receiver) = mpsc::sync_channel(CLIENT_QUEUE);
        if let Some(latest) = &self.latest {
            messages.try_send(Arc::clone(latest)).ok()?;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.active.insert(
            id,
            Subscriber {
                messages,
                connection,
            },
        );
        Some((id, receiver))
    }

    fn broadcast(&mut self, msg: DaemonToUi) {
        if self.closed {
            return;
        }
        let Ok(mut line) = serde_json::to_string(&msg) else {
            return;
        };
        line.push('\n');
        let payload: Arc<str> = line.into();
        if matches!(msg, DaemonToUi::Snapshot(_)) {
            self.latest = Some(Arc::clone(&payload));
        }
        self.active.retain(|_, subscriber| {
            if subscriber.messages.try_send(Arc::clone(&payload)).is_ok() {
                return true;
            }
            let _ = subscriber.connection.shutdown(Shutdown::Both);
            false
        });
        if matches!(msg, DaemonToUi::Shutdown) {
            self.closed = true;
            self.active.clear();
        }
    }
}

pub struct IpcServer {
    endpoint: DaemonEndpoint,
    endpoint_path: PathBuf,
    subscribers: Arc<Mutex<Subscribers>>,
}

impl IpcServer {
    pub fn bind(app_dir: &Path) -> Result<(Self, TcpListener), String> {
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|err| format!("绑定本地 IPC 失败：{err}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let mut random = [0u8; 32];
        getrandom::getrandom(&mut random).map_err(|e| format!("生成 IPC 会话令牌失败：{e}"))?;
        let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let endpoint = DaemonEndpoint {
            port,
            token: token.clone(),
            pid: std::process::id(),
            version: crate::VERSION.to_string(),
        };
        let endpoint_path = app_dir.join(ENDPOINT_FILE);
        let mut file = tempfile::NamedTempFile::new_in(app_dir).map_err(|e| e.to_string())?;
        serde_json::to_writer(file.as_file_mut(), &endpoint).map_err(|e| e.to_string())?;
        file.persist(&endpoint_path).map_err(|e| e.to_string())?;

        Ok((
            Self {
                endpoint,
                endpoint_path,
                subscribers: Arc::new(Mutex::new(Subscribers::default())),
            },
            listener,
        ))
    }

    pub fn listen<F>(&self, listener: TcpListener, on_command: F) -> Result<(), String>
    where
        F: Fn(UiToDaemon) -> Result<(), String> + Send + Sync + 'static,
    {
        let hub = Arc::clone(&self.subscribers);
        let token = self.endpoint.token.clone();
        let on_command = Arc::new(on_command);
        std::thread::Builder::new()
            .name("ipc-accept".into())
            .spawn(move || {
                for incoming in listener.incoming().flatten() {
                    if hub.lock().unwrap().closed {
                        break;
                    }
                    let _ = incoming.set_nodelay(true);
                    let clients = Arc::clone(&hub);
                    let cb = Arc::clone(&on_command);
                    let token = token.clone();
                    let _ =
                        std::thread::Builder::new()
                            .name("ipc-client".into())
                            .spawn(move || {
                                let _ = handle_client(incoming, &token, clients, cb);
                            });
                }
            })
            .map_err(|e| format!("启动 IPC 监听线程失败：{e}"))?;
        Ok(())
    }

    pub fn endpoint(&self) -> &DaemonEndpoint {
        &self.endpoint
    }

    pub fn broadcast_snapshot(&self, env: SnapshotEnvelope) {
        self.subscribers
            .lock()
            .unwrap()
            .broadcast(DaemonToUi::Snapshot(Box::new(env)));
    }

    pub fn broadcast_log(&self, line: String) {
        self.subscribers
            .lock()
            .unwrap()
            .broadcast(DaemonToUi::LogLine { line });
    }

    pub fn broadcast_open_account_settings(&self) {
        self.subscribers
            .lock()
            .unwrap()
            .broadcast(DaemonToUi::OpenAccountSettings);
    }

    pub fn broadcast_kick_result(&self, ip: String, is_local: bool, result: Result<(), String>) {
        self.subscribers
            .lock()
            .unwrap()
            .broadcast(DaemonToUi::KickResult {
                ip,
                is_local,
                result,
            });
    }

    pub fn broadcast_update_state(&self, state: UpdateState) {
        self.subscribers
            .lock()
            .unwrap()
            .broadcast(DaemonToUi::UpdateState(Box::new(state)));
    }

    pub fn broadcast_open_update_modal(&self) {
        self.subscribers
            .lock()
            .unwrap()
            .broadcast(DaemonToUi::OpenUpdateModal);
    }

    pub fn broadcast_shutdown(&self) {
        let mut hub = self.subscribers.lock().unwrap();
        if hub.closed {
            return;
        }
        hub.broadcast(DaemonToUi::Shutdown);
        drop(hub);
        let owns_endpoint_file = fs::read_to_string(&self.endpoint_path)
            .ok()
            .and_then(|text| serde_json::from_str::<DaemonEndpoint>(&text).ok())
            .is_none_or(|ep| ep.pid == self.endpoint.pid);
        if owns_endpoint_file {
            let _ = fs::remove_file(&self.endpoint_path);
        }
        let addr = ([127, 0, 0, 1], self.endpoint.port).into();
        let _ = TcpStream::connect_timeout(&addr, Duration::from_millis(200));
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.broadcast_shutdown();
    }
}

fn read_command(
    reader: &mut BufReader<TcpStream>,
    line: &mut String,
) -> io::Result<Option<ClientFrame>> {
    line.clear();
    const MAX_FRAME: u64 = 64 * 1024;
    let n = reader.by_ref().take(MAX_FRAME).read_line(line)?;
    if n == 0 {
        return Ok(None);
    }
    if !line.ends_with('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete IPC frame",
        ));
    }
    Ok(Some(serde_json::from_str(line.trim())?))
}

fn handle_client<F>(
    stream: TcpStream,
    expected_token: &str,
    hub: Arc<Mutex<Subscribers>>,
    on_command: Arc<F>,
) -> io::Result<()>
where
    F: Fn(UiToDaemon) -> Result<(), String> + Send + Sync + 'static,
{
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    match read_command(&mut reader, &mut line)? {
        Some(ClientFrame::Hello { token }) if token == expected_token => {}
        _ => return Ok(()),
    }
    reader.get_ref().set_read_timeout(None)?;
    let mut writer = reader.get_ref().try_clone()?;
    writer.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let Some((id, messages)) = hub.lock().unwrap().subscribe(writer.try_clone()?) else {
        return Ok(());
    };
    let send_thread = std::thread::Builder::new()
        .name("ipc-writer".into())
        .spawn(move || {
            for msg in messages {
                if writer.write_all(msg.as_bytes()).is_err() {
                    break;
                }
            }
            let _ = writer.shutdown(Shutdown::Both);
        });
    if send_thread.is_ok() {
        while let Ok(Some(ClientFrame::Request {
            id: request_id,
            command,
        })) = read_command(&mut reader, &mut line)
        {
            let result = on_command(*command);
            let mut line = serde_json::to_string(&DaemonToUi::CommandResult {
                id: request_id,
                result,
            })?;
            line.push('\n');
            let clients = hub.lock().unwrap();
            if !clients
                .active
                .get(&id)
                .is_some_and(|client| client.messages.try_send(line.into()).is_ok())
            {
                let _ = reader.get_ref().shutdown(Shutdown::Both);
                break;
            }
        }
    }
    hub.lock().unwrap().active.remove(&id);
    Ok(())
}

/// GUI 进程侧 IPC 客户端。
pub struct IpcClient {
    writer: TcpStream,
    next_id: u64,
    pub inbox: Arc<ClientInbox>,
}

impl IpcClient {
    pub fn connect(port: u16, token: &str) -> Result<Self, String> {
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
            .map_err(|err| format!("连接守护进程失败：{err}"))?;
        let _ = stream.set_nodelay(true);
        stream
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .map_err(|e| e.to_string())?;

        let hello = ClientFrame::Hello {
            token: token.to_string(),
        };
        let mut line = serde_json::to_string(&hello).map_err(|e| e.to_string())?;
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .map_err(|err| format!("握手失败：{err}"))?;

        let reader_stream = stream.try_clone().map_err(|err| err.to_string())?;
        let inbox = Arc::new(ClientInbox::default());
        let incoming = Arc::clone(&inbox);
        std::thread::Builder::new()
            .name("ipc-ui-reader".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(reader_stream);
                let mut buf = String::new();
                loop {
                    buf.clear();
                    match reader.read_line(&mut buf) {
                        Ok(0) | Err(_) => {
                            incoming.disconnect();
                            break;
                        }
                        Ok(_) => {
                            if let Ok(msg) = serde_json::from_str::<DaemonToUi>(&buf) {
                                let shutdown = matches!(msg, DaemonToUi::Shutdown);
                                incoming.receive(msg);
                                if shutdown {
                                    break;
                                }
                            }
                        }
                    }
                }
            })
            .map_err(|err| format!("启动 IPC 接收线程失败：{err}"))?;

        Ok(Self {
            writer: stream,
            next_id: 1,
            inbox,
        })
    }

    pub fn send(&mut self, command: UiToDaemon) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.inbox.reserve(id, &command)?;
        let frame = ClientFrame::Request {
            id,
            command: Box::new(command),
        };
        let sent = serde_json::to_vec(&frame)
            .map_err(io::Error::from)
            .and_then(|mut line| {
                line.push(b'\n');
                self.writer.write_all(&line)
            });
        if let Err(err) = sent {
            self.inbox.forget(id);
            if let ClientFrame::Request { command, .. } = &frame {
                if let UiToDaemon::KickDevice { ip, .. } = command.as_ref() {
                    self.inbox.forget_kick(ip);
                }
            }
            return Err(format!("发送守护命令失败：{err}"));
        }
        Ok(id)
    }
}

impl Drop for IpcClient {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExistingDaemonOutcome {
    Forwarded,
    Takeover {
        old_pid: u32,
        old_version: String,
        ipc_quit_sent: bool,
    },
}

pub(crate) fn should_takeover_daemon(
    current_version: &str,
    running_version: Option<&str>,
    endpoint_pid: u32,
    self_pid: u32,
    allow_takeover: bool,
) -> bool {
    if !allow_takeover || endpoint_pid == 0 || endpoint_pid == self_pid {
        return false;
    }
    running_version.is_some_and(|old| {
        crate::updater::compare_versions(current_version, old) == Some(std::cmp::Ordering::Greater)
    })
}

fn resolve_running_version(endpoint: &DaemonEndpoint, client: &IpcClient) -> Option<String> {
    let from_endpoint = endpoint.version.trim();
    if !from_endpoint.is_empty() {
        return Some(from_endpoint.to_string());
    }
    if let Some(ver) = crate::startup::process_exe_path(endpoint.pid)
        .and_then(|exe| crate::startup::read_exe_version(&exe))
    {
        return Some(ver);
    }
    let wait_until = Instant::now() + Duration::from_millis(200);
    loop {
        if let Some(snap) = client.inbox.take().snapshot {
            let ver = snap.update.current_version.trim();
            if !ver.is_empty() {
                return Some(ver.to_string());
            }
        }
        if Instant::now() >= wait_until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The instance mutex may precede publication of the endpoint. Retry discovery only,
/// never replay a command whose application is already possible.
pub fn coordinate_existing_daemon(
    app_dir: &Path,
    current_version: &str,
    allow_takeover: bool,
    start: bool,
    minimized: bool,
) -> Result<ExistingDaemonOutcome, String> {
    if !allow_takeover && minimized && !start {
        return Ok(ExistingDaemonOutcome::Forwarded);
    }
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let connect = || {
        let text = fs::read_to_string(app_dir.join(ENDPOINT_FILE)).map_err(|e| format!("{e}"))?;
        let endpoint: DaemonEndpoint = serde_json::from_str(&text).map_err(|e| format!("{e}"))?;
        let client = IpcClient::connect(endpoint.port, &endpoint.token)?;
        Ok::<_, String>((endpoint, client))
    };
    let (endpoint, mut client) = loop {
        match connect() {
            Ok(pair) => break pair,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                if minimized && !start {
                    return Ok(ExistingDaemonOutcome::Forwarded);
                }
                return Err(format!("无法连接已运行的守护进程：{err}"));
            }
        }
    };

    let running_version = resolve_running_version(&endpoint, &client);
    if should_takeover_daemon(
        current_version,
        running_version.as_deref(),
        endpoint.pid,
        std::process::id(),
        allow_takeover,
    ) {
        let old_version = running_version.unwrap_or_default();
        let mut ipc_quit_sent = false;
        if !endpoint.version.trim().is_empty() {
            if let Ok(req_id) = client.send(UiToDaemon::QuitForTakeover {
                new_version: current_version.to_string(),
            }) {
                match client.inbox.recv_reply_timeout(Duration::from_millis(1500)) {
                    Ok((reply_id, Ok(()))) if reply_id == req_id => ipc_quit_sent = true,
                    // 旧版明确拒绝接管（双方对版本高低判断不一致）：尊重它的决定，
                    // 不再用窗口消息强制关闭，改为只唤醒它的窗口。
                    Ok((reply_id, Err(_))) if reply_id == req_id => {
                        return forward_to_running(&mut client, start, minimized);
                    }
                    // 超时或连接中断：旧版很可能只是忙，交给窗口消息通道兜底通知。
                    _ => {}
                }
            }
        }
        drop(client);
        return Ok(ExistingDaemonOutcome::Takeover {
            old_pid: endpoint.pid,
            old_version,
            ipc_quit_sent,
        });
    }

    forward_to_running(&mut client, start, minimized)
}

fn forward_to_running(
    client: &mut IpcClient,
    start: bool,
    minimized: bool,
) -> Result<ExistingDaemonOutcome, String> {
    if minimized && !start {
        return Ok(ExistingDaemonOutcome::Forwarded);
    }
    let commands = [
        start.then_some(UiToDaemon::Start),
        (!minimized).then_some(UiToDaemon::ShowUi),
    ];
    for command in commands.into_iter().flatten() {
        let id = client.send(command)?;
        let (reply_id, result) = client
            .inbox
            .recv_reply_timeout(COMMAND_TIMEOUT)
            .map_err(|e| format!("未收到守护进程回执：{e}"))?;
        if reply_id != id {
            return Err("守护进程回执序号不匹配".into());
        }
        result?;
    }
    Ok(ExistingDaemonOutcome::Forwarded)
}

pub fn forward_launch(app_dir: &Path, start: bool, minimized: bool) -> Result<(), String> {
    coordinate_existing_daemon(app_dir, crate::VERSION, false, start, minimized).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{MonitorSnapshot, MonitorState};

    fn socket_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let first = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        (first, listener.accept().unwrap().0)
    }

    #[test]
    fn full_subscriber_queue_does_not_block_healthy_subscribers_or_shutdown() {
        let mut hub = Subscribers::default();
        let (slow, _peer1) = socket_pair();
        let (fast, _peer2) = socket_pair();
        let (slow_id, _unread) = hub.subscribe(slow).unwrap();
        let (_, healthy) = hub.subscribe(fast).unwrap();
        for n in 0..=CLIENT_QUEUE {
            hub.broadcast(DaemonToUi::LogLine {
                line: n.to_string(),
            });
            assert!(healthy.try_recv().is_ok());
        }
        assert!(!hub.active.contains_key(&slow_id));
        hub.broadcast(DaemonToUi::Shutdown);
        assert!(matches!(
            serde_json::from_str(&healthy.try_recv().unwrap()),
            Ok(DaemonToUi::Shutdown)
        ));
    }

    #[test]
    fn subscriber_receives_latest_snapshot_then_live_events_over_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, commands) = mpsc::channel();
        let (server, listener) = IpcServer::bind(dir.path()).unwrap();
        server
            .listen(listener, move |cmd| {
                tx.send(cmd).map_err(|e| format!("{e}"))
            })
            .unwrap();
        server.broadcast_snapshot(SnapshotEnvelope {
            snapshot: MonitorSnapshot::new(MonitorState::Paused, None, "paused", false),
            online_since_unix: None,
            last_check_unix: None,
            traffic: TrafficSummary::default(),
            active_account_id: String::new(),
            self_service: SelfServiceState::default(),
            update: UpdateState::default(),
        });
        let ep = server.endpoint();
        let mut client = IpcClient::connect(ep.port, &ep.token).unwrap();
        let id = client.send(UiToDaemon::ForceCheck).unwrap();
        assert_eq!(
            client.inbox.recv_reply_timeout(COMMAND_TIMEOUT).unwrap(),
            (id, Ok(()))
        );
        assert!(matches!(
            commands.recv_timeout(Duration::from_secs(2)),
            Ok(UiToDaemon::ForceCheck)
        ));
        server.broadcast_log("test line".into());
        // The second reply follows both broadcasts on this subscriber's FIFO stream.
        let id = client.send(UiToDaemon::ShowUi).unwrap();
        assert_eq!(
            client.inbox.recv_reply_timeout(COMMAND_TIMEOUT).unwrap(),
            (id, Ok(()))
        );
        let batch = client.inbox.take();
        assert_eq!(batch.snapshot.unwrap().snapshot.state, MonitorState::Paused);
        assert_eq!(batch.logs, ["test line"]);
        server.broadcast_shutdown();
        assert_eq!(
            client.inbox.recv_reply_timeout(COMMAND_TIMEOUT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        assert!(client.inbox.daemon_closed());
        assert!(!dir.path().join(ENDPOINT_FILE).exists());
    }
    #[test]
    fn replies_follow_application_and_reach_only_the_authenticated_requester() {
        let dir = tempfile::tempdir().unwrap();
        let (server, listener) = IpcServer::bind(dir.path()).unwrap();
        let (entered, applying) = mpsc::channel();
        let (finish, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        server
            .listen(listener, move |_| {
                entered.send(()).unwrap();
                wait.lock().unwrap().recv_timeout(COMMAND_TIMEOUT).unwrap();
                Err("模拟热加载失败".into())
            })
            .unwrap();
        let ep = server.endpoint();
        let mut wrong = IpcClient::connect(ep.port, "fake-invalid-token").unwrap();
        let _ = wrong.send(UiToDaemon::Start);
        assert!(matches!(
            wrong.inbox.recv_reply_timeout(COMMAND_TIMEOUT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        assert!(applying.try_recv().is_err());
        let mut first = IpcClient::connect(ep.port, &ep.token).unwrap();
        let mut second = IpcClient::connect(ep.port, &ep.token).unwrap();
        let id = first
            .send(UiToDaemon::ReloadSettings {
                start: false,
                password_changed: false,
                expected: Settings::default(),
            })
            .unwrap();
        applying.recv_timeout(COMMAND_TIMEOUT).unwrap();
        assert!(first.inbox.take().replies.is_empty());
        finish.send(()).unwrap();
        assert_eq!(
            first.inbox.recv_reply_timeout(COMMAND_TIMEOUT).unwrap(),
            (id, Err("模拟热加载失败".into()))
        );
        let second_id = second.send(UiToDaemon::Start).unwrap();
        assert_eq!(id, second_id);
        applying.recv_timeout(COMMAND_TIMEOUT).unwrap();
        assert!(second.inbox.take().replies.is_empty());
        finish.send(()).unwrap();
        assert_eq!(
            second.inbox.recv_reply_timeout(COMMAND_TIMEOUT).unwrap().0,
            second_id
        );
        assert!(first.inbox.take().replies.is_empty());
    }

    #[test]
    fn suspended_ui_resumes_with_bounded_history_logs_and_original_manual_results() {
        use crate::metrics_store::{SampleId, TelemetryPoint, MAX_STORED_POINTS};
        use crate::models::{DisconnectReason, OnlineDevice, SelfServiceData, UpdateStatus};
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = IpcClient::connect(listener.local_addr().unwrap().port(), "test").unwrap();
        let (peer, _) = listener.accept().unwrap();
        let mut writer = io::BufWriter::new(peer);
        let mut emit = |msg| {
            serde_json::to_writer(&mut writer, &msg).unwrap();
            writer.write_all(b"\n").unwrap();
        };
        let manual = client.send(UiToDaemon::ForceCheck).unwrap();
        let update = client
            .send(UiToDaemon::CheckUpdate { manual: true })
            .unwrap();
        let kicked_ip = "192.0.2.1";
        let kick = client
            .send(UiToDaemon::KickDevice {
                ip: kicked_ip.into(),
                is_local: true,
            })
            .unwrap();
        let snapshot = |seq: u64, state: MonitorState, ms| {
            let offline = state == MonitorState::Offline;
            let reason = if offline {
                DisconnectReason::LoginRejected
            } else {
                DisconnectReason::Online
            };
            let mut snapshot = MonitorSnapshot::new(state, Some(reason), "sample", true);
            snapshot.metrics.check_seq = seq;
            let mut point = TelemetryPoint::check(seq as i64, ms, !offline, reason.as_str());
            point.sample_id = Some(SampleId::generated(1, 2, seq));
            snapshot.metrics.last_point = Some(point);
            DaemonToUi::Snapshot(Box::new(SnapshotEnvelope {
                snapshot,
                online_since_unix: Some(1),
                last_check_unix: Some(seq as i64),
                traffic: TrafficSummary::default(),
                active_account_id: "mock".into(),
                self_service: SelfServiceState {
                    data: Some(SelfServiceData {
                        online_devices: vec![OnlineDevice {
                            use_time_secs: Some(seq),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                update: UpdateState::default(),
            }))
        };
        let update_msg = |status, downloaded_bytes, message: &str| {
            DaemonToUi::UpdateState(Box::new(UpdateState {
                status,
                downloaded_bytes,
                message: message.into(),
                ..Default::default()
            }))
        };
        emit(snapshot(1, MonitorState::Checking, 0));
        emit(snapshot(1, MonitorState::Offline, 0));
        emit(update_msg(UpdateStatus::Checking, 0, ""));
        emit(update_msg(UpdateStatus::Error, 0, "mock check error"));
        emit(update_msg(UpdateStatus::UpToDate, 0, ""));
        emit(DaemonToUi::KickResult {
            ip: kicked_ip.into(),
            is_local: true,
            result: Ok(()),
        });
        let total = MAX_STORED_POINTS as u64 + 257;
        for n in 1..=total {
            emit(snapshot(n, MonitorState::Online, n));
            emit(DaemonToUi::LogLine {
                line: format!("line {n}"),
            });
            emit(update_msg(UpdateStatus::Downloading, n, ""));
            emit(DaemonToUi::KickResult {
                ip: format!("mock-{n}"),
                is_local: false,
                result: Ok(()),
            });
        }
        // Late latency for an older sample updates its slot. New auxiliary data can carry this older reading.
        emit(snapshot(total - 100, MonitorState::Online, 9999));
        for id in [manual, update, kick] {
            emit(DaemonToUi::CommandResult { id, result: Ok(()) });
        }
        writer.flush().unwrap();
        for id in [manual, update, kick] {
            assert_eq!(
                client.inbox.recv_reply_timeout(COMMAND_TIMEOUT).unwrap(),
                (id, Ok(()))
            );
        }
        let batch = client.inbox.take();
        let env = batch.snapshot.unwrap();
        assert_eq!(env.snapshot.metrics.check_seq, total);
        assert_eq!(env.last_check_unix, Some(total as i64));
        assert_eq!(
            env.self_service.data.unwrap().online_devices[0].use_time_secs,
            Some(total - 100)
        );
        let samples = batch.samples.into_points();
        assert_eq!(samples.len(), MAX_STORED_POINTS);
        for (point, seq) in samples
            .iter()
            .zip(total - MAX_STORED_POINTS as u64 + 1..=total)
        {
            assert_eq!(point.sample_id, Some(SampleId::generated(1, 2, seq)));
            assert_eq!(point.ms, if seq == total - 100 { 9999 } else { seq });
        }
        assert_eq!(batch.logs.len(), MAX_LOG_LINES);
        assert!(batch
            .logs
            .into_iter()
            .eq((total - MAX_LOG_LINES as u64 + 1..=total).map(|n| format!("line {n}"))));
        assert!(batch.prompt_account);
        assert_eq!(
            batch.manual_check,
            Some(CheckResult::Status(Some(DisconnectReason::LoginRejected)))
        );
        assert_eq!(
            batch.manual_update,
            Some(UpdateCheckResult::Error("mock check error".into()))
        );
        let latest = batch.update.unwrap();
        assert_eq!(
            (latest.status, latest.downloaded_bytes),
            (UpdateStatus::Downloading, total)
        );
        assert_eq!(batch.kick_results.len(), CLIENT_QUEUE);
        assert_eq!(batch.kick_results[0], (kicked_ip.into(), true, Ok(())));
        assert_eq!(
            batch.kick_results.back().unwrap().0,
            format!("mock-{total}")
        );
        let empty = client.inbox.take();
        assert!(
            !empty.prompt_account && empty.manual_check.is_none() && empty.manual_update.is_none()
        );
        assert!(
            empty.snapshot.is_none()
                && empty.samples.into_points().is_empty()
                && empty.logs.is_empty()
        );
    }

    #[test]
    fn coordinates_takeover_for_newer_version_and_forwards_for_same_or_older() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, commands) = mpsc::channel();
        let (server, listener) = IpcServer::bind(dir.path()).unwrap();
        server
            .listen(listener, move |cmd| {
                // 模拟一个对版本高低判断不同的旧版：拒绝 v0.3.0 的接管请求
                let refuse = matches!(
                    &cmd,
                    UiToDaemon::QuitForTakeover { new_version } if new_version == "0.3.0"
                );
                tx.send(cmd).map_err(|e| format!("{e}"))?;
                if refuse {
                    Err("拒绝非高版本接管请求".into())
                } else {
                    Ok(())
                }
            })
            .unwrap();
        server.broadcast_snapshot(SnapshotEnvelope {
            snapshot: MonitorSnapshot::new(MonitorState::Online, None, "ok", true),
            online_since_unix: None,
            last_check_unix: None,
            traffic: TrafficSummary::default(),
            active_account_id: String::new(),
            self_service: SelfServiceState::default(),
            update: UpdateState {
                current_version: "0.2.6".into(),
                ..Default::default()
            },
        });

        let write_endpoint = |version: &str| {
            let mut ep = server.endpoint().clone();
            ep.pid = u32::MAX;
            ep.version = version.to_string();
            fs::write(
                dir.path().join(ENDPOINT_FILE),
                serde_json::to_vec(&ep).unwrap(),
            )
            .unwrap();
        };

        // 1) 同版本或旧版本启动：仅转发唤醒，不触发接管
        write_endpoint("0.2.7");
        for caller_ver in ["0.2.7", "0.2.6"] {
            let outcome =
                coordinate_existing_daemon(dir.path(), caller_ver, true, false, false).unwrap();
            assert_eq!(outcome, ExistingDaemonOutcome::Forwarded);
            assert!(matches!(
                commands.recv_timeout(Duration::from_secs(1)).unwrap(),
                UiToDaemon::ShowUi
            ));
        }

        // 2) 新版本面对 0.2.7+ 守护进程：通过 IPC 发送 QuitForTakeover 并确认回执
        let outcome = coordinate_existing_daemon(dir.path(), "0.2.8", true, false, false).unwrap();
        assert_eq!(
            outcome,
            ExistingDaemonOutcome::Takeover {
                old_pid: u32::MAX,
                old_version: "0.2.7".into(),
                ipc_quit_sent: true,
            }
        );
        assert!(matches!(
            commands.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiToDaemon::QuitForTakeover { new_version } if new_version == "0.2.8"
        ));

        // 3) 新版本面对无 version 字段的旧版（如 v0.2.6）：从初始快照识别版本并走旧版窗口退出通道
        write_endpoint("");
        let outcome = coordinate_existing_daemon(dir.path(), "0.2.7", true, false, false).unwrap();
        assert_eq!(
            outcome,
            ExistingDaemonOutcome::Takeover {
                old_pid: u32::MAX,
                old_version: "0.2.6".into(),
                ipc_quit_sent: false,
            }
        );
        assert!(commands.try_recv().is_err());

        // 4) 旧版明确拒绝接管：放弃接管、不走窗口消息强退，改为唤醒旧版窗口
        write_endpoint("0.2.7");
        let outcome = coordinate_existing_daemon(dir.path(), "0.3.0", true, false, false).unwrap();
        assert_eq!(outcome, ExistingDaemonOutcome::Forwarded);
        assert!(matches!(
            commands.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiToDaemon::QuitForTakeover { new_version } if new_version == "0.3.0"
        ));
        assert!(matches!(
            commands.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiToDaemon::ShowUi
        ));

        // 5) 旧版拒绝且是开机自启的静默启动：不弹窗也不接管，直接安静退出
        let outcome = coordinate_existing_daemon(dir.path(), "0.3.0", true, false, true).unwrap();
        assert_eq!(outcome, ExistingDaemonOutcome::Forwarded);
        assert!(matches!(
            commands.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiToDaemon::QuitForTakeover { .. }
        ));
        assert!(commands.try_recv().is_err());
    }
}
