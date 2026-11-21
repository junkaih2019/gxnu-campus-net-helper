//! 监控、诊断与设置的数据类型。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisconnectReason {
    Online,
    LinkDown,
    NoIp,
    NoGateway,
    GatewayUnreachable,
    DnsFailure,
    AuthExpired,
    InternetTimeout,
    UplinkUnreachable,
    LoginRejected,
    PortalUnavailable,
    CredentialsMissing,
}

impl DisconnectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "ONLINE",
            Self::LinkDown => "LINK_DOWN",
            Self::NoIp => "NO_IP",
            Self::NoGateway => "NO_GATEWAY",
            Self::GatewayUnreachable => "GATEWAY_UNREACHABLE",
            Self::DnsFailure => "DNS_FAILURE",
            Self::AuthExpired => "AUTH_EXPIRED",
            Self::InternetTimeout => "INTERNET_TIMEOUT",
            Self::UplinkUnreachable => "UPLINK_UNREACHABLE",
            Self::LoginRejected => "LOGIN_REJECTED",
            Self::PortalUnavailable => "PORTAL_UNAVAILABLE",
            Self::CredentialsMissing => "CREDENTIALS_MISSING",
        }
    }

    pub fn needs_account(self) -> bool {
        matches!(self, Self::CredentialsMissing | Self::LoginRejected)
    }

    /// 断网提醒里给人看的中文标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::Online => "网络正常",
            Self::LinkDown => "网线断开或 Wi-Fi 断开",
            Self::NoIp => "未获取 IPv4 地址",
            Self::NoGateway => "没有默认网关",
            Self::GatewayUnreachable => "网关无响应",
            Self::DnsFailure => "DNS 解析故障",
            Self::AuthExpired => "校园网认证过期",
            Self::InternetTimeout => "公网探测超时",
            Self::UplinkUnreachable => "公网出口异常",
            Self::LoginRejected => "登录被拒绝",
            Self::PortalUnavailable => "认证服务器不可用",
            Self::CredentialsMissing => "未保存账号密码",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonitorState {
    Stopped,
    Paused,
    Checking,
    Online,
    Suspect,
    Offline,
    Authenticating,
    Verifying,
}

impl MonitorState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "STOPPED",
            Self::Paused => "PAUSED",
            Self::Checking => "CHECKING",
            Self::Online => "ONLINE",
            Self::Suspect => "SUSPECT",
            Self::Offline => "OFFLINE",
            Self::Authenticating => "AUTHENTICATING",
            Self::Verifying => "VERIFYING",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosticResult {
    pub online: bool,
    pub reason: DisconnectReason,
    pub message: String,
    pub local_ip: String,
    pub gateway: String,
    /// 校园网门户 `chkstatus` 识别到的真实校园网 IPv4（路由器/TUN 环境下优先用作 `v4ip`）。
    pub portal_ip: String,
    pub can_authenticate: bool,
    /// 有序的 k/v 证据。
    pub evidence: Vec<(String, String)>,
}

impl DiagnosticResult {
    pub fn new(online: bool, reason: DisconnectReason, message: impl Into<String>) -> Self {
        Self {
            online,
            reason,
            message: message.into(),
            local_ip: String::new(),
            gateway: String::new(),
            portal_ip: String::new(),
            can_authenticate: false,
            evidence: Vec::new(),
        }
    }

    pub fn with_ip(mut self, local_ip: impl Into<String>, gateway: impl Into<String>) -> Self {
        self.local_ip = local_ip.into();
        self.gateway = gateway.into();
        self
    }

    pub fn with_portal_ip(mut self, portal_ip: Option<String>) -> Self {
        if let Some(ip) = portal_ip {
            self.portal_ip = ip;
        }
        self
    }

    /// 返回适合提交给 Dr.COM `v4ip` 参数的 IP：
    /// 1) 优先使用门户 `chkstatus` 返回的真实校园网 IP（如 `172.16.x.x`）；
    /// 2) 若 `local_ip` 属于 Mihomo TUN (`198.18.0.0/15`) 或 Tailscale (`100.64.0.0/10`) 虚拟地址，
    ///    返回空串让门户自动取真实请求源 IP，避免传假 IP 被拒。
    pub fn auth_ip(&self) -> &str {
        if !self.portal_ip.is_empty() {
            return &self.portal_ip;
        }
        if is_virtual_tunnel_ip(&self.local_ip) {
            return "";
        }
        &self.local_ip
    }

    pub fn authenticatable(mut self, can: bool) -> Self {
        self.can_authenticate = can;
        self
    }

    pub fn push_evidence(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.evidence.push((key.into(), value.into()));
    }

    pub fn evidence_text(&self) -> String {
        self.evidence
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn is_virtual_tunnel_ip(ip: &str) -> bool {
    let Ok(parsed) = ip.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    let [a, b, _, _] = parsed.octets();
    // 198.18.0.0/15 (Mihomo/Clash Fake-IP TUN) 或 100.64.0.0/10 (Tailscale/CGNAT)
    (a == 198 && (b == 18 || b == 19)) || (a == 100 && (64..=127).contains(&b))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorSnapshot {
    pub monitoring_enabled: bool,
    #[serde(default)]
    pub retry_after_secs: Option<u64>,
    pub state: MonitorState,
    pub reason: Option<DisconnectReason>,
    pub message: String,
    pub metrics: MonitorMetrics,
}

/// 单步诊断节点的展示状态（供概览页诊断链路可视化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StepState {
    #[default]
    Pending,
    Ok,
    Warn,
    Fail,
    Skipped,
}

/// 四层诊断链路快照：网卡链路 → IP 与网关 → 公网探测 → 校园门户。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagnosticPipeline {
    pub adapter: (StepState, String),
    pub ip_gateway: (StepState, String),
    pub internet: (StepState, String),
    pub portal: (StepState, String),
}

/// 概览页展示的最近一次检测数据。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MonitorMetrics {
    pub local_ip: String,
    pub gateway: String,
    pub last_check_ms: u64,
    pub last_check_text: String,
    pub check_seq: u64,
    pub pipeline: DiagnosticPipeline,
    pub last_point: Option<crate::metrics_store::TelemetryPoint>,
}

impl MonitorSnapshot {
    pub fn new(
        state: MonitorState,
        reason: Option<DisconnectReason>,
        message: impl Into<String>,
        monitoring_enabled: bool,
    ) -> Self {
        Self {
            monitoring_enabled,
            retry_after_secs: None,
            state,
            reason,
            message: message.into(),
            metrics: MonitorMetrics::default(),
        }
    }

    pub fn with_metrics(mut self, metrics: MonitorMetrics) -> Self {
        self.metrics = metrics;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub student_id: String,
    pub carrier: String,
    pub check_interval: i64,
    pub heartbeat_interval: i64,
    pub offline_interval: i64,
    pub confirm_failures: i64,
    pub auto_start_monitoring: bool,
    pub auto_boot: bool,
    pub notify_toast: bool,
    pub notify_wechat: bool,
    pub notify_quiet_minutes: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            student_id: String::new(),
            carrier: "2".to_string(),
            check_interval: 30,
            heartbeat_interval: 600,
            offline_interval: 15,
            confirm_failures: 2,
            auto_start_monitoring: false,
            auto_boot: false,
            notify_toast: true,
            notify_wechat: false,
            notify_quiet_minutes: 3,
        }
    }
}

impl Settings {
    /// 将配置值限制在支持的范围内。
    pub fn normalized(&self) -> Self {
        let carrier = if matches!(self.carrier.as_str(), "0" | "1" | "2" | "3") {
            self.carrier.clone()
        } else {
            "2".to_string()
        };
        Self {
            student_id: self.student_id.trim().to_string(),
            carrier,
            check_interval: self.check_interval.clamp(10, 3600),
            heartbeat_interval: self.heartbeat_interval.clamp(60, 86400),
            offline_interval: self.offline_interval.clamp(5, 300),
            confirm_failures: self.confirm_failures.clamp(1, 5),
            auto_start_monitoring: self.auto_start_monitoring,
            auto_boot: self.auto_boot,
            notify_toast: self.notify_toast,
            notify_wechat: self.notify_wechat,
            notify_quiet_minutes: self.notify_quiet_minutes.clamp(1, 60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_ip_prefers_portal_ip_and_skips_virtual_tunnel_ips() {
        let r1 = DiagnosticResult::new(false, DisconnectReason::AuthExpired, "need auth")
            .with_ip("198.18.0.1", "198.18.0.2")
            .with_portal_ip(Some("192.0.2.42".to_string()));
        assert_eq!(r1.auth_ip(), "192.0.2.42");

        let r2 = DiagnosticResult::new(false, DisconnectReason::AuthExpired, "need auth")
            .with_ip("198.18.0.1", "198.18.0.2");
        assert_eq!(r2.auth_ip(), "");

        let r3 = DiagnosticResult::new(false, DisconnectReason::AuthExpired, "need auth")
            .with_ip("198.51.100.70", "198.51.100.1");
        assert_eq!(r3.auth_ip(), "198.51.100.70");
    }

    #[test]
    fn settings_clamp_to_supported_ranges() {
        let raw = Settings {
            student_id: "  demo-student  ".to_string(),
            carrier: "9".to_string(),
            check_interval: 1,
            heartbeat_interval: 99_999_999,
            offline_interval: 0,
            confirm_failures: 9,
            notify_quiet_minutes: 0,
            ..Default::default()
        }
        .normalized();

        assert_eq!(raw.student_id, "demo-student");
        assert_eq!(raw.carrier, "2");
        assert_eq!(raw.check_interval, 10);
        assert_eq!(raw.heartbeat_interval, 86_400);
        assert_eq!(raw.offline_interval, 5);
        assert_eq!(raw.confirm_failures, 5);
        assert_eq!(raw.notify_quiet_minutes, 1);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let text = r#"{
            "student_id": "demo-student",
            "carrier": "2",
            "check_interval": 300,
            "heartbeat_interval": 1800,
            "offline_interval": 15,
            "confirm_failures": 2,
            "auto_start_monitoring": true,
            "auto_boot": true,
            "notify_toast": true,
            "notify_wechat": false,
            "notify_quiet_minutes": 3,
            "unknown_future_key": 1
        }"#;
        let parsed: Settings = serde_json::from_str(text).expect("config must parse");
        assert_eq!(parsed.check_interval, 300);
        assert!(parsed.auto_boot);

        let dumped = serde_json::to_string(&parsed).expect("config must serialize");
        assert!(dumped.contains("\"student_id\":\"demo-student\""));
        assert!(!dumped.contains("unknown_future_key"));
    }
}
