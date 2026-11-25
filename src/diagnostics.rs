//! 分层诊断。
//!
//! 诊断顺序：网卡 → 出网 IP → 网关 → 并行公网探测 →
//! （探测全失败且链路正常时）问门户在线状态 → 分类结论。

use std::error::Error;
use std::io;
use std::net::Ipv4Addr;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, LazyLock, Weak};
use std::time::{Duration, Instant};

use crate::campus_status::{CampusStatusInfo, CampusStatusProbe};
use crate::http::{self, HttpError};
use crate::logging;
use crate::models::{DiagnosticResult, DisconnectReason};
use crate::portal_gateway::{captive_origin_from_prefix, extract_captive_portal_origin};

#[derive(Clone, Copy)]
pub struct ProbeTarget {
    pub url: &'static str,
    pub expect_text: Option<&'static str>,
}

pub const DEFAULT_PROBES: &[ProbeTarget] = &[
    ProbeTarget {
        url: http::CONNECT_TEST_URL,
        expect_text: Some(http::CONNECT_TEST_BODY),
    },
    ProbeTarget {
        url: "https://www.baidu.com/favicon.ico",
        expect_text: None,
    },
];

const PROBE_TEXT_LIMIT: usize = 4 * 1024;
const PROBE_ERROR_CHARS: usize = 512;
static NEXT_PROBE_ROUND: AtomicU64 = AtomicU64::new(1);
static PROBE_SESSION: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    )
});

/// 只用来做路由决策，不发包。
const ROUTE_PROBE_IP: Ipv4Addr = Ipv4Addr::new(223, 5, 5, 5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoutePath {
    Unavailable,
    Ipv4 {
        interface: u64,
        source: Ipv4Addr,
        next_hop: Ipv4Addr,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeKind {
    Ok,
    Unexpected(u16),
    HttpStatus(u16),
    Error(HttpError),
}

impl ProbeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Unexpected(_) => "unexpected",
            Self::HttpStatus(_) => "http_status",
            Self::Error(HttpError::Dns) => "dns",
            Self::Error(HttpError::Timeout) => "timeout",
            Self::Error(_) => "error",
        }
    }

    fn note(self) -> String {
        match self {
            Self::Ok => String::new(),
            Self::Unexpected(status) | Self::HttpStatus(status) => format!("HTTP {status}"),
            Self::Error(error) => error.as_str().into(),
        }
    }
}

struct ProbeOutcome {
    kind: ProbeKind,
    hostname: &'static str,
    captive_origin: Option<String>,
}

struct ProbeRequest {
    round: Weak<u64>,
    result: mpsc::Sender<ProbeOutcome>,
}

// A single worker per target bounds outstanding HTTP requests across manual checks.
fn probe_worker(
    check: impl Fn(u64) -> ProbeOutcome + Send + 'static,
) -> mpsc::Sender<ProbeRequest> {
    let (tx, rx) = mpsc::channel::<ProbeRequest>();
    std::thread::spawn(move || {
        for request in rx {
            if let Some(round) = request.round.upgrade().map(|round| *round) {
                let _ = request.result.send(check(round));
            }
        }
    });
    tx
}

pub struct NetworkDiagnostics {
    probes: Vec<mpsc::Sender<ProbeRequest>>,
    campus_status: Option<CampusStatusProbe>,
}

impl Default for NetworkDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkDiagnostics {
    pub fn new() -> Self {
        // 公网探测禁用重定向，使门户跳转可以作为网关探测线索。
        let agent = http::no_redirect_agent(5);
        let probes = DEFAULT_PROBES
            .iter()
            .map(|target| {
                let agent = agent.clone();
                probe_worker(move |round| probe_once(&agent, target, round))
            })
            .collect();
        Self {
            probes,
            campus_status: None,
        }
    }

    pub fn with_campus_status(mut self, probe: CampusStatusProbe) -> Self {
        self.campus_status = Some(probe);
        self
    }

    /// 测量到校园网认证网关（yc.gxnu.edu.cn）的真实往返延迟（毫秒）。
    pub fn measure_portal_latency_ms(&self) -> Option<u64> {
        self.campus_status
            .as_ref()
            .and_then(CampusStatusProbe::measure_rtt_ms)
    }

    /// 测量网关延迟并同时提取门户识别到的本机校园网 IPv4。
    pub fn measure_portal_rtt_and_ip(&self) -> Option<(u64, Option<String>)> {
        self.campus_status
            .as_ref()
            .and_then(CampusStatusProbe::measure_rtt_and_ip)
    }

    /// 测量网关延迟并同时返回 `(毫秒, 门户识别的校园网 IPv4, 生效网关简短名)`。
    pub fn measure_portal_rtt_ip_and_gateway(
        &self,
    ) -> Option<(u64, Option<String>, Option<String>)> {
        self.campus_status
            .as_ref()
            .and_then(CampusStatusProbe::measure_rtt_ip_and_gateway)
    }

    pub(crate) fn route(&self) -> RoutePath {
        read_route_to(ROUTE_PROBE_IP)
    }

    /// The standalone read-only CLI retries a scan once if its evidence spans two routes.
    pub fn check(&self) -> Option<DiagnosticResult> {
        let mut route = self.route();
        for _ in 0..2 {
            let result = self.check_on_route(route);
            let after = self.route();
            if after == route {
                return Some(result);
            }
            route = after;
        }
        None
    }

    pub(crate) fn check_on_route(&self, route: RoutePath) -> DiagnosticResult {
        let mut evidence: Vec<(String, String)> = Vec::new();

        let (adapter_names, adapter_ips) = read_adapters();
        evidence.push((
            "adapters".to_string(),
            if adapter_names.is_empty() {
                "none".to_string()
            } else {
                adapter_names.join(", ")
            },
        ));
        if adapter_names.is_empty() {
            return finish(
                DiagnosticResult::new(
                    false,
                    DisconnectReason::LinkDown,
                    "网卡未连接或 Wi-Fi 已断开",
                ),
                evidence,
            );
        }

        let RoutePath::Ipv4 {
            interface,
            source: local_ip,
            next_hop: gateway,
        } = route
        else {
            evidence.push(("ipv4".into(), join_ips(&adapter_ips)));
            let (reason, message) = if adapter_ips.iter().any(|ip| is_usable_ipv4(*ip)) {
                (DisconnectReason::NoGateway, "没有可用的出网路由")
            } else {
                (DisconnectReason::NoIp, "未取得可用的出网 IPv4 地址")
            };
            return finish(DiagnosticResult::new(false, reason, message), evidence);
        };
        if !is_usable_ipv4(local_ip) {
            evidence.push(("ipv4".into(), join_ips(&adapter_ips)));
            return finish(
                DiagnosticResult::new(false, DisconnectReason::NoIp, "未取得可用的出网 IPv4 地址"),
                evidence,
            );
        }
        evidence.push(("interface_luid".into(), interface.to_string()));
        evidence.push(("local_ip".into(), local_ip.to_string()));
        if !adapter_ips.contains(&local_ip) {
            evidence.push(("local_ip_source".into(), "route".into()));
        }
        evidence.push((
            "gateway".into(),
            if gateway.is_unspecified() {
                "on-link".into()
            } else {
                gateway.to_string()
            },
        ));

        let outcomes = self.run_probes(&mut evidence);
        self.classify_outcomes(&outcomes, local_ip, gateway, evidence, ping_gateway)
    }

    fn classify_outcomes<F>(
        &self,
        outcomes: &[ProbeOutcome],
        local_ip: Ipv4Addr,
        gateway: Ipv4Addr,
        mut evidence: Vec<(String, String)>,
        gateway_ping_fn: F,
    ) -> DiagnosticResult
    where
        F: Fn(Ipv4Addr) -> Option<bool>,
    {
        // A zero next hop in a valid route means on-link. It can carry public
        // traffic but has no gateway whose ping could justify a blind login.
        let routed_gateway = !gateway.is_unspecified();
        let gateway_text = if routed_gateway {
            gateway.to_string()
        } else {
            String::new()
        };
        if let Some(ok) = outcomes.iter().find(|o| o.kind == ProbeKind::Ok) {
            evidence.push(("probe".into(), ok.hostname.into()));
            return finish(
                DiagnosticResult::new(true, DisconnectReason::Online, "公网连接正常")
                    .with_ip(local_ip.to_string(), gateway_text),
                evidence,
            );
        }

        let dns_only = !outcomes.is_empty()
            && outcomes
                .iter()
                .all(|o| o.kind == ProbeKind::Error(HttpError::Dns));
        let captive = outcomes
            .iter()
            .any(|o| matches!(o.kind, ProbeKind::Unexpected(_)));
        let mut gateway_reachable = None;
        if routed_gateway && !dns_only && !captive {
            gateway_reachable = gateway_ping_fn(gateway);
            if let Some(reachable) = gateway_reachable {
                evidence.push((
                    "gateway_ping".into(),
                    if reachable { "ok" } else { "failed" }.into(),
                ));
                if !reachable {
                    return finish(
                        DiagnosticResult::new(
                            false,
                            DisconnectReason::GatewayUnreachable,
                            "默认网关无响应",
                        )
                        .with_ip(local_ip.to_string(), gateway_text),
                        evidence,
                    );
                }
            }
        }

        let captive_origin = outcomes
            .iter()
            .find_map(|outcome| outcome.captive_origin.as_deref());
        let campus = self.query_campus(&mut evidence, captive_origin);
        let result = match campus {
            Some(CampusStatusInfo {
                online: false,
                portal_ip,
                gateway_label,
            }) => DiagnosticResult::new(
                false,
                DisconnectReason::AuthExpired,
                "校园网门户确认当前出口已下线",
            )
            .with_portal_ip(portal_ip)
            .with_portal_gateway(gateway_label)
            .authenticatable(true),
            Some(info) if dns_only => DiagnosticResult::new(
                false,
                DisconnectReason::DnsFailure,
                "校园网账号仍在线，公网出口 DNS 解析失败",
            )
            .with_portal_ip(info.portal_ip)
            .with_portal_gateway(info.gateway_label),
            None if dns_only => DiagnosticResult::new(
                false,
                DisconnectReason::DnsFailure,
                "DNS 无法解析联网检测域名",
            ),
            Some(info) => DiagnosticResult::new(
                false,
                DisconnectReason::UplinkUnreachable,
                "校园网账号仍在线，公网出口不可达（代理节点或上游异常）",
            )
            .with_portal_ip(info.portal_ip)
            .with_portal_gateway(info.gateway_label),
            None if gateway_reachable == Some(true)
                && !outcomes.is_empty()
                && outcomes
                    .iter()
                    .all(|o| o.kind == ProbeKind::Error(HttpError::Timeout)) =>
            {
                DiagnosticResult::new(false, DisconnectReason::InternetTimeout, "公网探测均超时")
                    .authenticatable(true)
            }
            None => DiagnosticResult::new(
                false,
                DisconnectReason::UplinkUnreachable,
                if captive {
                    "公网探测被重定向或返回异常内容，尚未确认校园网认证状态"
                } else {
                    "公网探测失败，尚未确认校园网认证状态"
                },
            ),
        };
        finish(result.with_ip(local_ip.to_string(), gateway_text), evidence)
    }

    fn run_probes(&self, evidence: &mut Vec<(String, String)>) -> Vec<ProbeOutcome> {
        let round = Arc::new(NEXT_PROBE_ROUND.fetch_add(1, Ordering::Relaxed));
        evidence.push(("probe_round".into(), probe_round_id(*round)));
        let (tx, rx) = mpsc::channel();
        for probe in &self.probes {
            let _ = probe.send(ProbeRequest {
                round: Arc::downgrade(&round),
                result: tx.clone(),
            });
        }
        drop(tx);
        let mut outcomes = Vec::new();
        for outcome in rx {
            let online = outcome.kind == ProbeKind::Ok;
            outcomes.push(outcome);
            if online {
                break;
            }
        }
        for outcome in &outcomes {
            if outcome.kind != ProbeKind::Ok {
                let key = if outcome.hostname.is_empty() {
                    "probe".to_string()
                } else {
                    format!("probe_{}", outcome.hostname)
                };
                evidence.push((key, outcome.kind.note()));
            }
        }
        outcomes
    }

    fn query_campus(
        &self,
        evidence: &mut Vec<(String, String)>,
        captive_origin: Option<&str>,
    ) -> Option<CampusStatusInfo> {
        let probe = self.campus_status.as_ref()?;
        let info = probe.check_detail_with_hint(captive_origin)?;
        evidence.push((
            "campus_status".to_string(),
            if info.online {
                "online".to_string()
            } else {
                "offline".to_string()
            },
        ));
        if let Some(ip) = info.portal_ip.as_deref() {
            evidence.push(("portal_ip".to_string(), ip.to_string()));
        }
        if let Some(gw) = info.gateway_label.as_deref() {
            evidence.push(("portal_gateway".to_string(), gw.to_string()));
        }
        Some(info)
    }
}

fn finish(mut result: DiagnosticResult, evidence: Vec<(String, String)>) -> DiagnosticResult {
    result.evidence = evidence;
    result
}

fn probe_round_id(round: u64) -> String {
    format!("{}-{round}", *PROBE_SESSION)
}

fn probe_once(agent: &ureq::Agent, target: &ProbeTarget, round: u64) -> ProbeOutcome {
    let hostname = target
        .url
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
        .and_then(|authority| authority.rsplit('@').next())
        .unwrap_or("");
    let started = Instant::now();
    // 在探测线程中记录，另一项先成功时仍可保留本次失败。
    let response = match http::get_response(agent, target.url) {
        Ok(response) => response,
        Err(error) => {
            let (kind, stage) = http::classify_transport(&error);
            log_probe_error(
                round,
                hostname,
                started.elapsed(),
                stage,
                kind,
                error.as_ref(),
            );
            return ProbeOutcome {
                kind: ProbeKind::Error(kind),
                hostname,
                captive_origin: None,
            };
        }
    };
    let (kind, captive_origin, read_error) = probe_response(response, target.expect_text);
    let elapsed = started.elapsed();
    match (kind, read_error) {
        (_, Some(error)) => log_probe_error(
            round,
            hostname,
            elapsed,
            "http_body",
            HttpError::from(&error),
            &error,
        ),
        (ProbeKind::Unexpected(200), None) => log_probe_failure(
            round,
            hostname,
            elapsed,
            "http_body",
            "status=200, error=UnexpectedContent",
        ),
        (ProbeKind::Unexpected(status) | ProbeKind::HttpStatus(status), None) => log_probe_failure(
            round,
            hostname,
            elapsed,
            "http_status",
            format_args!("status={status}"),
        ),
        _ => {}
    }
    ProbeOutcome {
        kind,
        hostname,
        captive_origin,
    }
}

fn log_probe_error(
    round: u64,
    hostname: &str,
    elapsed: Duration,
    stage: &str,
    kind: HttpError,
    error: &(dyn Error + 'static),
) {
    let mut os_error = None;
    let mut parts = Vec::new();
    for cause in http::error_chain(error) {
        let transport = cause.downcast_ref::<ureq::Transport>().or_else(|| {
            match cause.downcast_ref::<ureq::Error>() {
                Some(ureq::Error::Transport(transport)) => Some(transport),
                _ => None,
            }
        });
        if let Some(transport) = transport {
            // ureq 的 Display 包含完整 URL 和底层错误，仅保留这一层的类型和消息。
            let kind = transport.kind();
            parts.push(match transport.message() {
                Some(message) => format!("{kind:?}: {message}"),
                None => format!("{kind:?}"),
            });
            continue;
        }
        if let Some(io_error) = cause.downcast_ref::<io::Error>() {
            os_error = os_error.or_else(|| io_error.raw_os_error());
            if io_error.get_ref().is_some() {
                // 下一个 cause 会包含原始消息，避免重复输出。
                parts.push(format!("{:?}", io_error.kind()));
                continue;
            }
        }
        parts.push(cause.to_string());
    }
    let os_error = os_error.map_or_else(|| "none".into(), |code| code.to_string());
    let detail = redact_probe_error(&parts.join(" -> "));
    log_probe_failure(
        round,
        hostname,
        elapsed,
        stage,
        format_args!("error={kind}, os_error={os_error}, detail={detail}"),
    );
}

fn redact_probe_error(detail: &str) -> String {
    let mut safe = detail
        .split(|ch: char| ch.is_whitespace() || ch.is_control())
        .filter(|part| !part.is_empty())
        .map(|part| {
            if part.contains("://") || part.contains(['?', '=', '@']) {
                "[redacted]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if let Some((end, _)) = safe.char_indices().nth(PROBE_ERROR_CHARS) {
        safe.truncate(end);
        safe.push('…');
    }
    safe
}

fn log_probe_failure(
    round: u64,
    hostname: &str,
    elapsed: Duration,
    stage: &str,
    details: impl std::fmt::Display,
) {
    logging::info(&format!(
        "[PUBLIC_PROBE_FAILED] target={hostname}, probe_round={}, stage={stage}, elapsed_ms={}, {details}",
        probe_round_id(round),
        elapsed.as_millis()
    ));
}

fn probe_response(
    response: ureq::Response,
    expect_text: Option<&str>,
) -> (ProbeKind, Option<String>, Option<io::Error>) {
    let status = response.status();
    if let (200, Some(text)) = (status, expect_text) {
        let mut prefix = [0; PROBE_TEXT_LIMIT];
        let (len, error) = http::read_prefix_with_error(response.into_reader(), &mut prefix);
        let matched = text.is_empty()
            || prefix[..len]
                .windows(text.len())
                .any(|part| part == text.as_bytes());
        if matched && error.is_none() {
            return (ProbeKind::Ok, None, None);
        }
        let origin =
            captive_origin_from_prefix(&prefix[..len], error.is_some() || len == prefix.len());
        // 已收到 HTTP 响应，正文超时也属于响应异常，不能作为双路无响应的重连证据。
        return (ProbeKind::Unexpected(status), origin, error);
    }

    let origin = if matches!(status, 300..=399 | 511) {
        extract_captive_portal_origin(response.header("Location"), "")
    } else {
        None
    };
    http::drain_small_body(response);
    let kind = match status {
        200 => ProbeKind::Ok,
        201..=399 | 511 => ProbeKind::Unexpected(status),
        _ => ProbeKind::HttpStatus(status),
    };
    (kind, origin, None)
}

fn read_adapters() -> (Vec<String>, Vec<Ipv4Addr>) {
    let mut names = Vec::new();
    let mut ips = Vec::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if interface.name.to_lowercase().contains("loopback") {
                continue;
            }
            if let if_addrs::IfAddr::V4(address) = interface.addr {
                let ip = address.ip;
                if !ip.is_loopback() {
                    names.push(interface.name.clone());
                    ips.push(ip);
                }
            }
        }
    }
    (names, ips)
}

fn join_ips(ips: &[Ipv4Addr]) -> String {
    if ips.is_empty() {
        "none".to_string()
    } else {
        ips.iter()
            .map(|ip| ip.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn is_usable_ipv4(ip: Ipv4Addr) -> bool {
    !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified()
}

fn read_route_to(destination: Ipv4Addr) -> RoutePath {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetBestRoute2, MIB_IPFORWARD_ROW2};
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, IN_ADDR, IN_ADDR_0, SOCKADDR_IN, SOCKADDR_INET,
    };

    let dest = SOCKADDR_INET {
        Ipv4: SOCKADDR_IN {
            sin_family: AF_INET,
            sin_port: 0,
            sin_addr: IN_ADDR {
                S_un: IN_ADDR_0 {
                    S_addr: u32::from_ne_bytes(destination.octets()),
                },
            },
            sin_zero: [0; 8],
        },
    };
    // Source, next hop and interface all come from this one routing decision.
    // The zero interface index lets Windows select the current best interface.
    unsafe {
        let mut row: MIB_IPFORWARD_ROW2 = std::mem::zeroed();
        let mut source: SOCKADDR_INET = std::mem::zeroed();
        if GetBestRoute2(
            std::ptr::null(),
            0,
            std::ptr::null(),
            &dest,
            0,
            &mut row,
            &mut source,
        ) != 0
            || source.si_family != AF_INET
            || row.NextHop.si_family != AF_INET
        {
            return RoutePath::Unavailable;
        }
        RoutePath::Ipv4 {
            interface: row.InterfaceLuid.Value,
            source: Ipv4Addr::from(source.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes()),
            next_hop: Ipv4Addr::from(row.NextHop.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes()),
        }
    }
}

fn ping_gateway(gateway: Ipv4Addr) -> Option<bool> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let output = Command::new("ping")
        .args(["-n", "1", "-w", "1000", &gateway.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    Some(output.status.success())
}

#[allow(dead_code)]
fn debug_probe_summary(outcomes: &[ProbeOutcome]) -> String {
    outcomes
        .iter()
        .map(|outcome| format!("{}={}", outcome.hostname, outcome.kind.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn log_diagnostic(result: &DiagnosticResult, failures: u32) {
    let evidence = result.evidence_text();
    logging::warn(&format!(
        "[{}] 第 {} 次检测失败：{}{}",
        result.reason.as_str(),
        failures,
        result.message,
        if evidence.is_empty() {
            String::new()
        } else {
            format!("；{evidence}")
        }
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn spawn_http_once(response: String) -> String {
        let (base, _) = http::mock::serve(move |stream| {
            let _ = stream.write_all(response.as_bytes());
        });
        format!("{base}/test")
    }

    fn http_200(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    #[test]
    fn chunked_binary_icon_reuses_the_same_connection() {
        let (base, server) = http::mock::serve(|stream| {
            let icon = [0xff; http::DRAIN_LIMIT];
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/x-icon\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n", icon.len() - 1).unwrap();
            stream.write_all(&icon[1..]).unwrap();
            stream.write_all(b"\r\n0\r\n\r\n").unwrap();
            http::mock::read_request(stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                icon.len()
            )
            .unwrap();
            stream.write_all(&icon).unwrap();
            http::mock::read_request(stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        let target = ProbeTarget {
            url: Box::leak(format!("{base}/favicon.ico").into_boxed_str()),
            expect_text: None,
        };
        let agent = http::no_redirect_agent(5);
        for _ in 0..3 {
            assert_eq!(probe_once(&agent, &target, 0).kind, ProbeKind::Ok);
        }
        server.join().unwrap();
    }

    #[test]
    fn probes_close_large_streams_at_their_byte_budget() {
        for (status, expected_text, sent, kind) in [
            (200, None, http::DRAIN_LIMIT, ProbeKind::Ok),
            (
                511,
                Some("test"),
                http::DRAIN_LIMIT,
                ProbeKind::Unexpected(511),
            ),
            (
                503,
                Some("test"),
                http::DRAIN_LIMIT,
                ProbeKind::HttpStatus(503),
            ),
            (
                200,
                Some("test"),
                PROBE_TEXT_LIMIT,
                ProbeKind::Unexpected(200),
            ),
        ] {
            let (base, server) = http::mock::serve(move |stream| {
                // Declare a 16 MiB chunk but withhold everything past the client's budget.
                write!(
                    stream,
                    "HTTP/1.1 {status} Mock\r\nTransfer-Encoding: chunked\r\n\r\n1000000\r\n"
                )
                .unwrap();
                stream
                    .write_all(&[0xff; http::DRAIN_LIMIT][..sent])
                    .unwrap();
                match stream.read(&mut [0]) {
                    Ok(0) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                    other => panic!("expected client to drop the unfinished stream, got {other:?}"),
                }
            });
            let target = ProbeTarget {
                url: Box::leak(format!("{base}/test").into_boxed_str()),
                expect_text: expected_text,
            };
            assert_eq!(
                probe_once(&http::no_redirect_agent(8), &target, 0).kind,
                kind
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn text_marker_must_fit_entirely_in_the_prefix() {
        let microsoft = DEFAULT_PROBES[0];
        assert!(microsoft.url.starts_with("http://"));
        let marker = microsoft.expect_text.unwrap();
        for (end, kind) in [
            (PROBE_TEXT_LIMIT, ProbeKind::Ok),
            (PROBE_TEXT_LIMIT + 1, ProbeKind::Unexpected(200)),
        ] {
            let padding = "x".repeat(end - marker.len());
            // Split the marker over two reads, even when both chunks arrive together.
            let first = format!("{padding}Microsoft ");
            let response = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{first}\r\nc\r\nConnect Test\r\n0\r\n\r\n", first.len());
            let target = ProbeTarget {
                url: Box::leak(spawn_http_once(response).into_boxed_str()),
                expect_text: Some(marker),
            };
            assert_eq!(
                probe_once(&http::no_redirect_agent(3), &target, 0).kind,
                kind
            );
        }
    }

    #[test]
    fn captive_302_redirect_is_classified_as_unexpected() {
        let url = spawn_http_once(
            "HTTP/1.1 302 Found\r\nLocation: http://192.0.2.1/\r\nContent-Length: 0\r\n\r\n"
                .to_string(),
        );
        let agent = http::no_redirect_agent(3);
        let target = ProbeTarget {
            url: Box::leak(url.into_boxed_str()),
            expect_text: None,
        };
        let outcome = probe_once(&agent, &target, 0);
        assert_eq!(outcome.kind, ProbeKind::Unexpected(302));
        assert_eq!(outcome.kind.note(), "HTTP 302");
    }

    #[test]
    fn public_redirect_never_authorizes_login_without_portal_status() {
        for (location, expected_origin) in [
            ("http://192.168.1.1/login.html", None),
            (
                "http://192.168.125.21/login.html",
                Some("http://192.168.125.21"),
            ),
        ] {
            let target = ProbeTarget {
                url: Box::leak(
                    spawn_http_once(format!(
                        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n"
                    ))
                    .into_boxed_str(),
                ),
                expect_text: Some(http::CONNECT_TEST_BODY),
            };
            let redirect = probe_once(&http::no_redirect_agent(1), &target, 0);
            assert_eq!(redirect.kind, ProbeKind::Unexpected(302));
            assert_eq!(redirect.captive_origin.as_deref(), expected_origin);
            let another_failure = ProbeOutcome {
                kind: ProbeKind::Error(HttpError::Tls),
                hostname: "fixture",
                captive_origin: None,
            };
            let result = NetworkDiagnostics::new().classify_outcomes(
                &[redirect, another_failure],
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::LOCALHOST,
                vec![],
                |_| Some(true),
            );
            assert_eq!(result.reason, DisconnectReason::UplinkUnreachable);
            assert!(!result.can_authenticate);
        }
    }

    #[test]
    fn partial_response_keeps_a_complete_captive_origin_after_read_timeout() {
        let (release, gate) = mpsc::channel();
        let (base, server) = http::mock::serve(move |stream| {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<script>location.href='http://192.168.125.21/';</script>")
                .unwrap();
            gate.recv_timeout(Duration::from_secs(4)).unwrap();
        });
        let target = ProbeTarget {
            url: Box::leak(base.into_boxed_str()),
            expect_text: Some(http::CONNECT_TEST_BODY),
        };
        let agent = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .timeout(Duration::from_secs(1))
            .redirects(0)
            .build();
        let outcome = probe_once(&agent, &target, 0);
        release.send(()).unwrap();
        server.join().unwrap();
        assert_eq!(outcome.kind, ProbeKind::Unexpected(200));
        assert_eq!(
            outcome.captive_origin.as_deref(),
            Some("http://192.168.125.21")
        );
    }

    #[test]
    fn dns_blocked_with_offline_portal_becomes_authenticatable() {
        let portal_url = spawn_http_once(http_200(r#"dr1002({"result":0,"v46ip":"192.0.2.42"})"#));
        let diag =
            NetworkDiagnostics::new().with_campus_status(CampusStatusProbe::with_url(portal_url));
        let outcomes = vec![
            ProbeOutcome {
                kind: ProbeKind::Error(HttpError::Dns),
                hostname: "www.msftconnecttest.com",
                captive_origin: None,
            },
            ProbeOutcome {
                kind: ProbeKind::Error(HttpError::Dns),
                hostname: "www.baidu.com",
                captive_origin: None,
            },
        ];
        let res = diag.classify_outcomes(
            &outcomes,
            Ipv4Addr::new(198, 51, 100, 70),
            Ipv4Addr::new(198, 51, 100, 1),
            Vec::new(),
            |_| Some(true),
        );
        assert!(!res.online);
        assert_eq!(res.reason, DisconnectReason::AuthExpired);
        assert!(res.can_authenticate);
        assert_eq!(res.auth_ip(), "192.0.2.42");
    }

    #[test]
    fn dns_blocked_with_online_portal_stays_dns_failure() {
        let portal_url = spawn_http_once(http_200(r#"dr1002({"result":1})"#));
        let diag =
            NetworkDiagnostics::new().with_campus_status(CampusStatusProbe::with_url(portal_url));
        let outcomes = vec![ProbeOutcome {
            kind: ProbeKind::Error(HttpError::Dns),
            hostname: "www.baidu.com",
            captive_origin: None,
        }];
        let res = diag.classify_outcomes(
            &outcomes,
            Ipv4Addr::new(198, 51, 100, 70),
            Ipv4Addr::new(198, 51, 100, 1),
            Vec::new(),
            |_| Some(true),
        );
        assert!(!res.online);
        assert_eq!(res.reason, DisconnectReason::DnsFailure);
        assert!(!res.can_authenticate);
    }

    #[test]
    fn online_portal_prevents_login_for_redirect_and_public_service_error() {
        for (status, expected) in [
            ("302 Found", ProbeKind::Unexpected(302)),
            (
                "511 Network Authentication Required",
                ProbeKind::Unexpected(511),
            ),
            ("503 Service Unavailable", ProbeKind::HttpStatus(503)),
        ] {
            let url = spawn_http_once(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n"));
            let probe = probe_once(
                &http::no_redirect_agent(2),
                &ProbeTarget {
                    url: Box::leak(url.into_boxed_str()),
                    expect_text: None,
                },
                0,
            );
            assert_eq!(probe.kind, expected);
            let portal = spawn_http_once(http_200(r#"dr1002({"result":1})"#));
            let diag =
                NetworkDiagnostics::new().with_campus_status(CampusStatusProbe::with_url(portal));
            let timeout = ProbeOutcome {
                kind: ProbeKind::Error(HttpError::Timeout),
                hostname: "mock",
                captive_origin: None,
            };
            let result = diag.classify_outcomes(
                &[probe, timeout],
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::LOCALHOST,
                vec![],
                |_| Some(true),
            );
            assert_eq!(result.reason, DisconnectReason::UplinkUnreachable);
            assert!(!result.can_authenticate);
        }
    }

    #[test]
    fn a_successful_probe_returns_online_and_the_later_failure_is_logged() {
        let (running, started) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let gate = std::sync::Mutex::new(gate);
        let resolver = move |_: &str| {
            running.send(()).unwrap();
            gate.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
            Err(io::Error::other(io::Error::from_raw_os_error(11001)))
        };
        let agent = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .resolver(resolver)
            .build();
        let slow = probe_worker(move |round| {
            probe_once(
                &agent,
                &ProbeTarget {
                    url: "https://late-probe.invalid/check?marker=omit-this-value",
                    expect_text: None,
                },
                round,
            )
        });
        let fast = probe_worker(move |_| {
            started.recv_timeout(Duration::from_secs(3)).unwrap();
            ProbeOutcome {
                kind: ProbeKind::Ok,
                hostname: "fast",
                captive_origin: None,
            }
        });
        let diag = NetworkDiagnostics {
            probes: vec![slow, fast],
            campus_status: None,
        };
        let logs = logging::subscribe();
        let (finished, result) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let mut evidence = Vec::new();
            let outcomes = diag.run_probes(&mut evidence);
            let _ = finished.send(diag.classify_outcomes(
                &outcomes,
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::UNSPECIFIED,
                evidence,
                |_| panic!("a successful probe needs no gateway check"),
            ));
        });
        let result = result.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        handle.join().unwrap();
        let result = result.unwrap();
        assert!(result.online);
        let round = result
            .evidence
            .iter()
            .find(|(key, _)| key == "probe_round")
            .unwrap();
        let failure = std::iter::from_fn(|| logs.recv_timeout(Duration::from_secs(2)).ok())
            .find(|line| line.contains("[PUBLIC_PROBE_FAILED] target=late-probe.invalid,"))
            .expect("the pending probe must log its failure after the round returned");
        assert!(failure.contains("stage=dns, elapsed_ms="));
        assert!(failure.contains(&format!("probe_round={},", round.1)));
        assert!(failure.contains("error=DnsError, os_error=11001"));
        assert!(failure.contains("detail=Dns: resolve dns name"));
        assert!(!failure.contains("omit-this-value"));
    }

    #[test]
    fn probe_error_details_redact_urls_and_keep_a_bounded_single_line() {
        let description =
            "证书失败\r\nfrom https://probe-user@localhost/test?key=value to /test?key=other";
        assert_eq!(
            redact_probe_error(description),
            "证书失败 from [redacted] to [redacted]"
        );
        let long = "字".repeat(PROBE_ERROR_CHARS + 1);
        assert_eq!(
            redact_probe_error(&long),
            format!("{}…", "字".repeat(PROBE_ERROR_CHARS))
        );
    }

    #[test]
    fn nested_ureq_error_logs_the_os_cause_once() {
        let logs = logging::subscribe();
        let error = io::Error::other(ureq::Error::from(io::Error::from_raw_os_error(10054)));
        log_probe_error(
            0,
            "headers-probe.invalid",
            Duration::ZERO,
            "http_headers",
            HttpError::Connect,
            &error,
        );
        let failure = std::iter::from_fn(|| logs.recv_timeout(Duration::from_secs(2)).ok())
            .find(|line| line.contains("[PUBLIC_PROBE_FAILED] target=headers-probe.invalid,"))
            .unwrap();
        assert!(failure.contains("error=ConnectionError, os_error=10054"));
        assert_eq!(failure.matches("(os error 10054)").count(), 1);
    }

    #[test]
    fn ordinary_http_error_without_portal_confirmation_never_submits_credentials() {
        let d = NetworkDiagnostics::new();
        let failure = ProbeOutcome {
            kind: ProbeKind::HttpStatus(503),
            hostname: "mock",
            captive_origin: None,
        };
        let status = d.classify_outcomes(
            &[failure],
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::LOCALHOST,
            vec![],
            |_| Some(true),
        );
        assert!(!status.can_authenticate);
    }

    #[test]
    fn windows_route_lookup_pairs_loopback_source_and_interface() {
        let RoutePath::Ipv4 {
            interface, source, ..
        } = read_route_to(Ipv4Addr::LOCALHOST)
        else {
            panic!("Windows loopback route is missing");
        };
        assert_ne!(interface, 0);
        assert!(source.is_loopback());
    }

    #[test]
    fn successful_public_probe_on_an_on_link_route_is_online() {
        let diag = NetworkDiagnostics::new();
        for (kind, expected) in [
            (ProbeKind::Ok, DisconnectReason::Online),
            (
                ProbeKind::Error(HttpError::Other),
                DisconnectReason::UplinkUnreachable,
            ),
        ] {
            let outcome = ProbeOutcome {
                kind,
                hostname: "fixture",
                captive_origin: None,
            };
            let result = diag.classify_outcomes(
                &[outcome],
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::UNSPECIFIED,
                vec![],
                |_| panic!("on-link has no next hop"),
            );
            assert_eq!(result.reason, expected);
            assert!(!result.can_authenticate);
            assert!(result.gateway.is_empty());
        }
    }

    #[test]
    fn an_on_link_route_can_use_an_explicitly_offline_session() {
        let portal = spawn_http_once(http_200(r#"{"result":0}"#));
        let diag =
            NetworkDiagnostics::new().with_campus_status(CampusStatusProbe::with_url(portal));
        let failure = ProbeOutcome {
            kind: ProbeKind::Error(HttpError::Other),
            hostname: "fixture",
            captive_origin: None,
        };
        let r = diag.classify_outcomes(
            &[failure],
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::UNSPECIFIED,
            vec![],
            |_| panic!("no gateway"),
        );
        assert_eq!(r.reason, DisconnectReason::AuthExpired);
        assert!(r.can_authenticate && r.gateway.is_empty());
    }

    #[test]
    fn mixed_dns_and_transport_failures_follow_portal_evidence() {
        let outcomes = [HttpError::Dns, HttpError::Other].map(|error| ProbeOutcome {
            kind: ProbeKind::Error(error),
            hostname: "fixture",
            captive_origin: None,
        });
        for (reply, reason, can_auth) in [
            (
                "<html>unknown</html>",
                DisconnectReason::UplinkUnreachable,
                false,
            ),
            (
                r#"{"result":1}"#,
                DisconnectReason::UplinkUnreachable,
                false,
            ),
            (r#"{"result":0}"#, DisconnectReason::AuthExpired, true),
        ] {
            let diag = NetworkDiagnostics::new().with_campus_status(CampusStatusProbe::with_url(
                spawn_http_once(http_200(reply)),
            ));
            let result = diag.classify_outcomes(
                &outcomes,
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::LOCALHOST,
                vec![],
                |_| Some(true),
            );
            assert_eq!((result.reason, result.can_authenticate), (reason, can_auth));
        }
    }

    #[test]
    fn public_timeouts_with_unknown_session_require_a_verified_gateway() {
        let outcomes = [ProbeOutcome {
            kind: ProbeKind::Error(HttpError::Timeout),
            hostname: "fixture",
            captive_origin: None,
        }];
        let diag = NetworkDiagnostics::new();
        for gateway in [Some(false), None, Some(true)] {
            let r = diag.classify_outcomes(
                &outcomes,
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::LOCALHOST,
                vec![],
                |_| gateway,
            );
            assert_eq!(r.can_authenticate, gateway == Some(true));
        }
    }

    #[test]
    fn failed_probes_keep_all_evidence() {
        let probes = ["a", "b"]
            .into_iter()
            .map(|hostname| {
                probe_worker(move |_| ProbeOutcome {
                    kind: ProbeKind::Error(HttpError::Dns),
                    hostname,
                    captive_origin: None,
                })
            })
            .collect();
        let diag = NetworkDiagnostics {
            probes,
            campus_status: None,
        };
        let mut evidence = vec![];
        assert_eq!(diag.run_probes(&mut evidence).len(), 2);
        evidence.sort();
        assert_eq!(evidence[0].0, "probe_a");
        assert_eq!(evidence[1].0, "probe_b");
        assert_eq!(evidence[2].0, "probe_round");
    }

    #[test]
    fn portal_latency_requires_a_successful_recognizable_portal_response() {
        for (response, measured) in [
            (http_200(r#"dr1002({"result":1})"#), true),
            (
                http_200(&format!(
                    r#"dr1002({{"result":1}}){}"#,
                    " ".repeat(32 * 1024)
                )),
                false,
            ),
            (http_200("<html>error</html>"), false),
            (
                http_200(r#"dr1002({"result":1})"#).replace("200 OK", "503 Unavailable"),
                false,
            ),
        ] {
            let probe = CampusStatusProbe::with_url(spawn_http_once(response));
            assert_eq!(probe.measure_rtt_ms().is_some(), measured);
        }
    }
}
