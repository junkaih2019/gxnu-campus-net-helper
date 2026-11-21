//! 桌面通知。
//!
//! 企业微信推送与推送重试队列留到下一阶段。

use std::time::{Duration, Instant};

use crate::logging;
use crate::models::{DisconnectReason, MonitorSnapshot, MonitorState};

/// 通知使用的 AppUserModelID（在 startup.rs 里注册显示名）。
pub const AUMID: &str = "GXNU.CampusGuard";

#[derive(Clone, Copy, PartialEq, Eq)]
enum OfflineAction {
    WaitForNetwork,
    Authenticating,
    TimedRetry,
    CheckAccount,
}

pub struct ToastNotifier {
    enabled: bool,
    offline_since: Option<Instant>,
    last_reason: Option<DisconnectReason>,
    last_action: Option<OfflineAction>,
}

impl ToastNotifier {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            offline_since: None,
            last_reason: None,
            last_action: None,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn handle(&mut self, snapshot: &MonitorSnapshot) {
        if let Some(body) = self.notification_for(snapshot) {
            self.toast(&body);
        }
    }

    pub(crate) fn notification_for(&mut self, snapshot: &MonitorSnapshot) -> Option<String> {
        match snapshot.state {
            MonitorState::Paused | MonitorState::Stopped => {
                self.offline_since = None;
                self.last_reason = None;
                self.last_action = None;
                None
            }
            MonitorState::Offline => self.offline_notice(snapshot),
            MonitorState::Online if self.offline_since.is_some() => Some(self.on_recovery()),
            _ => None,
        }
    }

    fn offline_notice(&mut self, snapshot: &MonitorSnapshot) -> Option<String> {
        let reason = snapshot.reason?;
        self.offline_since.get_or_insert_with(Instant::now);
        self.last_reason = Some(reason);
        let (action, next) = if reason.needs_account() {
            (
                OfflineAction::CheckAccount,
                "请打开账号设置，核对学号、密码和运营商".to_owned(),
            )
        } else if let Some(seconds) = snapshot.retry_after_secs {
            (
                OfflineAction::TimedRetry,
                format!("{seconds} 秒后自动重试登录"),
            )
        } else if matches!(
            reason,
            DisconnectReason::AuthExpired | DisconnectReason::InternetTimeout
        ) {
            (OfflineAction::Authenticating, snapshot.message.clone())
        } else {
            (
                OfflineAction::WaitForNetwork,
                "等待网络恢复后自动复检".to_owned(),
            )
        };
        let previous = self.last_action.replace(action);
        if previous == Some(action)
            || (previous == Some(OfflineAction::Authenticating)
                && action == OfflineAction::TimedRetry
                && matches!(
                    reason,
                    DisconnectReason::AuthExpired | DisconnectReason::InternetTimeout
                ))
        {
            return None;
        }
        Some(format!("{}。{next}", reason.label()))
    }

    fn on_recovery(&mut self) -> String {
        let duration = self
            .offline_since
            .map(|at| at.elapsed())
            .unwrap_or_default();
        let last_reason = self
            .last_reason
            .map(DisconnectReason::label)
            .unwrap_or("未知原因");
        self.offline_since = None;
        self.last_reason = None;
        self.last_action = None;
        logging::info(&format!(
            "[NOTIFY_SENT] 网络恢复提醒；最后原因={last_reason}"
        ));
        format!("网络已恢复（断网 {}）", format_duration(duration))
    }

    /// 发送一条测试通知（`--test-toast` 或设置页测试按钮用）。
    pub fn send_test(&self, body: &str) {
        show_toast_sync(body);
    }

    fn toast(&self, body: &str) {
        if !self.enabled {
            return;
        }
        // 异步线程弹出 Windows Toast，避免 WinRT 通知调用阻塞断网重连与复验链路。
        let body = body.to_string();
        let _ = std::thread::Builder::new()
            .name("toast-notify".to_string())
            .spawn(move || show_toast_sync(&body));
    }
}

fn show_toast_sync(body: &str) {
    match notify_rust::Notification::new()
        .app_id(AUMID)
        .summary("校园网守护")
        .body(body)
        .timeout(notify_rust::Timeout::Milliseconds(6000))
        .show()
    {
        Ok(_) => logging::info(&format!("[NOTIFY_SENT] 系统弹窗：{body}")),
        Err(err) => logging::warn(&format!("[NOTIFY_RETRY] 系统弹窗失败：{err}")),
    }
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let sec = seconds % 60;
    if minutes >= 60 {
        return format!("{} 小时 {} 分", minutes / 60, minutes % 60);
    }
    if minutes > 0 {
        return format!("{minutes} 分 {sec} 秒");
    }
    format!("{sec} 秒")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_notification_duration() {
        assert_eq!(format_duration(Duration::from_secs(45)), "45 秒");
        assert_eq!(format_duration(Duration::from_secs(125)), "2 分 5 秒");
        assert_eq!(format_duration(Duration::from_secs(3900)), "1 小时 5 分");
    }
    #[test]
    fn outage_notices_follow_actions_and_report_account_attention_once() {
        use DisconnectReason::*;
        let mut n = ToastNotifier::new(false);
        let mut notice = |reason, delay| {
            let mut s =
                MonitorSnapshot::new(MonitorState::Offline, Some(reason), "尝试自动登录", true);
            s.retry_after_secs = delay;
            n.offline_notice(&s)
        };
        assert!(notice(LinkDown, None).unwrap().contains("等待网络"));
        assert!(notice(NoGateway, None).is_none());
        assert!(notice(AuthExpired, None).unwrap().contains("尝试自动登录"));
        assert!(notice(AuthExpired, Some(5)).is_none());
        assert!(notice(LoginRejected, Some(15))
            .unwrap()
            .contains("账号设置"));
        assert!(notice(LoginRejected, Some(30)).is_none());
        assert!(notice(PortalUnavailable, Some(60))
            .unwrap()
            .contains("60 秒"));
        assert!(notice(PortalUnavailable, Some(300)).is_none());
        assert!(notice(CredentialsMissing, None)
            .unwrap()
            .contains("账号设置"));
        n.handle(&MonitorSnapshot::new(
            MonitorState::Online,
            Some(Online),
            "",
            true,
        ));
        assert!(n
            .offline_notice(&MonitorSnapshot::new(
                MonitorState::Offline,
                Some(CredentialsMissing),
                "",
                true
            ))
            .is_some());
    }
}
