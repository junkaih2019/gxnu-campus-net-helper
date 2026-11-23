//! 门户认证。
//!
//! Dr.COM 的失败响应里 `msg` 是数字码、`msga` 才是人话，两者都保留。

use std::sync::Arc;

use crate::campus_status::jsonp_payload;
use crate::http;
use crate::logging;
use crate::models::DisconnectReason;
use crate::portal_gateway::PortalGatewayResolver;

pub const PORTAL_URL: &str = "https://yc.gxnu.edu.cn/drcom/login";
pub const LOGOUT_URL: &str = "https://yc.gxnu.edu.cn/drcom/logout";

#[derive(Debug, Clone)]
pub struct AuthenticationResult {
    pub success: bool,
    pub reason: DisconnectReason,
    pub message: String,
    /// 门户原始返回码（`msg` 数字码或 `ret_code`），只用于日志与排查。
    pub code: String,
}

#[derive(Debug, Clone)]
pub struct LogoutResult {
    pub success: bool,
    pub message: String,
}

enum AuthEndpoints {
    Fixed {
        portal_url: String,
        logout_url: String,
    },
    Dynamic(Arc<PortalGatewayResolver>),
}

pub struct CampusAuthenticator {
    agent: ureq::Agent,
    endpoints: AuthEndpoints,
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
            endpoints: AuthEndpoints::Fixed {
                portal_url: PORTAL_URL.to_string(),
                logout_url: LOGOUT_URL.to_string(),
            },
        }
    }

    pub fn with_url(url: impl Into<String>) -> Self {
        let portal_url = url.into();
        let logout_url = if let Some(prefix) = portal_url.strip_suffix("/login") {
            format!("{prefix}/logout")
        } else {
            LOGOUT_URL.to_string()
        };
        Self {
            agent: http::agent(10),
            endpoints: AuthEndpoints::Fixed {
                portal_url,
                logout_url,
            },
        }
    }

    pub fn with_resolver(resolver: Arc<PortalGatewayResolver>) -> Self {
        Self {
            agent: http::agent(10),
            endpoints: AuthEndpoints::Dynamic(resolver),
        }
    }

    fn current_urls(&self) -> (String, String, Option<(Arc<PortalGatewayResolver>, String)>) {
        match &self.endpoints {
            AuthEndpoints::Fixed {
                portal_url,
                logout_url,
            } => (portal_url.clone(), logout_url.clone(), None),
            AuthEndpoints::Dynamic(resolver) => {
                let ep = resolver.resolve_endpoints();
                (
                    ep.login_url,
                    ep.logout_url,
                    Some((Arc::clone(resolver), ep.origin)),
                )
            }
        }
    }

    /// 主动注销当前 Dr.COM 校园网会话（切换账号前调用，失败或本已离线均不阻塞后续登录）。
    pub fn logout(&self) -> LogoutResult {
        let (_, logout_url, dyn_ctx) = self.current_urls();
        let url = format!("{logout_url}?callback=dr1005&jsVersion=4.2.2&v=4532&lang=zh");
        match http::get_portal(&self.agent, &url) {
            Ok(response) => {
                if !(200..300).contains(&response.status) {
                    return LogoutResult {
                        success: false,
                        message: format!("注销接口返回 HTTP {}", response.status),
                    };
                }
                match portal_reply(&response.body) {
                    Some((success, message, _)) => LogoutResult {
                        success,
                        message: sanitize(&message, &[]),
                    },
                    None => LogoutResult {
                        success: false,
                        message: "门户注销响应格式未识别（可能当前已离线）".to_string(),
                    },
                }
            }
            Err(err) => {
                if let Some((resolver, origin)) = dyn_ctx {
                    if !resolver.is_custom_mode() {
                        resolver.invalidate_if_origin(&origin);
                    }
                }
                LogoutResult {
                    success: false,
                    message: format!("请求注销接口失败：{err}"),
                }
            }
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
        let carrier = if matches!(carrier, "0" | "1" | "2" | "3" | "4") {
            carrier
        } else {
            "2"
        };
        let (portal_url, _, dyn_ctx) = self.current_urls();
        let build_login_url = |base: &str| {
            format!(
                "{base}?callback=dr1004&DDDDD={}&upass={}&0MKKey=123456&R1=0&R2=&R3={carrier}&R6=0&para=00\
                 &v4ip={}&v6ip=&terminal_type=1&lang=zh&jsVersion=4.2.2&v=4532",
                http::urlencode(student_id),
                http::urlencode(password),
                http::urlencode(local_ip),
            )
        };
        let url = build_login_url(&portal_url);

        // Commit this request only after preparing it and immediately before network I/O.
        if !begin() {
            return None;
        }
        let mut response_res = http::get_portal(&self.agent, &url);

        // 若多校区自动模式下旧缓存网关连接失败，立即失效缓存并尝试重探一次新校区网关
        if response_res.is_err() {
            if let Some((resolver, failed_origin)) = &dyn_ctx {
                if !resolver.is_custom_mode() {
                    resolver.invalidate_if_origin(failed_origin);
                    if let Ok((fresh, _, _)) = resolver.probe_and_cache() {
                        if fresh.origin != *failed_origin {
                            let retry_url = build_login_url(&fresh.login_url);
                            response_res = http::get_portal(&self.agent, &retry_url);
                        }
                    }
                }
            }
        }

        Some(match response_res {
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
                logging::warn(&format!("[PORTAL_ERROR] 认证请求失败：{err}"));
                AuthenticationResult {
                    success: false,
                    reason: DisconnectReason::PortalUnavailable,
                    message: format!("认证服务器请求失败：{err}"),
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
                return translate_drcom_message(trimmed);
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
        format!("认证未通过（网关返回码 {code}），请核对账号、密码与运营商")
    }
}

/// 将师大 Dr.COM 认证门户（`yc.gxnu.edu.cn/a78.js` `RadiusErrorAry`）返回的内部英文错误码
/// 翻译为通俗易懂的中文提示，方便用户一眼定位是密码错、运营商选错还是并发占线。
pub fn translate_drcom_message(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lower = trimmed.to_ascii_lowercase();

    // 1. 精确或前缀匹配师大 Dr.COM 核心英文码表
    if lower == "userid error1" {
        return "账号不存在，请核对学号或所选运营商是否正确".to_string();
    }
    if lower == "userid error2" || lower == "userid error3" {
        return "密码错误，请在设置中重新核对并保存密码".to_string();
    }
    if matches!(
        lower.as_str(),
        "auth error99" | "auth error198" | "auth error199"
    ) {
        return "学号或密码错误，请在设置中重新核对".to_string();
    }
    if lower == "auth error4" || lower.contains("errcode=04") {
        return "上网时长或校园网套餐流量已用尽".to_string();
    }
    if lower == "auth error5" || lower.contains("errcode=05") {
        return "校园网账号已停机（可能欠费或报停）".to_string();
    }
    if lower == "auth error9" || lower.contains("errcode=09") {
        return "套餐时长或流量已超支".to_string();
    }
    if lower.contains("errcode=11") {
        return "当前账号不允许登录校园网".to_string();
    }
    if lower.contains("errcode=80") {
        return "接入认证服务器不存在".to_string();
    }
    if lower.contains("errcode=81") || lower.contains("ldap") {
        return "统一身份认证校验失败，请核对密码".to_string();
    }
    if lower.contains("errcode=85")
        || lower == "set_onlinet error"
        || lower == "in use"
        || lower == "wuxian olno"
    {
        return "在线设备数量已达上限（该账号正在其他终端登录）".to_string();
    }
    if lower.contains("errcode=86") || lower == "err(11)" {
        return "设备 IP 或 MAC 绑定校验失败".to_string();
    }
    if lower.contains("errcode=88") {
        return "校园网 IP 地址冲突，请尝试重连 Wi-Fi 或插拔网线".to_string();
    }
    if lower.contains("errcode=94") {
        return "学校认证服务器繁忙，稍后将自动重试".to_string();
    }
    if lower == "auth error80" || lower == "port err" {
        return "当前时段不允许上网".to_string();
    }
    if lower == "auth error258"
        || matches!(
            lower.as_str(),
            "err(2)" | "err(3)" | "err(7)" | "err(10)" | "err(17)"
        )
        || lower.contains("err(6)")
        || lower.contains("vlanid")
    {
        return "当前账号不允许在此区域或网段登录".to_string();
    }
    if lower == "can not use static ip" {
        return "不允许使用固定静态 IP，请将网卡改为自动获取（DHCP）".to_string();
    }
    if lower == "no errcode" {
        return "校园网网关认证未通过，稍后将自动重试".to_string();
    }

    // 2. 运营商 PPPoE 代拨返回码（Oppp error 系列）
    if lower == "oppp error: 1" || lower == "oppp error: 18" {
        return "运营商宽带账号或密码错误，请核对宽带密码或运营商选项".to_string();
    }
    if lower == "oppp error: 5"
        || lower == "oppp error: 21"
        || lower.contains("oppp error: userid inuse")
        || lower.contains("oppp error: too many connections")
    {
        return "运营商宽带账号已在其他设备在线（超出并发上限）".to_string();
    }
    if lower == "oppp error: 26" || lower == "oppp error: 29" {
        return "运营商宽带账号已绑定其他端口或位置".to_string();
    }
    if lower.contains("oppp error: can't find user") || lower == "bind userid error" {
        return "未绑定该运营商宽带，请检查运营商是否选错（电信/联通/移动/校园网）".to_string();
    }
    if lower.contains("oppp error: timeout") {
        return "运营商宽带状态异常（可能欠费停机或运营商响应超时）".to_string();
    }
    if lower.contains("oppp error: user dial-in so soon") {
        return "运营商宽带刚下线，稍等几秒后自动重连".to_string();
    }
    if lower == "auth error" {
        return "身份验证未通过，请核对学号、密码与运营商".to_string();
    }

    // 已经是中文或其他说明时保持原样（顺手清洗 HTML 换行标签）
    trimmed
        .replace("<br/>", "；")
        .replace("<br>", "；")
        .trim()
        .to_string()
}

/// 判断门户拒绝说明是否属于明确的「账号/密码/运营商配置错误」（用于保存账号前精准拦截，
/// 避免把并发已满、刚下线冷却、流量用尽或服务器繁忙误拦为账密错误）。
pub fn is_definite_credential_error(message: &str) -> bool {
    let m = message.trim();
    if m.is_empty() {
        return false;
    }
    [
        "密码错误",
        "账号不存在",
        "学号或密码错误",
        "用户名或密码错误",
        "账号或密码错误",
        "宽带账号或密码错误",
        "未绑定该运营商宽带",
        "统一身份认证校验失败",
        "身份验证未通过",
    ]
    .iter()
    .any(|kw| m.contains(kw))
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
    fn login_and_logout_report_oversized_jsonp() {
        use std::io::Write;
        for logout in [false, true] {
            let (base, server) = http::mock::serve(|stream| {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 32769\r\n\r\n")
                    .unwrap();
            });
            let auth = CampusAuthenticator::with_url(format!("{base}/login"));
            let (success, message) = if logout {
                let reply = auth.logout();
                (reply.success, reply.message)
            } else {
                let reply = auth.authenticate("fixture", "fixture", "0", "192.0.2.10");
                (reply.success, reply.message)
            };
            assert!(!success);
            assert!(message.contains("32768 字节上限"), "{message}");
            server.join().unwrap();
        }
    }

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
    fn numeric_code_prefers_msga_text_and_translates_to_plain_chinese() {
        let (success, message, code) =
            parse_portal_reply(r#"dr1004({"result":0,"wopt":0,"msg":1,"msga":"userid error1"});"#);
        assert!(!success);
        assert!(message.contains("账号不存在"));
        assert!(message.contains("运营商"));
        assert_eq!(code, "1");

        let (success2, message2, code2) =
            parse_portal_reply(r#"dr1004({"result":0,"wopt":0,"msg":1,"msga":"userid error2"});"#);
        assert!(!success2);
        assert!(message2.contains("密码错误"));
        assert_eq!(code2, "1");
    }

    #[test]
    fn translates_common_drcom_and_carrier_errors_to_plain_chinese() {
        assert!(translate_drcom_message("userid error3").contains("密码错误"));
        assert!(translate_drcom_message("Authentication Fail ErrCode=85").contains("上限"));
        assert!(translate_drcom_message("Oppp error: 1").contains("运营商宽带账号或密码错误"));
        assert!(translate_drcom_message("bind userid error").contains("未绑定该运营商宽带"));
        assert!(translate_drcom_message("Oppp error: TOO MANY CONNECTIONS").contains("并发上限"));
        assert!(translate_drcom_message("can not use static ip").contains("DHCP"));

        assert!(is_definite_credential_error(&translate_drcom_message(
            "userid error1"
        )));
        assert!(is_definite_credential_error(&translate_drcom_message(
            "userid error2"
        )));
        assert!(is_definite_credential_error(&translate_drcom_message(
            "Oppp error: 1"
        )));
        assert!(is_definite_credential_error(&translate_drcom_message(
            "bind userid error"
        )));
        assert!(!is_definite_credential_error(&translate_drcom_message(
            "Authentication Fail ErrCode=85"
        )));
        assert!(!is_definite_credential_error(&translate_drcom_message(
            "Oppp error: User dial-in so soon"
        )));
        assert!(!is_definite_credential_error(&translate_drcom_message(
            "Authentication Fail ErrCode=04"
        )));
    }

    #[test]
    fn numeric_code_without_text_is_readable() {
        let (success, message, code) = parse_portal_reply(r#"dr1004({"result":0,"msg":1});"#);
        assert!(!success);
        assert!(message.contains("返回码 1"));
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
    fn authenticate_supports_broadnet_carrier_r3_4() {
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
        let res = auth.authenticate("demo-student", "pass123", "4", "172.16.24.195");
        let req = handle.join().unwrap();

        assert!(res.success);
        assert!(req.contains("&R3=4&"));
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

    #[test]
    fn logout_sends_drcom_logout_and_parses_reply() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = r#"dr1005({"result":1,"msg":"注销成功"});"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).unwrap();
            req
        });

        let auth = CampusAuthenticator::with_url(format!("http://127.0.0.1:{port}/drcom/login"));
        let res = auth.logout();
        let req = handle.join().unwrap();

        assert!(res.success);
        assert!(res.message.contains("注销成功"));
        assert!(req.contains("GET /drcom/logout?callback=dr1005"));
    }
}
