//! HTTP 小封装。
//!
//! 校园门户和探测均直连，忽略 `HTTP_PROXY` 和 `HTTPS_PROXY` 环境变量，
//! 否则本机 Clash/mihomo 的重启会被误判成断网。

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpError {
    Timeout,
    Dns,
    Connect,
    Other,
}

impl HttpError {
    /// 返回日志与诊断证据使用的错误分类。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "Timeout",
            Self::Dns => "DnsError",
            Self::Connect => "ConnectionError",
            Self::Other => "Error",
        }
    }
}

/// 创建 agent：连接超时 3 秒，读超时按调用方给的值。
pub fn agent(read_timeout_secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(Duration::from_secs(read_timeout_secs + 3))
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(read_timeout_secs))
        .build()
}

/// 创建不跟随重定向的 agent，用于识别公网探测中的门户重定向。
pub fn no_redirect_agent(read_timeout_secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(Duration::from_secs(read_timeout_secs + 3))
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(read_timeout_secs))
        .redirects(0)
        .build()
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// GET 一个 URL。4xx/5xx 也返回 `Ok`，
/// 只有网络层失败才是 `Err`。
pub fn get(agent: &ureq::Agent, url: &str) -> Result<HttpResponse, HttpError> {
    match agent
        .get(url)
        .set("User-Agent", "GXNUCampusLogin/4.3")
        .call()
    {
        Ok(response) => {
            let status = response.status();
            match response.into_string() {
                Ok(body) => Ok(HttpResponse { status, body }),
                Err(err) => Err(classify(&err.to_string())),
            }
        }
        Err(ureq::Error::Status(status, response)) => Ok(HttpResponse {
            status,
            body: response.into_string().unwrap_or_default(),
        }),
        Err(ureq::Error::Transport(transport)) => Err(classify(&transport.to_string())),
    }
}

fn classify(text: &str) -> HttpError {
    let lower = text.to_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        HttpError::Timeout
    } else if lower.contains("dns")
        || lower.contains("resolve")
        || lower.contains("lookup")
        || lower.contains("name or service")
    {
        HttpError::Dns
    } else if lower.contains("refused") || lower.contains("connect") || lower.contains("reset") {
        HttpError::Connect
    } else {
        HttpError::Other
    }
}

/// 查询串转义。
pub fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(urlencode("demo-student"), "demo-student");
        assert_eq!(urlencode("demo@example"), "demo%40example");
        assert_eq!(urlencode("a b+c"), "a%20b%2Bc");
        assert_eq!(urlencode("中"), "%E4%B8%AD");
    }

    #[test]
    fn classify_maps_transport_messages() {
        assert_eq!(classify("Connection timed out"), HttpError::Timeout);
        assert_eq!(
            classify("failed to lookup address information"),
            HttpError::Dns
        );
        assert_eq!(classify("connection refused"), HttpError::Connect);
        assert_eq!(classify("something odd"), HttpError::Other);
    }
}
