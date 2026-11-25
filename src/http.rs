//! HTTP 小封装。
//!
//! 校园门户和探测均直连，忽略 `HTTP_PROXY` 和 `HTTPS_PROXY` 环境变量，
//! 否则本机 Clash/mihomo 的重启会被误判成断网。

use std::error::Error;
use std::io::{self, Read};
use std::time::Duration;

pub(crate) const DRAIN_LIMIT: usize = 16 * 1024;
pub(crate) const CONNECT_TEST_URL: &str = "http://www.msftconnecttest.com/connecttest.txt";
pub(crate) const CONNECT_TEST_BODY: &str = "Microsoft Connect Test";
const PORTAL_BODY_LIMIT: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpError {
    Timeout,
    Dns,
    Connect,
    Tls,
    BodyTooLarge(usize),
    Other,
}

impl HttpError {
    /// 返回日志与诊断证据使用的错误分类。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "Timeout",
            Self::Dns => "DnsError",
            Self::Connect => "ConnectionError",
            Self::Tls => "TlsError",
            Self::BodyTooLarge(_) => "ResponseTooLarge",
            Self::Other => "Error",
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BodyTooLarge(limit) => write!(f, "响应正文超过 {limit} 字节上限"),
            _ => f.write_str(self.as_str()),
        }
    }
}

impl From<io::Error> for HttpError {
    fn from(error: io::Error) -> Self {
        Self::from(&error)
    }
}

impl From<&io::Error> for HttpError {
    fn from(error: &io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::TimedOut => Self::Timeout,
            io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::NotConnected
            | io::ErrorKind::BrokenPipe => Self::Connect,
            _ => Self::Other,
        }
    }
}

/// 直连且不跟随重定向，适用于公网探测、门户校验和认证接口。
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

/// 保留所有 HTTP 状态，由调用方决定是否需要正文。
pub(crate) fn get_response(
    agent: &ureq::Agent,
    url: &str,
) -> Result<ureq::Response, Box<ureq::Transport>> {
    match agent
        .get(url)
        .set("User-Agent", "GXNUCampusLogin/4.3")
        .call()
    {
        Ok(response) | Err(ureq::Error::Status(_, response)) => Ok(response),
        Err(ureq::Error::Transport(transport)) => Err(Box::new(transport)),
    }
}

pub fn get_portal(agent: &ureq::Agent, url: &str) -> Result<HttpResponse, HttpError> {
    let response = get_response(agent, url).map_err(|error| classify_transport(&error).0)?;
    let status = response.status();
    let body = if (200..300).contains(&status) {
        read_text(response, PORTAL_BODY_LIMIT)?
    } else {
        drain_small_body(response);
        String::new()
    };
    Ok(HttpResponse { status, body })
}

/// 尽量填满前缀缓冲区，允许短读，在 EOF 或缓冲区末尾停止。
pub(crate) fn read_prefix(reader: impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let (filled, error) = read_prefix_with_error(reader, buf);
    error.map_or(Ok(filled), Err)
}

/// 即使读取中断，也保留已收到的字节数供调用者检查前缀。
pub(crate) fn read_prefix_with_error(
    mut reader: impl Read,
    buf: &mut [u8],
) -> (usize, Option<io::Error>) {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return (filled, Some(e)),
        }
    }
    (filled, None)
}

/// 小正文读完即可复用连接。达到预算或读取失败时丢弃流，状态码仍有效。
pub fn drain_small_body(response: ureq::Response) {
    let mut reader = response.into_reader().take(DRAIN_LIMIT as u64);
    let mut buf = [0; 1024];
    while let Ok(n) = read_prefix(&mut reader, &mut buf) {
        if n < buf.len() {
            break;
        }
    }
}

/// 限制解压后的完整正文。额外的一个判界字节放在栈上，避免为了判界扩大正文缓冲区。
pub fn read_text(response: ureq::Response, limit: usize) -> Result<String, HttpError> {
    if response.header("Transfer-Encoding").is_none()
        && response
            .header("Content-Length")
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|len| len > limit as u64)
    {
        return Err(HttpError::BodyTooLarge(limit));
    }
    let mut reader = response.into_reader();
    let mut bytes = Vec::new();
    reader.by_ref().take(limit as u64).read_to_end(&mut bytes)?;
    if bytes.len() == limit && read_prefix(reader, &mut [0])? != 0 {
        return Err(HttpError::BodyTooLarge(limit));
    }
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned()))
}

pub(crate) fn classify_transport(error: &ureq::Transport) -> (HttpError, &'static str) {
    use ureq::ErrorKind;

    // ureq 2 的 rustls 后端和 TCP 拨号共用 ConnectionFailed，用库自身的阶段消息区分。
    let (kind, stage) = match (error.kind(), error.message()) {
        (_, Some("tls connection creation failed" | "tls connection init failed")) => {
            (HttpError::Tls, "tls")
        }
        (ErrorKind::Dns, _) => return (HttpError::Dns, "dns"),
        (ErrorKind::ConnectionFailed, _) => (HttpError::Connect, "tcp"),
        (ErrorKind::BadStatus | ErrorKind::BadHeader, _) => (HttpError::Other, "http_headers"),
        _ => (HttpError::Other, "request"),
    };
    let io_kind = error_chain(error)
        .filter_map(|cause| cause.downcast_ref::<io::Error>())
        .map(HttpError::from)
        .find(|&kind| kind != HttpError::Other);
    match (kind, io_kind) {
        (_, Some(HttpError::Timeout)) => (HttpError::Timeout, stage),
        (HttpError::Other, Some(kind)) => (kind, stage),
        _ => (kind, stage),
    }
}

/// 保留 io::Error 包裹的底层错误，Error::source() 可能直接跳过这一层。
pub(crate) fn error_chain<'a>(
    error: &'a (dyn Error + 'static),
) -> impl Iterator<Item = &'a (dyn Error + 'static)> {
    std::iter::successors(Some(error), |&cause| {
        cause
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|inner| inner as &(dyn Error + 'static))
            .or_else(|| cause.source())
    })
    .take(8)
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
pub(crate) mod mock {
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    pub fn read_request(stream: &mut TcpStream) {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                reader.read_line(&mut line).unwrap(),
                0,
                "incomplete mock request"
            );
            if line == "\r\n" {
                return;
            }
        }
    }

    /// Accept exactly one connection. A keep-alive test can read another request on that socket.
    pub fn serve(
        handler: impl FnOnce(&mut TcpStream) + Send + 'static,
    ) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            drop(listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            read_request(&mut stream);
            handler(&mut stream);
        });
        (base, server)
    }
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
    fn winsock_timeout_uses_the_error_kind() {
        assert_eq!(
            HttpError::from(io::Error::from_raw_os_error(10060)),
            HttpError::Timeout
        );
    }

    #[test]
    fn dns_resolution_timeout_stays_a_dns_failure() {
        let agent = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .resolver(|_: &str| Err(io::Error::from_raw_os_error(10060)))
            .build();
        let error = get_response(&agent, "http://resolver-timeout.invalid/").unwrap_err();
        assert_eq!(classify_transport(&error), (HttpError::Dns, "dns"));
    }

    #[test]
    fn classifies_https_handshake_failure_at_the_tls_stage() {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("https://{}/test", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut record = [0; 5];
            stream.read_exact(&mut record).unwrap();
            let mut hello = vec![0; u16::from_be_bytes([record[3], record[4]]) as usize];
            stream.read_exact(&mut hello).unwrap();
            // 发送 TLS fatal handshake_failure alert。
            stream.write_all(&[21, 3, 3, 0, 2, 2, 40]).unwrap();
        });
        let error = get_response(&no_redirect_agent(3), &url).unwrap_err();
        server.join().unwrap();
        assert_eq!(classify_transport(&error), (HttpError::Tls, "tls"));
    }
}
