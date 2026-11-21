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
    /// 校园网门户 `chkstatus` 识别到的真实校园网 IPv4（穿透 Mihomo/Tailscale TUN）。
    #[serde(default)]
    pub portal_ip: String,
    pub last_check_ms: u64,
    pub last_check_text: String,
    pub check_seq: u64,
    pub pipeline: DiagnosticPipeline,
    pub last_point: Option<crate::metrics_store::TelemetryPoint>,
}

impl MonitorMetrics {
    /// 返回最适合展示与匹配在线设备的真实校园网 IPv4（优先 `portal_ip`，跳过虚拟隧道 IP）。
    pub fn effective_campus_ip(&self) -> &str {
        if !self.portal_ip.is_empty() {
            return &self.portal_ip;
        }
        if !self.local_ip.is_empty() && !is_virtual_tunnel_ip(&self.local_ip) {
            return &self.local_ip;
        }
        ""
    }
}

/// 校园网自助服务（172.16.4.2/Self/）同步状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SelfServiceStatus {
    #[default]
    Idle,
    Refreshing,
    Ready,
    CaptchaCooldown,
    Unreachable,
    AuthFailed,
    UnsupportedPassword,
    NoCredentials,
}

/// 校园网自助服务返回的流量汇总（同时区分校园网限额口径与全账号实际流量口径）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SelfServiceTraffic {
    /// 本月校园网出口计费已用流量（MB）。
    pub campus_used_mb: f64,
    /// 校园网累计剩余可用流量（MB，含结转免费额度）。
    pub campus_remaining_mb: f64,
    /// 本月全账号实际下行总流量（MB，含电信/联通/移动运营商出口）。
    pub month_down_mb: f64,
    /// 本月全账号实际上行总流量（MB，含电信/联通/移动运营商出口）。
    pub month_up_mb: f64,
    /// 门户返回的校园网套餐说明（如「每月免费30G」）。
    #[serde(default)]
    pub plan_description: String,
    /// 本月累计上网会话次数（来自 `getUserOnlineLog` 汇总）。
    #[serde(default)]
    pub month_session_count: Option<u64>,
}

impl SelfServiceTraffic {
    pub fn month_actual_total_mb(&self) -> f64 {
        (self.month_down_mb.max(0.0) + self.month_up_mb.max(0.0)).max(self.campus_used_mb.max(0.0))
    }

    pub fn campus_total_mb(&self) -> f64 {
        self.campus_used_mb.max(0.0) + self.campus_remaining_mb.max(0.0)
    }
}

/// 门户当前在线的一台终端会话（仅保留非敏感白名单字段）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnlineDevice {
    pub ip: String,
    /// 门户返回的本地上线时间文本（如 `2026-11-22 05:52:34`）。
    pub login_time: String,
    /// 解析后的 Unix 秒时间戳（用于界面实时计算已在线时长）。
    #[serde(default)]
    pub login_unix: Option<i64>,
    /// 终端类型标签（如 `PC`、`手机`、`平板`）。
    #[serde(default)]
    pub terminal_type: String,
    /// 门户服务端返回的当前会话已在线秒数（规避本地时钟偏差）。
    #[serde(default)]
    pub use_time_secs: Option<u64>,
}

/// 门户近期上网记录单条会话（来自 `/Self/bill/getUserOnlineLog`，含真实运营商流量）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecentSession {
    pub login_unix_ms: i64,
    pub logout_unix_ms: Option<i64>,
    pub ip: String,
    /// 真实会话持续秒数（优先由 `logout - login` 计算）。
    pub duration_secs: u64,
    /// 校园网计费流量（MB；运营商出口通常为 0）。
    pub campus_flow_mb: f64,
    /// 实际下行流量（MB，来自 `internetDownFlow`）。
    pub actual_down_mb: f64,
    /// 实际上行流量（MB，来自 `internetUpFlow`）。
    pub actual_up_mb: f64,
}

impl RecentSession {
    pub fn actual_total_mb(&self) -> f64 {
        let actual = self.actual_down_mb.max(0.0) + self.actual_up_mb.max(0.0);
        if actual > 0.0 {
            actual
        } else {
            self.campus_flow_mb.max(0.0)
        }
    }
}

/// 单次自助服务只读查询的完整白名单快照。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SelfServiceData {
    /// 所属学号的脱敏隔离键（同校不同运营商配置共享缓存，不含明文学号）。
    #[serde(default)]
    pub account_key: String,
    pub traffic: SelfServiceTraffic,
    /// 账号允许同时在线的最大终端数（如 `Some(3)`）。
    #[serde(default)]
    pub max_online_devices: Option<u32>,
    #[serde(default)]
    pub online_devices: Vec<OnlineDevice>,
    #[serde(default)]
    pub recent_sessions: Vec<RecentSession>,
    /// 本次快照拉取成功的 Unix 秒时间戳。
    pub fetched_at_unix: i64,
}

impl SelfServiceData {
    /// 尝试在当前在线列表中匹配本机终端，返回对应的权威会话上线 Unix 秒时间戳。
    pub fn matched_local_since_unix(&self, local_campus_ip: &str) -> Option<i64> {
        let device = if !local_campus_ip.is_empty() {
            self.online_devices.iter().find(|d| d.ip == local_campus_ip)
        } else if self.online_devices.len() == 1 {
            self.online_devices.first()
        } else {
            None
        }?;
        if let Some(secs) = device.use_time_secs {
            return Some(self.fetched_at_unix.saturating_sub(secs as i64));
        }
        device.login_unix
    }
}

/// 守护进程通过 IPC 推送给界面的自助服务整体状态（保留旧缓存以支持优雅降级）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SelfServiceState {
    pub status: SelfServiceStatus,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub cooldown_until_unix: Option<i64>,
    #[serde(default)]
    pub data: Option<SelfServiceData>,
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

/// 单日校园网本机流量统计（字节）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyTraffic {
    /// 本地日期 `YYYY-MM-DD`。
    pub date: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl DailyTraffic {
    pub fn total_bytes(&self) -> u64 {
        self.rx_bytes.saturating_add(self.tx_bytes)
    }
}

/// 守护进程推送给界面的本机流量摘要。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficSummary {
    pub today_rx_bytes: u64,
    pub today_tx_bytes: u64,
    pub month_rx_bytes: u64,
    pub month_tx_bytes: u64,
    /// 按时间正序（含今日）的近 7 天统计。
    #[serde(default)]
    pub last_7_days: Vec<DailyTraffic>,
}

impl TrafficSummary {
    pub fn today_total_bytes(&self) -> u64 {
        self.today_rx_bytes.saturating_add(self.today_tx_bytes)
    }

    pub fn month_total_bytes(&self) -> u64 {
        self.month_rx_bytes.saturating_add(self.month_tx_bytes)
    }
}

/// 单个校园网账号配置（支持自定义备注名与独立月流量上限）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountProfile {
    /// 稳定唯一标识（如 `default` 或 `acc-a1b2c3d4`，用作凭据目标后缀与活跃索引）。
    pub id: String,
    /// 用户自定义备注（如「宿舍联通」「实验室电信」），界面优先显示备注。
    pub remark: String,
    pub student_id: String,
    /// 运营商编码：`"0"` 校园用户 / `"1"` 电信 / `"2"` 联通 / `"3"` 移动。
    pub carrier: String,
    /// 可选月流量提醒上限（GB）；`0` 表示不限制（默认，适合运营商免计费出口）。
    pub monthly_quota_gb: i64,
}

impl Default for AccountProfile {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            remark: String::new(),
            student_id: String::new(),
            carrier: "2".to_string(),
            monthly_quota_gb: 0,
        }
    }
}

impl AccountProfile {
    pub fn normalized(&self) -> Self {
        let carrier = normalize_carrier(&self.carrier);
        Self {
            id: self.id.trim().to_string(),
            remark: self.remark.trim().to_string(),
            student_id: self.student_id.trim().to_string(),
            carrier,
            monthly_quota_gb: self.monthly_quota_gb.clamp(0, 2000),
        }
    }

    /// 脱敏学号（保留前 4 位，后接 `***`）。
    pub fn masked_student_id(&self) -> String {
        mask_student_id(&self.student_id)
    }

    /// 界面优先展示备注名；备注为空时回退为脱敏学号。
    pub fn display_name(&self) -> String {
        let remark = self.remark.trim();
        if !remark.is_empty() {
            return remark.to_string();
        }
        let sid = self.student_id.trim();
        if !sid.is_empty() {
            return mask_student_id(sid);
        }
        "未命名账号".to_string()
    }

    /// 按账号隔离的 Windows 凭据管理器目标名。
    pub fn credential_target(&self) -> String {
        crate::credentials::target_for_account(&self.id)
    }
}

pub fn mask_student_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return "未填学号".to_string();
    }
    if trimmed.chars().count() <= 4 {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(4).collect();
        format!("{head}***")
    }
}

fn normalize_carrier(raw: &str) -> String {
    if matches!(raw, "0" | "1" | "2" | "3") {
        raw.to_string()
    } else {
        "2".to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 活跃账号的学号镜像字段（向后兼容旧版配置与只读访问）。
    pub student_id: String,
    /// 活跃账号的运营商镜像字段。
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
    /// 活跃账号的可选月流量提醒上限（GB）；`0` 表示不限制。
    pub monthly_quota_gb: i64,
    /// 多账号配置列表（为空且旧版 `student_id` 非空时自动平滑迁移）。
    #[serde(default)]
    pub accounts: Vec<AccountProfile>,
    /// 当前活跃账号的 `AccountProfile::id`。
    #[serde(default)]
    pub active_account_id: String,
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
            monthly_quota_gb: 0,
            accounts: Vec::new(),
            active_account_id: String::new(),
        }
    }
}

impl Settings {
    /// 将配置值限制在支持的范围内，自动完成旧版单账号迁移并同步活跃账号镜像字段。
    pub fn normalized(&self) -> Self {
        let legacy_sid = self.student_id.trim().to_string();
        let legacy_carrier = normalize_carrier(&self.carrier);
        let legacy_quota = self.monthly_quota_gb.clamp(0, 2000);

        let mut accounts: Vec<AccountProfile> = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        if self.accounts.is_empty() {
            if !legacy_sid.is_empty() {
                accounts.push(AccountProfile {
                    id: "default".to_string(),
                    remark: String::new(),
                    student_id: legacy_sid.clone(),
                    carrier: legacy_carrier.clone(),
                    monthly_quota_gb: legacy_quota,
                });
            }
        } else {
            for (idx, raw) in self.accounts.iter().enumerate() {
                let mut p = raw.normalized();
                if p.id.is_empty() || seen_ids.contains(&p.id) {
                    let mut n = idx + 1;
                    loop {
                        let candidate = format!("acc-{n}");
                        if !seen_ids.contains(&candidate) {
                            p.id = candidate;
                            break;
                        }
                        n += 1;
                    }
                }
                seen_ids.insert(p.id.clone());
                accounts.push(p);
            }
        }

        let active_id = self.active_account_id.trim().to_string();
        let active_account_id = if accounts.iter().any(|a| a.id == active_id) {
            active_id
        } else {
            accounts.first().map(|a| a.id.clone()).unwrap_or_default()
        };

        let (student_id, carrier, monthly_quota_gb) = accounts
            .iter()
            .find(|a| a.id == active_account_id)
            .map(|a| (a.student_id.clone(), a.carrier.clone(), a.monthly_quota_gb))
            .unwrap_or((legacy_sid, legacy_carrier, legacy_quota));

        Self {
            student_id,
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
            monthly_quota_gb,
            accounts,
            active_account_id,
        }
    }

    pub fn active_account(&self) -> Option<&AccountProfile> {
        self.accounts
            .iter()
            .find(|a| a.id == self.active_account_id)
            .or_else(|| self.accounts.first())
    }

    pub fn find_account(&self, id: &str) -> Option<&AccountProfile> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// 返回当前活跃账号的展示名（优先备注，次选脱敏学号，未配置时返回「未配置账号」）。
    pub fn active_display_name(&self) -> String {
        if let Some(acc) = self.active_account() {
            return acc.display_name();
        }
        if !self.student_id.trim().is_empty() {
            return mask_student_id(&self.student_id);
        }
        "未配置账号".to_string()
    }

    /// 切换当前活跃账号并同步镜像字段；账号不存在时返回错误。
    pub fn switch_active_account(&mut self, account_id: &str) -> Result<AccountProfile, String> {
        let Some(target) = self.accounts.iter().find(|a| a.id == account_id).cloned() else {
            return Err("未找到要切换的目标账号".to_string());
        };
        self.active_account_id = target.id.clone();
        self.student_id = target.student_id.clone();
        self.carrier = target.carrier.clone();
        self.monthly_quota_gb = target.monthly_quota_gb;
        Ok(target)
    }

    /// 新增或更新账号配置，可选同时设为当前活跃账号。
    pub fn upsert_account(&mut self, profile: AccountProfile, make_active: bool) {
        let profile = profile.normalized();
        if let Some(existing) = self.accounts.iter_mut().find(|a| a.id == profile.id) {
            *existing = profile.clone();
        } else {
            self.accounts.push(profile.clone());
        }
        if make_active || self.active_account_id.is_empty() {
            self.active_account_id = profile.id;
        }
        *self = self.normalized();
    }

    /// 删除指定账号；若删掉了活跃账号则自动切换到剩余第一个账号。
    pub fn remove_account(&mut self, account_id: &str) -> Option<AccountProfile> {
        let pos = self.accounts.iter().position(|a| a.id == account_id)?;
        let removed = self.accounts.remove(pos);
        if self.active_account_id == account_id {
            self.active_account_id = self
                .accounts
                .first()
                .map(|a| a.id.clone())
                .unwrap_or_default();
        }
        if self.accounts.is_empty() {
            self.student_id.clear();
            self.monthly_quota_gb = 0;
        }
        *self = self.normalized();
        Some(removed)
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

    #[test]
    fn legacy_single_account_config_migrates_into_accounts_list() {
        let legacy_json = r#"{
            "student_id": "20240001",
            "carrier": "1",
            "monthly_quota_gb": 50
        }"#;
        let parsed = serde_json::from_str::<Settings>(legacy_json)
            .unwrap()
            .normalized();
        assert_eq!(parsed.accounts.len(), 1);
        assert_eq!(parsed.active_account_id, "default");
        assert_eq!(parsed.accounts[0].student_id, "20240001");
        assert_eq!(parsed.accounts[0].carrier, "1");
        assert_eq!(parsed.accounts[0].monthly_quota_gb, 50);
        assert_eq!(parsed.active_display_name(), "2024***");
    }

    #[test]
    fn multi_account_switch_upsert_remove_and_display_name_priority() {
        let mut settings = Settings::default();
        settings.upsert_account(
            AccountProfile {
                id: "acc-dorm".into(),
                remark: "宿舍联通".into(),
                student_id: "20240001".into(),
                carrier: "2".into(),
                monthly_quota_gb: 0,
            },
            true,
        );
        settings.upsert_account(
            AccountProfile {
                id: "acc-lab".into(),
                remark: String::new(),
                student_id: "20249999".into(),
                carrier: "1".into(),
                monthly_quota_gb: 80,
            },
            false,
        );

        assert_eq!(settings.active_account_id, "acc-dorm");
        assert_eq!(settings.active_display_name(), "宿舍联通");
        assert_eq!(settings.student_id, "20240001");
        assert_eq!(settings.carrier, "2");

        settings.switch_active_account("acc-lab").unwrap();
        assert_eq!(settings.active_account_id, "acc-lab");
        assert_eq!(settings.student_id, "20249999");
        assert_eq!(settings.carrier, "1");
        assert_eq!(settings.monthly_quota_gb, 80);
        assert_eq!(settings.active_display_name(), "2024***");

        assert!(settings.switch_active_account("missing").is_err());

        settings.remove_account("acc-lab").unwrap();
        assert_eq!(settings.accounts.len(), 1);
        assert_eq!(settings.active_account_id, "acc-dorm");
        assert_eq!(settings.student_id, "20240001");
    }
}
