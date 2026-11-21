//! 门户认证。
//!
//! Dr.COM 的失败响应里 `msg` 是数字码、`msga` 才是人话，两者都保留。

use crate::campus_status::jsonp_payload;
use crate::http;
use crate::logging;
use crate::models::DisconnectReason;

pub const PORTAL_URL: &str = "https://yc.gxnu.edu.cn/drcom/login";

#[derive(Debug, Clone)]
pub struct AuthenticationResult {
    pub success: bool,
    pub reason: DisconnectReason,
    pub message: String,
    /// 门户原始返回码（`msg` 数字码或 `ret_code`），只用于日志与排查。
    pub code: String,
}

pub struct CampusAuthenticator {
    agent: ureq::Agent,
    portal_url: String,
}

impl Default for CampusAuthenticator {
    fn default() -> Self {
        Self::new()
    }
}

impl CampusAuthenticator {
    pub fn new() -> Self {
        Self {
            agent: http::agent(10),
            portal_url: PORTAL_URL.to_string(),
        }
    }

    pub fn with_url(url: impl Into<String>) -> Self {
        Self {
            agent: http::agent(10),
            portal_url: url.into(),
        }
    }

    pub fn authenticate(
        &self,
        student_id: &str,
        password: &str,
        carrier: &str,
        local_ip: &str,
    ) -> AuthenticationResult {
        self.authenticate_if(student_id, password, carrier, local_ip, || true)
            .expect("unconditional request")
    }

    pub fn authenticate_if(
        &self,
        student_id: &str,
        password: &str,
        carrier: &str,
        local_ip: &str,
        begin: impl FnOnce() -> bool,
    ) -> Option<AuthenticationResult> {
        if student_id.is_empty() || password.is_empty() {
            return Some(AuthenticationResult {
                success: false,
                reason: DisconnectReason::CredentialsMissing,
                message: "缺少学号或密码".to_string(),
                code: String::new(),
            });
        }
        let carrier = if matches!(carrier, "0" | "1" | "2" | "3") {
            carrier
        } else {
            "2"
        };
        let url = format!(
            "{}?callback=dr1004&DDDDD={}&upass={}&0MKKey=123456&R1=0&R2=&R3={}&R6=0&para=00\
             &v4ip={}&v6ip=&terminal_type=1&lang=zh&jsVersion=4.2.2&v=4532",
            self.portal_url,
            http::urlencode(student_id),
            http::urlencode(password),
            carrier,
            http::urlencode(local_ip),
        );

        // Commit this request only after preparing it and immediately before network I/O.
        if !begin() {
            return None;
        }
        Some(match http::get(&self.agent, &url) {
            Ok(response) => {
                let reply = (200..300)
                    .contains(&response.status)
                    .then(|| portal_reply(&response.body))
                    .flatten();
                match reply {
                    Some((success, message, code)) => AuthenticationResult {
                        success,
                        reason: if success {
                            DisconnectReason::AuthExpired
                        } else {
                            DisconnectReason::LoginRejected
                        },
                        message: sanitize(&message, &[student_id, password]),
                        code: sanitize(&code, &[student_id, password]),
                    },
                    None => AuthenticationResult {
                        success: false,
                        reason: DisconnectReason::PortalUnavailable,
                        message: format!("无法识别认证服务器响应（HTTP {}）", response.status),
                        code: String::new(),
                    },
                }
            }
            Err(err) => {
                logging::warn(&format!("[PORTAL_ERROR] 认证请求失败：{}", err.as_str()));
                AuthenticationResult {
                    success: false,
                    reason: DisconnectReason::PortalUnavailable,
                    message: format!("认证服务器请求失败：{}", err.as_str()),
                    code: String::new(),
                }
            }
        })
    }
}

fn result_is_success(value: &serde_json::Value) -> bool {
    if let Some(number) = value.as_i64() {
        return number == 1;
    }
    if let Some(flag) = value.as_bool() {
        return flag;
    }
    match value.as_str() {
        Some(text) => matches!(
            text.trim().to_lowercase().as_str(),
            "1" | "true" | "success"
        ),
        None => false,
    }
}

fn code_of(payload: &serde_json::Value) -> String {
    match payload.get("msg") {
        Some(value) => {
            if let Some(number) = value.as_i64() {
                return number.to_string();
            }
            if let Some(text) = value.as_str() {
                let trimmed = text.trim();
                if !trimmed.is_empty() && trimmed.chars().all(|ch| ch.is_ascii_digit()) {
                    return trimmed.to_string();
                }
            }
            match payload.get("ret_code") {
                Some(ret) => format!("ret_code={ret}"),
                None => String::new(),
            }
        }
        None => match payload.get("ret_code") {
            Some(ret) => format!("ret_code={ret}"),
            None => String::new(),
        },
    }
}

fn describe(payload: &serde_json::Value, success: bool) -> String {
    for key in ["msga", "msg"] {
        if let Some(text) = payload.get(key).and_then(|value| value.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() && !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
                return trimmed.to_string();
            }
        }
    }
    if success {
        return "认证服务器未返回说明".to_string();
    }
    let code = code_of(payload);
    if code.is_empty() {
        "认证服务器未返回说明".to_string()
    } else {
        format!("认证服务器返回失败码 {code}")
    }
}

/// 返回 `(success, message, code)`。
fn portal_reply(text: &str) -> Option<(bool, String, String)> {
    if text.trim().eq_ignore_ascii_case("success") {
        return Some((true, "认证成功".into(), String::new()));
    }
    let payload: serde_json::Value = serde_json::from_str(&jsonp_payload(text)?).ok()?;
    let result = payload.get("result")?;
    let success = result_is_success(result);
    if !success
        && !matches!(result.as_i64(), Some(0))
        && result.as_bool() != Some(false)
        && !matches!(
            result.as_str().map(str::trim),
            Some("0" | "false" | "failure")
        )
    {
        return None;
    }
    Some((success, describe(&payload, success), code_of(&payload)))
}

#[cfg(test)]
fn parse_portal_reply(text: &str) -> (bool, String, String) {
    portal_reply(text).unwrap_or_else(|| (false, "无法识别认证服务器响应".into(), String::new()))
}

fn sanitize(message: &str, secrets: &[&str]) -> String {
    let lower = message.to_ascii_lowercase();
    if ["/drcom/login?", "upass=", "ddddd="]
        .iter()
        .any(|field| lower.contains(field))
    {
        return "门户说明包含认证参数，已省略".into();
    }
    let mut safe = message.to_owned();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        safe = safe.replace(secret, "***");
        safe = safe.replace(&http::urlencode(secret), "***");
    }
    safe.chars()
        .map(|ch| if ch == '\r' || ch == '\n' { ' ' } else { ch })
        .take(200)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_reply_is_parsed() {
        let (success, message, _code) =
            parse_portal_reply(r#"dr1004({"result":1,"msg":"Portal协议认证成功"});"#);
        assert!(success);
        assert!(message.contains("成功"));
    }

    #[test]
    fn chinese_rejection_keeps_the_text() {
        let (success, message, _code) =
            parse_portal_reply(r#"dr1004({"result":0,"msg":"账号或密码错误"});"#);
        assert!(!success);
        assert!(message.contains("密码错误"));
    }

    #[test]
    fn numeric_code_prefers_msga_text() {
        let (success, message, code) =
            parse_portal_reply(r#"dr1004({"result":0,"wopt":0,"msg":1,"msga":"userid error1"});"#);
        assert!(!success);
        assert!(message.contains("userid error1"));
        assert_eq!(code, "1");
    }

    #[test]
    fn numeric_code_without_text_is_readable() {
        let (success, message, code) = parse_portal_reply(r#"dr1004({"result":0,"msg":1});"#);
        assert!(!success);
        assert!(message.contains("失败码 1"));
        assert_eq!(code, "1");
    }

    #[test]
    fn ret_code_is_used_when_msg_is_absent() {
        let (_success, _message, code) =
            parse_portal_reply(r#"dr1004({"result":0,"ret_code":7});"#);
        assert_eq!(code, "ret_code=7");
    }

    #[test]
    fn garbage_is_not_success() {
        let (success, message, _code) = parse_portal_reply("gateway unavailable");
        assert!(!success);
        assert!(message.contains("无法识别"));
    }

    #[test]
    fn sanitize_removes_secrets_and_newlines() {
        let cleaned = sanitize("账号 demo-student\n密码 abc", &["demo-student", "abc"]);
        assert!(!cleaned.contains("demo-student"));
        assert!(!cleaned.contains('\n'));
        assert!(cleaned.contains("***"));
    }

    #[test]
    fn authenticate_sends_clean_drcom_params_and_parses_success() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = r#"dr1004({"result":1,"msg":"Portal协议认证成功"});"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).unwrap();
            req
        });

        let auth = CampusAuthenticator::with_url(format!("http://127.0.0.1:{port}/drcom/login"));
        let res = auth.authenticate("demo-student", "example@pass", "2", "192.0.2.42");
        let req = handle.join().unwrap();

        assert!(res.success);
        assert!(req.contains("DDDDD=demo-student&upass=example%40pass&"));
        assert!(req.contains("&R3=2&"));
        assert!(req.contains("&v4ip=192.0.2.42&"));
        assert!(!req.contains("%00"));
    }

    #[test]
    fn only_recognized_replies_can_accept_or_reject_credentials() {
        for body in [
            "Login unsuccessful",
            "<html>not a success</html>",
            r#"{"result":42}"#,
        ] {
            assert!(portal_reply(body).is_none());
        }
        assert!(portal_reply(" SUCCESS ").unwrap().0);
        assert!(!portal_reply(r#"{"result":0,"msg":7}"#).unwrap().0);
    }
}
