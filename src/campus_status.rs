//! 问门户“我这个出口现在到底在不在线”。
//!
//! 查询 `/drcom/chkstatus`，
//! `result = 1 在线 / 0 不在线`。它免凭据、直连、体积小，用来区分
//! “校园会话掉线”和“代理节点/上游出口不通”，并支持多校区网关自动探测与故障切换。

use std::sync::Arc;
use std::time::Instant;

use crate::http;
use crate::logging;
use crate::portal_gateway::{probe_chkstatus_once, PortalGatewayResolver};

pub const CHKSTATUS_URL: &str = "https://yc.gxnu.edu.cn/drcom/chkstatus";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusStatusInfo {
    pub online: bool,
    /// 门户侧看到的真实校园网 IPv4（例如经路由器或 TUN 出口后的 `172.16.x.x`）。
    pub portal_ip: Option<String>,
    /// 本次响应所属的网关简短名称（如 `"育才网关"` / `"雁山网关"`）。
    pub gateway_label: Option<String>,
}

enum ProbeTarget {
    FixedUrl(String),
    Dynamic(Arc<PortalGatewayResolver>),
}

pub struct CampusStatusProbe {
    agent: ureq::Agent,
    target: ProbeTarget,
}

impl Default for CampusStatusProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl CampusStatusProbe {
    pub fn new() -> Self {
        Self {
            agent: http::no_redirect_agent(5),
            target: ProbeTarget::FixedUrl(CHKSTATUS_URL.to_string()),
        }
    }

    pub fn with_url(url: impl Into<String>) -> Self {
        Self {
            agent: http::no_redirect_agent(5),
            target: ProbeTarget::FixedUrl(url.into()),
        }
    }

    pub fn with_resolver(resolver: Arc<PortalGatewayResolver>) -> Self {
        Self {
            agent: http::no_redirect_agent(5),
            target: ProbeTarget::Dynamic(resolver),
        }
    }

    /// 静默测量到当前生效校园网认证网关的真实应用层往返延迟（毫秒）。
    pub fn measure_rtt_ms(&self) -> Option<u64> {
        self.measure_rtt_ip_and_gateway().map(|(ms, _, _)| ms)
    }

    /// 静默测量网关 RTT，同时顺带解析门户看到的真实校园网 IPv4。
    pub fn measure_rtt_and_ip(&self) -> Option<(u64, Option<String>)> {
        self.measure_rtt_ip_and_gateway()
            .map(|(ms, ip, _)| (ms, ip))
    }

    /// 静默测量网关 RTT，并返回 `(毫秒, 真实校园网 IPv4, 生效网关简短名)`。
    pub fn measure_rtt_ip_and_gateway(&self) -> Option<(u64, Option<String>, Option<String>)> {
        match &self.target {
            ProbeTarget::FixedUrl(base_url) => {
                let url = format!("{base_url}?callback=dr1002&v=4532&lang=zh");
                let started = Instant::now();
                let response = http::get_portal(&self.agent, &url).ok()?;
                if !(200..300).contains(&response.status) {
                    return None;
                }
                let detail = parse_chkstatus_detail(&response.body)?;
                Some((
                    started.elapsed().as_millis().max(1) as u64,
                    detail.portal_ip,
                    detail.gateway_label,
                ))
            }
            ProbeTarget::Dynamic(resolver) => {
                if let Some(portal) = resolver.cached_endpoints() {
                    if let Some((ms, info)) = probe_chkstatus_once(&self.agent, &portal) {
                        return Some((ms, info.portal_ip, Some(portal.short_name)));
                    }
                    if resolver.is_custom_mode() {
                        return None;
                    }
                    resolver.invalidate_if_origin(&portal.origin);
                }
                if let Ok((fresh, ms, info)) = resolver.probe_and_cache() {
                    return Some((ms, info.portal_ip, Some(fresh.short_name)));
                }
                None
            }
        }
    }

    /// `Some(true)` 在线、`Some(false)` 不在线、`None` 查不到/看不懂。
    pub fn check(&self) -> Option<bool> {
        self.check_detail().map(|info| info.online)
    }

    /// 返回门户在线状态以及门户侧识别到的真实校园网 IPv4。
    pub fn check_detail(&self) -> Option<CampusStatusInfo> {
        self.check_detail_with_hint(None)
    }

    pub(crate) fn check_detail_with_hint(
        &self,
        captive_origin: Option<&str>,
    ) -> Option<CampusStatusInfo> {
        match &self.target {
            ProbeTarget::FixedUrl(base_url) => self.check_single_url(base_url, None),
            ProbeTarget::Dynamic(resolver) => {
                if let Some(portal) = resolver.cached_endpoints().filter(|portal| {
                    captive_origin.is_none_or(|origin| {
                        origin == portal.origin || portal.is_https_alternative_to(origin)
                    })
                }) {
                    if let Some(info) =
                        self.check_single_url(&portal.chkstatus_url, Some(&portal.short_name))
                    {
                        return Some(info);
                    }
                    if resolver.is_custom_mode() {
                        return None;
                    }
                    resolver.invalidate_if_origin(&portal.origin);
                }
                if let Ok((fresh, _ms, info)) = resolver.probe_and_cache_with_hint(captive_origin) {
                    logging::info(&format!(
                        "[CAMPUS_STATUS] 已自动探测网关「{}」，门户判定{}",
                        fresh.label,
                        if info.online { "在线" } else { "不在线" }
                    ));
                    return Some(info);
                }
                None
            }
        }
    }

    fn check_single_url(
        &self,
        chkstatus_url: &str,
        gateway_short_name: Option<&str>,
    ) -> Option<CampusStatusInfo> {
        let url = format!("{chkstatus_url}?callback=dr1002&v=4532&lang=zh");
        match http::get_portal(&self.agent, &url) {
            Ok(response) => {
                let mut detail = (200..300)
                    .contains(&response.status)
                    .then(|| parse_chkstatus_detail(&response.body))
                    .flatten();
                if let (Some(info), Some(gw)) = (detail.as_mut(), gateway_short_name) {
                    info.gateway_label = Some(gw.to_string());
                }
                match &detail {
                    Some(info) => {
                        let gw_note = info
                            .gateway_label
                            .as_deref()
                            .map(|g| format!("（{g}）"))
                            .unwrap_or_default();
                        let ip_note = info
                            .portal_ip
                            .as_deref()
                            .map(|ip| format!("（门户识别IP={ip}）"))
                            .unwrap_or_default();
                        logging::info(&format!(
                            "[CAMPUS_STATUS] 门户{}判定本次出口{}{}",
                            gw_note,
                            if info.online { "在线" } else { "不在线" },
                            ip_note
                        ));
                    }
                    None => logging::info(&format!(
                        "[CAMPUS_STATUS] 门户响应无法识别（HTTP {}）",
                        response.status
                    )),
                }
                detail
            }
            Err(err) => {
                logging::info(&format!("[CAMPUS_STATUS] 无法查询门户在线状态：{err}"));
                None
            }
        }
    }
}

pub fn parse_chkstatus(text: &str) -> Option<bool> {
    parse_chkstatus_detail(text).map(|info| info.online)
}

pub fn parse_chkstatus_detail(text: &str) -> Option<CampusStatusInfo> {
    let payload = jsonp_payload(text)?;
    let value: serde_json::Value = serde_json::from_str(&payload).ok()?;
    let result = value.get("result")?;
    let number = result.as_i64().or_else(|| {
        result
            .as_str()
            .and_then(|raw| raw.trim().parse::<i64>().ok())
    })?;
    if !matches!(number, 0 | 1) {
        return None;
    }
    let portal_ip = ["v46ip", "v4ip", "lip", "ss5"].into_iter().find_map(|key| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|ip| is_valid_portal_ipv4(ip))
            .map(str::to_string)
    });
    Some(CampusStatusInfo {
        online: number == 1,
        portal_ip,
        gateway_label: None,
    })
}

fn is_valid_portal_ipv4(ip: &str) -> bool {
    let Ok(parsed) = ip.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    !parsed.is_unspecified() && !parsed.is_loopback() && !parsed.is_link_local()
}

/// 从 `dr1002({...})` 里取出 JSON 正文；也接受裸 JSON。
pub fn jsonp_payload(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if let (Some(start), Some(end)) = (trimmed.find('('), trimmed.rfind(')')) {
        if end > start {
            let inner = trimmed[start + 1..end].trim();
            if inner.starts_with('{') {
                return Some(inner.to_string());
            }
        }
    }
    if trimmed.starts_with('{') {
        return Some(trimmed.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_reply_is_detected() {
        let body = r#"dr1002({"result":1,"v4ip":"192.0.2.42","stime":"2000-01-01 12:00:00"});"#;
        assert_eq!(parse_chkstatus(body), Some(true));
        let detail = parse_chkstatus_detail(body).unwrap();
        assert!(detail.online);
        assert_eq!(detail.portal_ip.as_deref(), Some("192.0.2.42"));
    }

    #[test]
    fn offline_reply_extracts_v46ip() {
        let body = r#"dr1002({"result":0,"v46ip":"192.0.2.42"});"#;
        let detail = parse_chkstatus_detail(body).unwrap();
        assert!(!detail.online);
        assert_eq!(detail.portal_ip.as_deref(), Some("192.0.2.42"));
    }

    #[test]
    fn offline_reply_is_detected() {
        assert_eq!(parse_chkstatus(r#"dr1002({"result":0});"#), Some(false));
    }

    #[test]
    fn string_result_is_accepted() {
        assert_eq!(parse_chkstatus(r#"dr1002({"result":"1"});"#), Some(true));
        assert_eq!(parse_chkstatus(r#"dr1002({"result":"0"});"#), Some(false));
    }

    #[test]
    fn captive_html_is_unknown() {
        assert_eq!(parse_chkstatus("<html><body>请先认证</body></html>"), None);
    }

    #[test]
    fn bare_json_without_callback_is_accepted() {
        assert_eq!(parse_chkstatus(r#"{"result":1}"#), Some(true));
    }
}
