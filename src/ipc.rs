//! 本地回环 IPC（127.0.0.1 临时端口 + 随机会话令牌）：
//! 用于「轻守护进程（~5–8MB）」与「按需拉起的 GUI 进程」之间双向实时同步。

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "cmd")]
enum ClientFrame {
    Hello { token: String },
    Request { id: u64, command: UiToDaemon },
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
        let _ = fs::remove_file(&self.endpoint_path);
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
            let result = on_command(command);
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
    pub replies: Receiver<(u64, Result<(), String>)>,
    pub kick_results: Receiver<(String, bool, Result<(), String>)>,
    pub update_states: Receiver<UpdateState>,
    pub daemon_closed: Arc<AtomicBool>,
    pub connection_lost: Arc<AtomicBool>,
    pub open_account_settings: Arc<AtomicBool>,
    pub open_update_modal: Arc<AtomicBool>,
}

impl IpcClient {
    pub fn connect(
        port: u16,
        token: &str,
    ) -> Result<(Self, Receiver<SnapshotEnvelope>, Receiver<String>), String> {
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
        let (snap_tx, snap_rx) = mpsc::channel::<SnapshotEnvelope>();
        let (log_tx, log_rx) = mpsc::channel::<String>();
        let daemon_closed = Arc::new(AtomicBool::new(false));
        let open_account_settings = Arc::new(AtomicBool::new(false));
        let open_update_modal = Arc::new(AtomicBool::new(false));
        let (reply_tx, replies) = mpsc::channel();
        let (kick_tx, kick_results) = mpsc::channel();
        let (upd_tx, update_states) = mpsc::channel();
        let connection_lost = Arc::new(AtomicBool::new(false));
        let lost_ref = Arc::clone(&connection_lost);
        let closed_ref = Arc::clone(&daemon_closed);
        let open_acct_ref = Arc::clone(&open_account_settings);
        let open_upd_ref = Arc::clone(&open_update_modal);

        std::thread::Builder::new()
            .name("ipc-ui-reader".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(reader_stream);
                let mut buf = String::new();
                loop {
                    buf.clear();
                    match reader.read_line(&mut buf) {
                        Ok(0) | Err(_) => {
                            lost_ref.store(true, Ordering::Release);
                            break;
                        }
                        Ok(_) => {
                            if let Ok(msg) = serde_json::from_str::<DaemonToUi>(buf.trim()) {
                                match msg {
                                    DaemonToUi::Snapshot(env) => {
                                        let _ = snap_tx.send(*env);
                                    }
                                    DaemonToUi::LogLine { line } => {
                                        let _ = log_tx.send(line);
                                    }
                                    DaemonToUi::CommandResult { id, result } => {
                                        let _ = reply_tx.send((id, result));
                                    }
                                    DaemonToUi::KickResult {
                                        ip,
                                        is_local,
                                        result,
                                    } => {
                                        let _ = kick_tx.send((ip, is_local, result));
                                    }
                                    DaemonToUi::UpdateState(st) => {
                                        let _ = upd_tx.send(*st);
                                    }
                                    DaemonToUi::OpenAccountSettings => {
                                        open_acct_ref.store(true, Ordering::Release);
                                    }
                                    DaemonToUi::OpenUpdateModal => {
                                        open_upd_ref.store(true, Ordering::Release);
                                    }
                                    DaemonToUi::Shutdown => {
                                        closed_ref.store(true, Ordering::SeqCst);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .map_err(|err| format!("启动 IPC 接收线程失败：{err}"))?;

        Ok((
            Self {
                writer: stream,
                next_id: 1,
                replies,
                kick_results,
                update_states,
                daemon_closed,
                connection_lost,
                open_account_settings,
                open_update_modal,
            },
            snap_rx,
            log_rx,
        ))
    }

    pub fn send(&mut self, command: UiToDaemon) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = serde_json::to_vec(&ClientFrame::Request { id, command })
            .map_err(|e| format!("{e}"))?;
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .map_err(|e| format!("发送守护命令失败：{e}"))?;
        Ok(id)
    }
}

impl Drop for IpcClient {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

/// The instance mutex may precede publication of the endpoint. Retry discovery only,
/// never replay a command whose application is already possible.
pub fn forward_launch(app_dir: &Path, start: bool, minimized: bool) -> Result<(), String> {
    if minimized && !start {
        return Ok(());
    }
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let connect = || {
        let text = fs::read_to_string(app_dir.join(ENDPOINT_FILE)).map_err(|e| format!("{e}"))?;
        let endpoint: DaemonEndpoint = serde_json::from_str(&text).map_err(|e| format!("{e}"))?;
        IpcClient::connect(endpoint.port, &endpoint.token)
    };
    let (mut client, _snapshots, _logs) = loop {
        match connect() {
            Ok(connection) => break connection,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => return Err(format!("无法连接已运行的守护进程：{err}")),
        }
    };
    let commands = [
        start.then_some(UiToDaemon::Start),
        (!minimized).then_some(UiToDaemon::ShowUi),
    ];
    for command in commands.into_iter().flatten() {
        let id = client.send(command)?;
        let (reply_id, result) = client
            .replies
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|e| format!("未收到守护进程回执：{e}"))?;
        if reply_id != id {
            return Err("守护进程回执序号不匹配".into());
        }
        result?;
    }
    Ok(())
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
        let (mut client, snapshots, logs) = IpcClient::connect(ep.port, &ep.token).unwrap();
        assert_eq!(
            snapshots
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .snapshot
                .state,
            MonitorState::Paused
        );
        let id = client.send(UiToDaemon::ForceCheck).unwrap();
        assert_eq!(
            client.replies.recv_timeout(COMMAND_TIMEOUT).unwrap(),
            (id, Ok(()))
        );
        assert!(matches!(
            commands.recv_timeout(Duration::from_secs(2)),
            Ok(UiToDaemon::ForceCheck)
        ));
        server.broadcast_log("test line".into());
        assert_eq!(
            logs.recv_timeout(Duration::from_secs(2)).unwrap(),
            "test line"
        );
        server.broadcast_shutdown();
        assert!(logs.recv_timeout(Duration::from_secs(2)).is_err());
        assert!(client.daemon_closed.load(Ordering::SeqCst));
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
        let (mut wrong, _, _) = IpcClient::connect(ep.port, "fake-invalid-token").unwrap();
        let _ = wrong.send(UiToDaemon::Start);
        assert!(matches!(
            wrong.replies.recv_timeout(COMMAND_TIMEOUT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        assert!(applying.try_recv().is_err());
        let (mut first, _s1, _l1) = IpcClient::connect(ep.port, &ep.token).unwrap();
        let (mut second, _s2, _l2) = IpcClient::connect(ep.port, &ep.token).unwrap();
        let id = first
            .send(UiToDaemon::ReloadSettings {
                start: false,
                password_changed: false,
                expected: Settings::default(),
            })
            .unwrap();
        applying.recv_timeout(COMMAND_TIMEOUT).unwrap();
        assert!(first.replies.try_recv().is_err());
        finish.send(()).unwrap();
        assert_eq!(
            first.replies.recv_timeout(COMMAND_TIMEOUT).unwrap(),
            (id, Err("模拟热加载失败".into()))
        );
        let second_id = second.send(UiToDaemon::Start).unwrap();
        assert_eq!(id, second_id);
        applying.recv_timeout(COMMAND_TIMEOUT).unwrap();
        assert!(second.replies.try_recv().is_err());
        finish.send(()).unwrap();
        assert_eq!(
            second.replies.recv_timeout(COMMAND_TIMEOUT).unwrap().0,
            second_id
        );
        assert!(first.replies.try_recv().is_err());
    }
}
