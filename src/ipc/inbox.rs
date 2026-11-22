use std::collections::{HashSet, VecDeque};
use std::sync::{mpsc::RecvTimeoutError, Arc, Condvar, Mutex};
use std::time::Duration;

use super::{DaemonToUi, SnapshotEnvelope, UiToDaemon, CLIENT_QUEUE};
use crate::metrics_store::SampleHistory;
use crate::models::{DisconnectReason, MonitorSnapshot, MonitorState, UpdateState, UpdateStatus};

pub const MAX_LOG_LINES: usize = 4000;
type Reply = (u64, Result<(), String>);
type KickResult = (String, bool, Result<(), String>);

#[derive(Debug, PartialEq, Eq)]
pub enum CheckResult {
    Message(String),
    Status(Option<DisconnectReason>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateCheckResult {
    Available,
    UpToDate,
    Error(String),
}

#[derive(Default)]
pub struct UiBatch {
    pub snapshot: Option<Box<SnapshotEnvelope>>,
    pub samples: SampleHistory,
    pub logs: VecDeque<String>,
    pub replies: VecDeque<Reply>,
    pub kick_results: VecDeque<KickResult>,
    pub update: Option<UpdateState>,
    pub manual_check: Option<CheckResult>,
    pub manual_update: Option<UpdateCheckResult>,
    pub prompt_account: bool,
    pub open_account_settings: bool,
    pub open_update_modal: bool,
    pub connection_lost: bool,
}

#[derive(Default)]
struct Incoming {
    unread: UiBatch,
    awaiting: HashSet<u64>,
    requested_kicks: HashSet<String>,
    highest_check: u64,
    manual_check_after: Option<u64>,
    waiting_for_update: bool,
    account_alert_opened: bool,
    disconnected: bool,
    shutdown: bool,
}

impl Incoming {
    fn snapshot(&mut self, mut env: Box<SnapshotEnvelope>) {
        let snapshot = &env.snapshot;
        let seq = snapshot.metrics.check_seq;
        if let Some(point) = &snapshot.metrics.last_point {
            self.unread.samples.push(point.clone());
        }
        if seq >= self.highest_check {
            self.highest_check = seq;
            self.unread.prompt_account |= prompt_account(snapshot, &mut self.account_alert_opened);
            if self.manual_check_after.is_some_and(|after| seq > after)
                && snapshot.state != MonitorState::Checking
            {
                self.manual_check_after = None;
                self.unread.manual_check = Some(
                    if snapshot.metrics.last_point.is_none() && snapshot.reason.is_none() {
                        CheckResult::Message(snapshot.message.clone())
                    } else {
                        CheckResult::Status(snapshot.reason)
                    },
                );
            }
        } else if let Some(previous) = self.unread.snapshot.as_mut() {
            // Traffic and self-service can publish a new envelope carrying an older monitor reading.
            if previous.snapshot.metrics.check_seq > seq {
                std::mem::swap(&mut env.snapshot, &mut previous.snapshot);
                env.online_since_unix = previous.online_since_unix;
                env.last_check_unix = previous.last_check_unix;
            }
        }
        if env.update.status != UpdateStatus::Idle {
            self.unread.update = None;
        } else if self.unread.update.is_none() {
            if let Some(previous) = self.unread.snapshot.as_mut() {
                if previous.update.status != UpdateStatus::Idle {
                    self.unread.update = Some(std::mem::take(&mut previous.update));
                }
            }
        }
        self.unread.snapshot = Some(env);
    }

    fn update(&mut self, state: UpdateState) {
        if self.waiting_for_update {
            let outcome = match state.status {
                UpdateStatus::Available => Some(UpdateCheckResult::Available),
                UpdateStatus::UpToDate => Some(UpdateCheckResult::UpToDate),
                UpdateStatus::Error => Some(UpdateCheckResult::Error(state.message.clone())),
                _ => None,
            };
            if outcome.is_some() {
                self.waiting_for_update = false;
                self.unread.manual_update = outcome;
            }
        }
        self.unread.update = Some(state);
    }

    fn kick_result(&mut self, result: KickResult) {
        let queue = &mut self.unread.kick_results;
        if let Some(index) = queue.iter().position(|(ip, ..)| ip == &result.0) {
            queue.remove(index);
        } else if queue.len() == CLIENT_QUEUE {
            // Other clients' broadcast results must leave room for this window's pending kicks.
            let Some(index) = queue
                .iter()
                .position(|(ip, ..)| !self.requested_kicks.contains(ip))
            else {
                return;
            };
            queue.remove(index);
        }
        queue.push_back(result);
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

#[derive(Default)]
pub struct ClientInbox {
    state: Mutex<Incoming>,
    reply_ready: Condvar,
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl ClientInbox {
    /// 注册 UI 立即重绘唤醒器（当更新状态、命令回执或弹窗指令到达时立即唤醒界面，消除 250ms 轮询延迟）。
    pub fn set_waker(&self, waker: Arc<dyn Fn() + Send + Sync>) {
        *self.waker.lock().unwrap() = Some(waker);
    }

    fn wake(&self) {
        let cb = self.waker.lock().unwrap().clone();
        if let Some(cb) = cb {
            cb();
        }
    }
    pub(super) fn reserve(&self, id: u64, command: &UiToDaemon) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.disconnected || state.shutdown {
            return Err("守护进程连接已断开".into());
        }
        if state.awaiting.len() + state.unread.replies.len() == CLIENT_QUEUE {
            return Err("待处理的守护命令过多".into());
        }
        match command {
            UiToDaemon::ForceCheck => {
                state.manual_check_after = Some(state.highest_check);
                state.unread.manual_check = None;
            }
            UiToDaemon::CheckUpdate { manual: true } => {
                state.waiting_for_update = true;
                state.unread.manual_update = None;
            }
            UiToDaemon::KickDevice { ip, .. } => {
                if state.requested_kicks.len() == CLIENT_QUEUE {
                    return Err("待处理的注销请求过多".into());
                }
                state.requested_kicks.insert(ip.clone());
            }
            _ => {}
        }
        state.awaiting.insert(id);
        Ok(())
    }

    pub fn forget(&self, id: u64) {
        self.state.lock().unwrap().awaiting.remove(&id);
    }

    pub fn forget_kick(&self, ip: &str) {
        self.state.lock().unwrap().requested_kicks.remove(ip);
    }

    pub fn take(&self) -> UiBatch {
        let mut state = self.state.lock().unwrap();
        let mut batch = std::mem::take(&mut state.unread);
        batch.connection_lost = state.disconnected;
        for (ip, ..) in &batch.kick_results {
            state.requested_kicks.remove(ip);
        }
        batch
    }

    pub(super) fn receive(&self, msg: DaemonToUi) {
        let should_wake = {
            let mut state = self.state.lock().unwrap();
            let had_manual_check = state.unread.manual_check.is_some();
            match msg {
                DaemonToUi::Snapshot(env) => {
                    state.snapshot(env);
                    !had_manual_check && state.unread.manual_check.is_some()
                }
                DaemonToUi::LogLine { line } => {
                    if state.unread.logs.len() == MAX_LOG_LINES {
                        state.unread.logs.pop_front();
                    }
                    state.unread.logs.push_back(line);
                    false
                }
                DaemonToUi::CommandResult { id, result } => {
                    if state.awaiting.remove(&id) {
                        state.unread.replies.push_back((id, result));
                        self.reply_ready.notify_one();
                        true
                    } else {
                        false
                    }
                }
                DaemonToUi::KickResult {
                    ip,
                    is_local,
                    result,
                } => {
                    state.kick_result((ip, is_local, result));
                    true
                }
                DaemonToUi::UpdateState(update) => {
                    state.update(*update);
                    true
                }
                DaemonToUi::OpenAccountSettings => {
                    state.unread.open_account_settings = true;
                    true
                }
                DaemonToUi::OpenUpdateModal => {
                    state.unread.open_update_modal = true;
                    true
                }
                DaemonToUi::Shutdown => {
                    state.shutdown = true;
                    self.reply_ready.notify_all();
                    true
                }
            }
        };
        if should_wake {
            self.wake();
        }
    }

    pub(super) fn disconnect(&self) {
        self.state.lock().unwrap().disconnected = true;
        self.reply_ready.notify_all();
        self.wake();
    }

    pub fn connection_lost(&self) -> bool {
        self.state.lock().unwrap().disconnected
    }

    pub fn daemon_closed(&self) -> bool {
        self.state.lock().unwrap().shutdown
    }

    pub fn recv_reply_timeout(&self, timeout: Duration) -> Result<Reply, RecvTimeoutError> {
        let (mut state, _) = self
            .reply_ready
            .wait_timeout_while(self.state.lock().unwrap(), timeout, |s| {
                s.unread.replies.is_empty() && !s.disconnected && !s.shutdown
            })
            .unwrap();
        if let Some(reply) = state.unread.replies.pop_front() {
            Ok(reply)
        } else if state.disconnected || state.shutdown {
            Err(RecvTimeoutError::Disconnected)
        } else {
            Err(RecvTimeoutError::Timeout)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(seq: u64, state: MonitorState) -> DaemonToUi {
        let reason =
            (state == MonitorState::Offline).then_some(DisconnectReason::CredentialsMissing);
        let mut snapshot = MonitorSnapshot::new(state, reason, "检测结果", true);
        snapshot.metrics.check_seq = seq;
        DaemonToUi::Snapshot(Box::new(SnapshotEnvelope {
            snapshot,
            online_since_unix: None,
            last_check_unix: None,
            traffic: Default::default(),
            active_account_id: String::new(),
            self_service: Default::default(),
            update: Default::default(),
        }))
    }

    #[test]
    fn manual_baseline_and_account_alert_latch_survive_drains_and_stale_snapshots() {
        let inbox = ClientInbox::default();
        inbox.receive(snapshot(8, MonitorState::Offline));
        assert!(inbox.take().prompt_account);
        inbox.reserve(1, &UiToDaemon::ForceCheck).unwrap();
        inbox.receive(snapshot(7, MonitorState::Online));
        inbox.receive(snapshot(8, MonitorState::Checking));
        inbox.receive(snapshot(8, MonitorState::Offline));
        let batch = inbox.take();
        assert!(!batch.prompt_account);
        assert_eq!(batch.manual_check, None);
        inbox.receive(snapshot(9, MonitorState::Checking));
        assert_eq!(inbox.take().manual_check, None);
        inbox.receive(snapshot(9, MonitorState::Paused));
        inbox.receive(snapshot(9, MonitorState::Offline));
        inbox.receive(snapshot(10, MonitorState::Checking));
        let batch = inbox.take();
        assert!(batch.prompt_account);
        assert_eq!(
            batch.manual_check,
            Some(CheckResult::Message("检测结果".into()))
        );
        assert_eq!(batch.snapshot.unwrap().snapshot.metrics.check_seq, 10);
        inbox.receive(snapshot(9, MonitorState::Stopped));
        inbox.receive(snapshot(11, MonitorState::Offline));
        assert!(!inbox.take().prompt_account);
    }

    #[test]
    fn manual_update_retains_first_terminal_and_progress_follows_wire_order() {
        let inbox = ClientInbox::default();
        for (id, status, expected) in [
            (1, UpdateStatus::Available, UpdateCheckResult::Available),
            (2, UpdateStatus::UpToDate, UpdateCheckResult::UpToDate),
        ] {
            inbox
                .reserve(id, &UiToDaemon::CheckUpdate { manual: true })
                .unwrap();
            for status in [
                UpdateStatus::Checking,
                UpdateStatus::Idle,
                status,
                UpdateStatus::Downloading,
            ] {
                inbox.receive(DaemonToUi::UpdateState(Box::new(UpdateState {
                    status,
                    ..Default::default()
                })));
            }
            let batch = inbox.take();
            assert_eq!(batch.manual_update, Some(expected));
            assert_eq!(batch.update.unwrap().status, UpdateStatus::Downloading);
        }
        let DaemonToUi::Snapshot(mut env) = snapshot(1, MonitorState::Online) else {
            unreachable!()
        };
        env.update.status = UpdateStatus::Applying;
        inbox.receive(DaemonToUi::UpdateState(Box::new(UpdateState {
            status: UpdateStatus::Downloading,
            ..Default::default()
        })));
        inbox.receive(DaemonToUi::Snapshot(env));
        inbox.receive(snapshot(1, MonitorState::Online)); // An idle snapshot preserves the last active state.
        assert_eq!(inbox.take().update.unwrap().status, UpdateStatus::Applying);
    }

    #[test]
    fn reply_slots_bound_waiting_and_unread_commands_and_reopen_on_consumption() {
        let inbox = ClientInbox::default();
        for id in 0..CLIENT_QUEUE as u64 {
            inbox.reserve(id, &UiToDaemon::Pause).unwrap();
        }
        assert!(inbox.reserve(100, &UiToDaemon::Pause).is_err());
        assert_eq!(
            inbox.recv_reply_timeout(Duration::from_millis(1)),
            Err(RecvTimeoutError::Timeout)
        );
        for id in 0..CLIENT_QUEUE as u64 + 20 {
            inbox.receive(DaemonToUi::CommandResult { id, result: Ok(()) });
        }
        assert!(inbox.reserve(100, &UiToDaemon::Pause).is_err());
        assert_eq!(inbox.recv_reply_timeout(Duration::ZERO), Ok((0, Ok(()))));
        inbox.reserve(100, &UiToDaemon::Pause).unwrap();
        let batch = inbox.take();
        assert!(batch
            .replies
            .into_iter()
            .eq((1..CLIENT_QUEUE as u64).map(|id| (id, Ok(())))));
        inbox.disconnect();
        assert_eq!(
            inbox.recv_reply_timeout(Duration::from_secs(1)),
            Err(RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn waker_fires_on_interactive_update_events_and_skips_routine_logs() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let inbox = ClientInbox::default();
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_cb = Arc::clone(&wakes);
        inbox.set_waker(Arc::new(move || {
            wakes_cb.fetch_add(1, Ordering::SeqCst);
        }));

        inbox.receive(DaemonToUi::LogLine {
            line: "routine heartbeat".into(),
        });
        assert_eq!(wakes.load(Ordering::SeqCst), 0);

        inbox.receive(DaemonToUi::UpdateState(Box::new(UpdateState {
            status: UpdateStatus::Available,
            ..Default::default()
        })));
        assert_eq!(wakes.load(Ordering::SeqCst), 1);

        inbox.receive(DaemonToUi::OpenUpdateModal);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }
}
