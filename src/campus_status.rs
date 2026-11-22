//! 问门户“我这个出口现在到底在不在线”。
//!
//! 查询 `/drcom/chkstatus`，
//! `result = 1 在线 / 0 不在线`。它免凭据、直连、体积小，用来区分
//! “校园会话掉线”和“代理节点/上游出口不通”。

use std::time::Instant;

use crate::http;
use crate::logging;

pub const CHKSTATUS_URL: &str = "https://yc.gxnu.edu.cn/drcom/chkstatus";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusStatusInfo {
    pub online: bool,
    /// 门户侧看到的真实校园网 IPv4（例如经路由器或 TUN 出口后的 `172.16.x.x`）。
    pub portal_ip: Option<String>,
}

pub struct CampusStatusProbe {
    agent: ureq::Agent,
    url: String,
}

impl Default for CampusStatusProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl CampusStatusProbe {
    pub fn new() -> Self {
        Self {
            agent: http::agent(5),
            url: CHKSTATUS_URL.to_string(),
        }
    }

    pub fn with_url(url: impl Into<String>) -> Self {
        Self {
            agent: http::agent(5),
            url: url.into(),
        }
    }

    /// 静默测量到校园网认证网关（yc.gxnu.edu.cn）的真实应用层往返延迟（毫秒）。
    /// 使用完整 HTTP 响应而非纯 TCP 握手，避免 TUN/代理虚拟网卡本地秒回 ACK 导致的假 1ms 延迟。
    pub fn measure_rtt_ms(&self) -> Option<u64> {
        self.measure_rtt_and_ip().map(|(ms, _)| ms)
    }

    /// 静默测量网关 RTT，同时顺带解析门户看到的真实校园网 IPv4（穿透代理 TUN，零额外请求）。
    pub fn measure_rtt_and_ip(&self) -> Option<(u64, Option<String>)> {
        let url = format!("{}?callback=dr1002&v=4532&lang=zh", self.url);
        let started = Instant::now();
        match http::get_portal(&self.agent, &url) {
            Ok(response) if (200..300).contains(&response.status) => {
                let detail = parse_chkstatus_detail(&response.body)?;
                Some((
                    started.elapsed().as_millis().max(1) as u64,
                    detail.portal_ip,
                ))
            }
            _ => None,
        }
    }

    /// `Some(true)` 在线、`Some(false)` 不在线、`None` 查不到/看不懂。
    pub fn check(&self) -> Option<bool> {
        self.check_detail().map(|info| info.online)
    }

    /// 返回门户在线状态以及门户侧识别到的真实校园网 IPv4。
    pub fn check_detail(&self) -> Option<CampusStatusInfo> {
        let url = format!("{}?callback=dr1002&v=4532&lang=zh", self.url);
        match http::get_portal(&self.agent, &url) {
            Ok(response) => {
                let detail = (200..300)
                    .contains(&response.status)
                    .then(|| parse_chkstatus_detail(&response.body))
                    .flatten();
                match &detail {
                    Some(info) => {
                        let ip_note = info
                            .portal_ip
                            .as_deref()
                            .map(|ip| format!("（门户识别IP={ip}）"))
                            .unwrap_or_default();
                        logging::info(&format!(
                            "[CAMPUS_STATUS] 门户判定本次出口{}{}",
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
