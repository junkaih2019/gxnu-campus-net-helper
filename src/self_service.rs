//! GXNU 校园网自助服务（`http://172.16.4.2/Self/`）轻量只读客户端与低频缓存调度器。
//!
//! 设计原则：
//! 1. **零重型依赖**：用严格的轻量 HTML 属性解析替代 `scraper`，保持单文件轻量与快速编译。
//! 2. **只读白名单**：仅反序列化流量、并发上限、在线终端与近期上网记录，绝不落盘密码、`sessionId`、MAC 或 Cookie。
//! 3. **低频防风控**：按学号共享 5 分钟缓存、15 秒手动节流、触发验证码自动进入 30 分钟静默冷却，绝不跟随 10s 巡检轮询。
//! 4. **离线可达**：经实测 `172.16.4.2/Self/` 在未认证校园网状态下仍放行，可在登录被拒时离线定位「流量用尽」或「占线设备」。

use std::net::IpAddr;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDateTime, TimeZone};
use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use ureq::{Agent, Response};
use url::Url;

use crate::credentials::StoredCredential;
use crate::http;
use crate::logging;
use crate::models::{
    OnlineDevice, RecentSession, SelfServiceData, SelfServiceState, SelfServiceStatus,
    SelfServiceTraffic,
};

const ORIGIN: &str = "http://172.16.4.2";
const ENTRY: &str = "http://172.16.4.2/Self/";
const LOGIN: &str = "http://172.16.4.2/Self/login/?302=LI";
const VERIFY: &str = "http://172.16.4.2/Self/login/verify";
const DASHBOARD: &str = "http://172.16.4.2/Self/dashboard";
const SESSION_IMAGE: &str = "http://172.16.4.2/Self/login/randomCode";
const ONLINE_LOG_PAGE: &str = "http://172.16.4.2/Self/bill/userOnlineLog";
const ONLINE_LOG_API: &str = "http://172.16.4.2/Self/bill/getUserOnlineLog";

pub const CACHE_TTL_SECS: i64 = 5 * 60;
pub const MANUAL_THROTTLE_SECS: i64 = 15;
pub const CAPTCHA_COOLDOWN_SECS: i64 = 30 * 60;
const MANUAL_CAPTCHA_MIN_WAIT_SECS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalError {
    CaptchaRequired,
    UnsupportedPassword,
    AuthFailed(String),
    Unreachable(String),
    ParseError(String),
}

impl PortalError {
    pub fn status(&self) -> SelfServiceStatus {
        match self {
            Self::CaptchaRequired => SelfServiceStatus::CaptchaCooldown,
            Self::UnsupportedPassword => SelfServiceStatus::UnsupportedPassword,
            Self::AuthFailed(_) => SelfServiceStatus::AuthFailed,
            Self::Unreachable(_) | Self::ParseError(_) => SelfServiceStatus::Unreachable,
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Self::CaptchaRequired => "门户要求验证码，已暂停自动同步".to_string(),
            Self::UnsupportedPassword => "密码含非单字节字符，暂不支持自助服务同步".to_string(),
            Self::AuthFailed(msg) | Self::Unreachable(msg) | Self::ParseError(msg) => msg.clone(),
        }
    }
}

impl std::fmt::Display for PortalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.summary())
    }
}

/// 使用独立直连内存会话执行一次只读自助服务查询（完成后自动释放 Cookie 与连接）。
pub fn fetch_snapshot(credential: &StoredCredential) -> Result<SelfServiceData, PortalError> {
    let agent = http::no_redirect_agent(8);
    let dashboard_html = login(&agent, credential)?;
    let (mut traffic_info, max_online) = parse_dashboard_account(&dashboard_html)?;
    let online_devices = fetch_online_devices(&agent)?;
    let fetched_at_unix = Local::now().timestamp();

    // 优先查询本月上网明细接口（包含运营商实际上下行流量）；失败时平滑回退到首页近期记录。
    let recent_sessions = match fetch_month_online_log(&agent) {
        Ok((sessions, summary_down, summary_up, summary_count)) => {
            if let Some(d) = summary_down {
                if d > traffic_info.month_down_mb {
                    traffic_info.month_down_mb = d;
                }
            }
            if let Some(u) = summary_up {
                if u > traffic_info.month_up_mb {
                    traffic_info.month_up_mb = u;
                }
            }
            traffic_info.month_session_count = summary_count;
            sessions
        }
        Err(_) => fetch_dashboard_history_fallback(&agent).unwrap_or_default(),
    };

    Ok(SelfServiceData {
        account_key: account_fingerprint(&credential.username),
        traffic: traffic_info,
        max_online_devices: max_online,
        online_devices,
        recent_sessions,
        fetched_at_unix,
    })
}

fn login(agent: &Agent, credential: &StoredCredential) -> Result<String, PortalError> {
    let response = checked_response("打开自助服务", agent.get(ENTRY).call())?;
    require_redirect(&response, DASHBOARD, "打开自助服务")?;
    let response = checked_response("打开账户首页", agent.get(DASHBOARD).call())?;
    require_redirect(&response, LOGIN, "打开账户首页")?;

    let page = get_page(agent, LOGIN, "打开登录表单")?;
    let checkcode = parse_login_page(&page)?;

    // 登录页加载时会请求该资源以绑定服务端会话，验证码字段保持为空。
    let image = checked_response(
        "加载登录表单资源",
        agent.get(SESSION_IMAGE).set("Referer", LOGIN).call(),
    )?;
    if image.status() != 200 {
        return Err(PortalError::Unreachable(format!(
            "登录表单资源返回 HTTP {}",
            image.status()
        )));
    }
    drop(image);

    // 门户旧版 MD5 脚本将 UTF-16 码元按低 8 位装入消息块。
    let password_bytes = credential
        .password
        .encode_utf16()
        .map(u8::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PortalError::UnsupportedPassword)?;
    let digest = format!("{:x}", md5::compute(password_bytes));

    let response = checked_response(
        "提交登录",
        agent
            .post(VERIFY)
            .set("Origin", ORIGIN)
            .set("Referer", LOGIN)
            .send_form(&[
                ("foo", ""),
                ("bar", ""),
                ("checkcode", &checkcode),
                ("account", &credential.username),
                ("password", &digest),
                ("code", ""),
            ]),
    )?;
    if matches!(response.status(), 200) {
        return Err(PortalError::AuthFailed(
            "自助服务登录未通过（可能密码错误或触发验证码）".into(),
        ));
    }
    require_redirect(&response, DASHBOARD, "验证登录结果")?;
    get_page(agent, DASHBOARD, "读取账户首页")
}

/// 轻量严格解析登录表单：校验 `action`、确认 `#randomDiv` 隐藏、提取隐藏 `checkcode` 与 `var md5 = true;`。
fn parse_login_page(page: &str) -> Result<String, PortalError> {
    let mut form_ok = false;
    let mut random_div_hidden = false;
    let mut checkcode: Option<String> = None;

    for tag in iter_html_tags(page) {
        let name = tag.name;
        if name.eq_ignore_ascii_case("form") {
            let method = tag.attr("method").unwrap_or_default();
            let action = tag.attr("action").unwrap_or_default();
            if method.eq_ignore_ascii_case("post")
                && !action.is_empty()
                && matches_destination(LOGIN, action, VERIFY)
            {
                form_ok = true;
            }
        } else if tag
            .attr("id")
            .is_some_and(|id| id.eq_ignore_ascii_case("randomDiv"))
        {
            if tag
                .attr("class")
                .is_some_and(|cls| cls.split_ascii_whitespace().any(|tok| tok == "hide"))
            {
                random_div_hidden = true;
            }
        } else if name.eq_ignore_ascii_case("input") {
            let typ = tag.attr("type").unwrap_or_default();
            let input_name = tag.attr("name").unwrap_or_default();
            if typ.eq_ignore_ascii_case("hidden") && input_name == "checkcode" {
                if let Some(val) = tag.attr("value").filter(|v| !v.is_empty()) {
                    checkcode = Some(val.to_string());
                }
            }
        }
    }

    if !form_ok {
        return Err(PortalError::ParseError(
            "自助服务登录表单地址与预期不符".into(),
        ));
    }
    if !random_div_hidden {
        return Err(PortalError::CaptchaRequired);
    }
    let checkcode =
        checkcode.ok_or_else(|| PortalError::ParseError("登录表单缺少 checkcode".into()))?;
    if !page.contains("var md5 = true;") {
        return Err(PortalError::ParseError("登录页密码处理方式已变化".into()));
    }
    Ok(checkcode)
}

#[derive(Deserialize)]
struct RawDashboardUserGroup {
    #[serde(rename = "ipMaxCount", default)]
    ip_max_count: Option<u32>,
    #[serde(rename = "userGroupDescription", default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct RawDashboardAccount {
    #[serde(rename = "useFlow")]
    use_flow: f64,
    #[serde(rename = "leftFlow")]
    left_flow: f64,
    #[serde(rename = "internetDownFlow", default)]
    internet_down_flow: f64,
    #[serde(rename = "internetUpFlow", default)]
    internet_up_flow: f64,
    #[serde(rename = "userGroup", default)]
    user_group: Option<RawDashboardUserGroup>,
}

fn parse_dashboard_account(page: &str) -> Result<(SelfServiceTraffic, Option<u32>), PortalError> {
    for (index, delimiter) in page.match_indices("})(") {
        let json = &page[index + delimiter.len()..];
        let mut values =
            serde_json::Deserializer::from_str(json).into_iter::<RawDashboardAccount>();
        if let Some(Ok(acc)) = values.next() {
            let (max_online, plan_desc) = match acc.user_group {
                Some(group) => (
                    group.ip_max_count.filter(|&n| n > 0),
                    group.description.unwrap_or_default().trim().to_string(),
                ),
                None => (None, String::new()),
            };
            let traffic_info = SelfServiceTraffic {
                campus_used_mb: sanitize_flow(acc.use_flow),
                campus_remaining_mb: sanitize_flow(acc.left_flow),
                month_down_mb: sanitize_flow(acc.internet_down_flow),
                month_up_mb: sanitize_flow(acc.internet_up_flow),
                plan_description: plan_desc,
                month_session_count: None,
            };
            return Ok((traffic_info, max_online));
        }
    }
    Err(PortalError::ParseError(
        "门户首页缺少已用流量或可用流量字段".into(),
    ))
}

#[derive(Deserialize)]
struct RawOnlineSession {
    ip: IpAddr,
    #[serde(rename = "loginTime")]
    login_time: String,
    #[serde(rename = "terminalType", default)]
    terminal_type: Option<String>,
    #[serde(rename = "useTime", default, deserialize_with = "de_opt_u64")]
    use_time: Option<u64>,
}

fn fetch_online_devices(agent: &Agent) -> Result<Vec<OnlineDevice>, PortalError> {
    let raw: Vec<RawOnlineSession> = dashboard_json(agent, "getOnlineList", "读取在线设备")?;
    Ok(raw
        .into_iter()
        .map(|item| {
            let login_unix = parse_local_datetime(&item.login_time);
            OnlineDevice {
                ip: item.ip.to_string(),
                login_time: item.login_time,
                login_unix,
                terminal_type: normalize_terminal_type(item.terminal_type.as_deref()),
                use_time_secs: item.use_time,
            }
        })
        .collect())
}

#[derive(Deserialize)]
struct RawOnlineLogSummary {
    #[serde(rename = "INTERNETDOWNFLOW", default)]
    internet_down_flow: Option<f64>,
    #[serde(rename = "INTERNETUPFLOW", default)]
    internet_up_flow: Option<f64>,
    #[serde(rename = "COU", default, deserialize_with = "de_opt_u64")]
    count: Option<u64>,
}

#[derive(Deserialize)]
struct RawOnlineLogRow {
    #[serde(rename = "loginTime")]
    login_time: i64,
    #[serde(rename = "logoutTime", default)]
    logout_time: Option<i64>,
    #[serde(rename = "userIp")]
    user_ip: IpAddr,
    #[serde(rename = "time", default)]
    time_minutes: f64,
    #[serde(rename = "flow", default)]
    campus_flow: f64,
    #[serde(rename = "internetDownFlow", default)]
    internet_down_flow: f64,
    #[serde(rename = "internetUpFlow", default)]
    internet_up_flow: f64,
}

#[derive(Deserialize)]
struct RawOnlineLogResponse {
    #[serde(default)]
    rows: Vec<RawOnlineLogRow>,
    #[serde(default)]
    summary: Option<RawOnlineLogSummary>,
    #[serde(default)]
    total: Option<u64>,
}

type OnlineLogBundle = (Vec<RecentSession>, Option<f64>, Option<f64>, Option<u64>);

fn fetch_month_online_log(agent: &Agent) -> Result<OnlineLogBundle, PortalError> {
    let now = Local::now();
    let month_start = now.format("%Y-%m-01").to_string();
    let today = now.format("%Y-%m-%d").to_string();
    let url = format!(
        "{ONLINE_LOG_API}?startTime={month_start}&endTime={today}&order=asc&limit=6&offset=0"
    );
    let response = checked_response(
        "读取上网记录明细",
        agent
            .get(&url)
            .set("Referer", ONLINE_LOG_PAGE)
            .set("X-Requested-With", "XMLHttpRequest")
            .call(),
    )?;
    if response.status() != 200 {
        return Err(PortalError::Unreachable(format!(
            "读取上网记录明细返回 HTTP {}",
            response.status()
        )));
    }
    let body = response
        .into_string()
        .map_err(|_| PortalError::ParseError("无法读取上网记录明细响应".into()))?;
    parse_month_online_log(&body)
}

fn parse_month_online_log(body: &str) -> Result<OnlineLogBundle, PortalError> {
    let parsed: RawOnlineLogResponse = serde_json::from_str(body)
        .map_err(|_| PortalError::ParseError("上网记录明细 JSON 格式无法识别".into()))?;
    let mut sessions: Vec<RecentSession> = parsed
        .rows
        .into_iter()
        .map(|row| {
            let duration_secs = row
                .logout_time
                .and_then(|out| out.checked_sub(row.login_time))
                .filter(|&ms| ms > 0)
                .map(|ms| (ms / 1000) as u64)
                .unwrap_or_else(|| (row.time_minutes.max(0.0) * 60.0).round() as u64);
            RecentSession {
                login_unix_ms: row.login_time,
                logout_unix_ms: row.logout_time,
                ip: row.user_ip.to_string(),
                duration_secs,
                campus_flow_mb: sanitize_flow(row.campus_flow),
                actual_down_mb: sanitize_flow(row.internet_down_flow),
                actual_up_mb: sanitize_flow(row.internet_up_flow),
            }
        })
        .collect();
    sessions.sort_by_key(|a| std::cmp::Reverse(a.login_unix_ms));
    sessions.truncate(6);

    let (summary_down, summary_up, summary_count) = match parsed.summary {
        Some(s) => (
            s.internet_down_flow.map(sanitize_flow),
            s.internet_up_flow.map(sanitize_flow),
            s.count.or(parsed.total),
        ),
        None => (None, None, parsed.total),
    };
    Ok((sessions, summary_down, summary_up, summary_count))
}

fn fetch_dashboard_history_fallback(agent: &Agent) -> Result<Vec<RecentSession>, PortalError> {
    let rows: Vec<Vec<Value>> = dashboard_json(agent, "getLoginHistory", "读取首页近期记录")?;
    let mut sessions: Vec<RecentSession> = rows
        .iter()
        .filter_map(|row| {
            let [login, logout, ip, _mac, duration, traffic, ..] = row.as_slice() else {
                return None;
            };
            let login_ms = login.as_i64()?;
            let logout_ms = if logout.is_null() {
                None
            } else {
                Some(logout.as_i64()?)
            };
            let ip_addr: IpAddr = ip.as_str()?.parse().ok()?;
            let duration_minutes = duration.as_f64().unwrap_or(0.0);
            let campus_flow = sanitize_flow(traffic.as_f64().unwrap_or(0.0));
            let duration_secs = logout_ms
                .and_then(|out| out.checked_sub(login_ms))
                .filter(|&ms| ms > 0)
                .map(|ms| (ms / 1000) as u64)
                .unwrap_or_else(|| (duration_minutes.max(0.0) * 60.0).round() as u64);
            Some(RecentSession {
                login_unix_ms: login_ms,
                logout_unix_ms: logout_ms,
                ip: ip_addr.to_string(),
                duration_secs,
                campus_flow_mb: campus_flow,
                actual_down_mb: 0.0,
                actual_up_mb: 0.0,
            })
        })
        .collect();
    sessions.sort_by_key(|a| std::cmp::Reverse(a.login_unix_ms));
    sessions.truncate(6);
    Ok(sessions)
}

fn dashboard_json<T: DeserializeOwned>(
    agent: &Agent,
    endpoint: &str,
    stage: &str,
) -> Result<T, PortalError> {
    let body = get_page(agent, &format!("{DASHBOARD}/{endpoint}"), stage)?;
    serde_json::from_str(&body)
        .map_err(|_| PortalError::ParseError(format!("{stage}时收到无法识别的数据")))
}

fn checked_response(
    stage: &str,
    result: Result<Response, ureq::Error>,
) -> Result<Response, PortalError> {
    match result {
        Ok(response) => Ok(response),
        Err(ureq::Error::Status(status, _)) => Err(PortalError::Unreachable(format!(
            "{stage}返回 HTTP {status}"
        ))),
        Err(ureq::Error::Transport(error)) => Err(PortalError::Unreachable(format!(
            "{stage}连接失败（{:?}）",
            error.kind()
        ))),
    }
}

fn get_page(agent: &Agent, url: &str, stage: &str) -> Result<String, PortalError> {
    let response = checked_response(stage, agent.get(url).call())?;
    if response.status() != 200 {
        return Err(PortalError::Unreachable(format!(
            "{stage}返回 HTTP {}",
            response.status()
        )));
    }
    response
        .into_string()
        .map_err(|_| PortalError::ParseError(format!("{stage}无法读取响应正文")))
}

fn matches_destination(base: &str, destination: &str, expected: &str) -> bool {
    let actual = Url::parse(base).and_then(|url| url.join(destination));
    let expected = Url::parse(expected).expect("静态门户地址");
    actual.is_ok_and(|url| {
        url.origin() == expected.origin()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path().split(';').next() == Some(expected.path())
    })
}

fn require_redirect(response: &Response, expected: &str, stage: &str) -> Result<(), PortalError> {
    let destination = response.header("Location").unwrap_or_default();
    if !matches!(response.status(), 302 | 303)
        || !matches_destination(response.get_url(), destination, expected)
    {
        return Err(PortalError::AuthFailed(format!(
            "{stage}未跳转到预期页面（HTTP {}）",
            response.status()
        )));
    }
    Ok(())
}

fn account_fingerprint(student_id: &str) -> String {
    let trimmed = student_id.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // 64 位 FNV-1a 指纹：仅用于内存缓存隔离，绝不暴露明文学号
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in trimmed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn sanitize_flow(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

fn normalize_terminal_type(raw: Option<&str>) -> String {
    let trimmed = raw.unwrap_or("").trim().trim_start_matches('#');
    if trimmed.is_empty() {
        return String::new();
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper.contains("PC") || upper.contains("WIN") || upper.contains("MAC") {
        "PC".to_string()
    } else if upper.contains("PAD") || upper.contains("TABLET") {
        "平板".to_string()
    } else if upper.contains("PHONE")
        || upper.contains("MOBILE")
        || upper.contains("ANDROID")
        || upper.contains("IOS")
    {
        "手机".to_string()
    } else {
        trimmed.chars().take(6).collect()
    }
}

fn parse_local_datetime(text: &str) -> Option<i64> {
    let naive = NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M:%S").ok()?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.timestamp())
}

fn de_opt_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let val = Option::<Value>::deserialize(deserializer)?;
    match val {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64))),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                trimmed
                    .parse::<u64>()
                    .map(Some)
                    .or_else(|_| trimmed.parse::<f64>().map(|f| Some(f.max(0.0) as u64)))
                    .map_err(de::Error::custom)
            }
        }
        _ => Ok(None),
    }
}

/// 极简无依赖 HTML 起始标签迭代器（跳过 `<script>` 内容与 `<!--` 注释，支持单/双引号与无引号属性）。
struct HtmlTag<'a> {
    name: &'a str,
    attrs: &'a str,
}

impl<'a> HtmlTag<'a> {
    fn attr(&self, key: &str) -> Option<&'a str> {
        let mut s = self.attrs;
        loop {
            s = s.trim_start();
            if s.is_empty() || s.starts_with('/') {
                return None;
            }
            let name_end = s
                .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
                .unwrap_or(s.len());
            if name_end == 0 {
                s = &s[1..];
                continue;
            }
            let attr_name = &s[..name_end];
            s = s[name_end..].trim_start();
            let value = if let Some(after_eq) = s.strip_prefix('=') {
                let v = after_eq.trim_start();
                if let Some(rest) = v.strip_prefix('"') {
                    let end = rest.find('"').unwrap_or(rest.len());
                    let val = &rest[..end];
                    s = rest.get(end + 1..).unwrap_or("");
                    val
                } else if let Some(rest) = v.strip_prefix('\'') {
                    let end = rest.find('\'').unwrap_or(rest.len());
                    let val = &rest[..end];
                    s = rest.get(end + 1..).unwrap_or("");
                    val
                } else {
                    let end = v
                        .find(|c: char| c.is_ascii_whitespace() || c == '>')
                        .unwrap_or(v.len());
                    let val = &v[..end];
                    s = &v[end..];
                    val
                }
            } else {
                ""
            };
            if attr_name.eq_ignore_ascii_case(key) {
                return Some(value);
            }
        }
    }
}

fn iter_html_tags(html: &str) -> Vec<HtmlTag<'_>> {
    let mut tags = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let Some(rel_lt) = html[i..].find('<') else {
            break;
        };
        i += rel_lt;
        if html[i..].starts_with("<!--") {
            if let Some(end) = html[i + 4..].find("-->") {
                i += 4 + end + 3;
                continue;
            }
            break;
        }
        let start = i + 1;
        if start >= bytes.len() || matches!(bytes[start], b'/' | b'!') {
            i = start;
            continue;
        }
        // 查找闭合 `>`，注意跳过引号内的 `>`
        let mut j = start;
        let mut quote: Option<u8> = None;
        while j < bytes.len() {
            let b = bytes[j];
            if let Some(q) = quote {
                if b == q {
                    quote = None;
                }
            } else if b == b'"' || b == b'\'' {
                quote = Some(b);
            } else if b == b'>' {
                break;
            }
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        let inner = html[start..j].trim();
        let name_end = inner
            .find(|c: char| c.is_ascii_whitespace() || c == '/')
            .unwrap_or(inner.len());
        let name = &inner[..name_end];
        let attrs = inner.get(name_end..).unwrap_or("");
        if !name.is_empty() {
            tags.push(HtmlTag { name, attrs });
        }
        i = j + 1;
        // 跳过 <script>...</script> 正文，避免把 JS 字符串误当作 HTML 标签
        if name.eq_ignore_ascii_case("script") {
            let lower_rest = html[i..].to_ascii_lowercase();
            if let Some(end) = lower_rest.find("</script") {
                i += end + 8;
            } else {
                break;
            }
        }
    }
    tags
}

// ── 守护进程侧低频按需调度器 ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum FetchTrigger {
    /// 打开主窗口或切换账号时的常规同步（遵守 5 分钟缓存 TTL 与 30 分钟验证码冷却）。
    UiOpen,
    /// 用户主动点击刷新按钮（遵守 15 秒最小防抖；验证码冷却期满 60 秒后允许手动重试）。
    Manual,
    /// `AUTH_EXPIRED` 自动重连成功后的异步归因复核（绕过 5 分钟 TTL，不拖慢重连）。
    PostReconnect {
        dropped_at_unix: i64,
        local_campus_ip: String,
    },
    /// `LOGIN_REJECTED` 重连被拒后的离线精准诊断（利用 172.16.4.2 离线放行特性）。
    LoginRejected { local_campus_ip: String },
}

#[derive(Clone)]
struct ActiveTarget {
    account_id: String,
    student_id: String,
    account_key: String,
    carrier: String,
}

struct ManagerState {
    target: ActiveTarget,
    public: SelfServiceState,
    in_flight: bool,
    last_attempt_mono: Option<Instant>,
    captcha_until_unix: Option<i64>,
    last_captcha_at_unix: Option<i64>,
}

type StateBroadcast = Arc<dyn Fn(SelfServiceState) + Send + Sync>;
type CredLookup = Arc<dyn Fn(&str, &str) -> Option<StoredCredential> + Send + Sync>;

/// 守护进程侧自助服务管理器：按学号缓存、防风控节流、单后台工作线程。
pub struct SelfServiceManager {
    inner: Arc<Mutex<ManagerState>>,
    tx: mpsc::Sender<FetchTrigger>,
    broadcast: StateBroadcast,
}

impl SelfServiceManager {
    pub fn new(
        account_id: String,
        student_id: String,
        carrier: String,
        cred_lookup: CredLookup,
        broadcast: StateBroadcast,
    ) -> Self {
        let target = ActiveTarget {
            account_id,
            account_key: account_fingerprint(&student_id),
            student_id,
            carrier,
        };
        let inner = Arc::new(Mutex::new(ManagerState {
            target,
            public: SelfServiceState::default(),
            in_flight: false,
            last_attempt_mono: None,
            captcha_until_unix: None,
            last_captcha_at_unix: None,
        }));
        let (tx, rx) = mpsc::channel::<FetchTrigger>();

        let worker_inner = Arc::clone(&inner);
        let worker_broadcast = Arc::clone(&broadcast);
        let _ = std::thread::Builder::new()
            .name("self-service-worker".to_string())
            .spawn(move || {
                while let Ok(trigger) = rx.recv() {
                    run_single_fetch(&worker_inner, &cred_lookup, &worker_broadcast, trigger);
                }
            });

        Self {
            inner,
            tx,
            broadcast,
        }
    }

    pub fn current_state(&self) -> SelfServiceState {
        self.inner
            .lock()
            .map(|g| g.public.clone())
            .unwrap_or_default()
    }

    /// 同步最新活跃账号配置。
    /// 同一个学号下切换运营商（如宿舍联通 ↔ 教室校园网）直接复用已有缓存，不重复登录门户！
    pub fn sync_account(&self, account_id: &str, student_id: &str, carrier: &str) {
        let new_key = account_fingerprint(student_id);
        let mut guard = self.inner.lock().unwrap();
        let student_changed = guard.target.account_key != new_key;
        guard.target = ActiveTarget {
            account_id: account_id.to_string(),
            student_id: student_id.to_string(),
            account_key: new_key,
            carrier: carrier.to_string(),
        };
        if student_changed {
            guard.public = SelfServiceState::default();
            guard.captcha_until_unix = None;
            guard.last_captcha_at_unix = None;
            let state = guard.public.clone();
            drop(guard);
            (self.broadcast)(state);
        }
    }

    /// 按触发策略决定是否发起异步同步；返回给调用方的提示文案（手动刷新被节流时返回 Ok，不报错）。
    pub fn trigger(&self, trigger: FetchTrigger) {
        let now_unix = Local::now().timestamp();
        let now_mono = Instant::now();
        let mut guard = self.inner.lock().unwrap();

        if guard.in_flight {
            return;
        }

        if guard.target.student_id.trim().is_empty() {
            guard.public.status = SelfServiceStatus::NoCredentials;
            guard.public.message = "请先保存校园网学号与密码".to_string();
            let state = guard.public.clone();
            drop(guard);
            (self.broadcast)(state);
            return;
        }

        let is_manual = matches!(trigger, FetchTrigger::Manual);
        let bypass_cache = !matches!(trigger, FetchTrigger::UiOpen);

        // 1. 验证码静默冷却保护
        if let Some(until) = guard.captcha_until_unix {
            if now_unix < until {
                let allow_manual_retry = is_manual
                    && guard
                        .last_captcha_at_unix
                        .is_some_and(|at| now_unix - at >= MANUAL_CAPTCHA_MIN_WAIT_SECS);
                if !allow_manual_retry {
                    let rem_mins = ((until - now_unix + 59) / 60).max(1);
                    guard.public.status = SelfServiceStatus::CaptchaCooldown;
                    guard.public.cooldown_until_unix = Some(until);
                    guard.public.message = format!("门户需验证码，冷却中（约 {rem_mins} 分钟）");
                    let state = guard.public.clone();
                    drop(guard);
                    (self.broadcast)(state);
                    return;
                }
            } else {
                guard.captcha_until_unix = None;
            }
        }

        // 2. 15 秒最小请求防抖（任何触发都不在 15 秒内重复打门户）
        if let Some(last) = guard.last_attempt_mono {
            if last.elapsed() < Duration::from_secs(MANUAL_THROTTLE_SECS as u64) {
                return;
            }
        }

        // 3. 常规 UI 打开复用 5 分钟缓存（按学号共享）
        if !bypass_cache {
            if let Some(data) = &guard.public.data {
                if data.account_key == guard.target.account_key
                    && now_unix - data.fetched_at_unix < CACHE_TTL_SECS
                {
                    return;
                }
            }
        }

        guard.in_flight = true;
        guard.last_attempt_mono = Some(now_mono);
        guard.public.status = SelfServiceStatus::Refreshing;
        guard.public.message = "正在同步自助服务…".to_string();
        let state = guard.public.clone();
        drop(guard);

        (self.broadcast)(state);
        let _ = self.tx.send(trigger);
    }
}

fn run_single_fetch(
    inner: &Arc<Mutex<ManagerState>>,
    cred_lookup: &CredLookup,
    broadcast: &StateBroadcast,
    trigger: FetchTrigger,
) {
    let target = {
        let guard = inner.lock().unwrap();
        guard.target.clone()
    };

    let Some(cred) = cred_lookup(&target.account_id, &target.student_id)
        .filter(|c| c.matches_account(&target.student_id))
    else {
        let state = {
            let mut guard = inner.lock().unwrap();
            guard.in_flight = false;
            guard.public.status = SelfServiceStatus::NoCredentials;
            guard.public.message = "当前账号未保存密码".to_string();
            guard.public.clone()
        };
        broadcast(state);
        return;
    };

    let result = fetch_snapshot(&cred);
    let now_unix = Local::now().timestamp();

    let (state, maybe_log) = {
        let mut guard = inner.lock().unwrap();
        guard.in_flight = false;

        // 若拉取期间用户已切换到其他学号，丢弃过期结果避免串号
        if guard.target.account_key != target.account_key {
            return;
        }

        match result {
            Ok(data) => {
                guard.captcha_until_unix = None;
                guard.last_captcha_at_unix = None;
                let log_msg = build_trigger_diagnosis(&trigger, &target.carrier, &data);
                guard.public = SelfServiceState {
                    status: SelfServiceStatus::Ready,
                    message: String::new(),
                    cooldown_until_unix: None,
                    data: Some(data),
                };
                (guard.public.clone(), log_msg)
            }
            Err(err) => {
                let status = err.status();
                let mut msg = err.summary();
                if err == PortalError::CaptchaRequired {
                    let until = now_unix + CAPTCHA_COOLDOWN_SECS;
                    guard.captcha_until_unix = Some(until);
                    guard.last_captcha_at_unix = Some(now_unix);
                    guard.public.cooldown_until_unix = Some(until);
                    msg = "门户要求验证码，已静默冷却 30 分钟".to_string();
                }
                guard.public.status = status;
                guard.public.message = msg.clone();
                let log_msg = if matches!(trigger, FetchTrigger::Manual)
                    || err == PortalError::CaptchaRequired
                {
                    Some((true, format!("[SELF_SERVICE] {msg}")))
                } else {
                    None
                };
                (guard.public.clone(), log_msg)
            }
        }
    };

    if let Some((is_warn, line)) = maybe_log {
        if is_warn {
            logging::warn(&line);
        } else {
            logging::info(&line);
        }
    }
    broadcast(state);
}

/// 根据触发场景（重连后复核 / 离线被拒诊断）生成严谨、不误报的多端归因日志。
fn build_trigger_diagnosis(
    trigger: &FetchTrigger,
    carrier: &str,
    data: &SelfServiceData,
) -> Option<(bool, String)> {
    let count = data.online_devices.len();
    let cap_str = data
        .max_online_devices
        .map(|m| format!("{count}/{m}"))
        .unwrap_or_else(|| format!("{count}"));

    match trigger {
        FetchTrigger::UiOpen | FetchTrigger::Manual => None,
        FetchTrigger::PostReconnect {
            dropped_at_unix,
            local_campus_ip,
        } => {
            let others: Vec<&OnlineDevice> = data
                .online_devices
                .iter()
                .filter(|d| !local_campus_ip.is_empty() && d.ip != *local_campus_ip)
                .collect();
            let near_drop = others.iter().find(|d| {
                d.login_unix
                    .is_some_and(|ts| (ts - dropped_at_unix).abs() <= 120)
            });
            let at_limit = data
                .max_online_devices
                .is_some_and(|max| count >= max as usize);

            if let Some(peer) = near_drop {
                Some((
                    true,
                    format!(
                        "[SELF_SERVICE] 断联复核：检测到其他终端（{}）同期上线，疑似触发并发挤线（当前 {} 台在线）",
                        peer.ip, cap_str
                    ),
                ))
            } else if at_limit && !others.is_empty() {
                let ips: Vec<&str> = others.iter().map(|d| d.ip.as_str()).collect();
                Some((
                    true,
                    format!(
                        "[SELF_SERVICE] 断联复核：在线终端已达上限（{} 台，其他终端：{}）",
                        cap_str,
                        ips.join("、")
                    ),
                ))
            } else {
                Some((
                    false,
                    format!(
                        "[SELF_SERVICE] 断联复核：当前 {} 台在线，未见并发挤线（多为网关会话超时）",
                        cap_str
                    ),
                ))
            }
        }
        FetchTrigger::LoginRejected { local_campus_ip } => {
            if carrier == "0" && data.traffic.campus_remaining_mb <= 0.1 {
                return Some((
                    true,
                    "[SELF_SERVICE] 离线诊断：校园网套餐剩余流量已用尽（0 MB），请切换运营商出口或充值".to_string(),
                ));
            }
            let others: Vec<&str> = data
                .online_devices
                .iter()
                .filter(|d| local_campus_ip.is_empty() || d.ip != *local_campus_ip)
                .map(|d| d.ip.as_str())
                .collect();
            let at_limit = data
                .max_online_devices
                .is_some_and(|max| count >= max as usize);

            if at_limit && !others.is_empty() {
                Some((
                    true,
                    format!(
                        "[SELF_SERVICE] 离线诊断：在线终端已满（{} 台），占线 IP：{}",
                        cap_str,
                        others.join("、")
                    ),
                ))
            } else if !others.is_empty() {
                Some((
                    true,
                    format!(
                        "[SELF_SERVICE] 离线诊断：当前另有 {} 台终端在线（{}），请核对密码、运营商出口或并发上限",
                        others.len(),
                        others.join("、")
                    ),
                ))
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_login_page_extracts_checkcode_and_detects_captcha() {
        let valid_html = r#"
        <html><body>
          <form method="post" action="/Self/login/verify">
            <input type="hidden" name="checkcode" value="abc-999" />
            <div id="randomDiv" class="form-group hide"></div>
          </form>
          <script>var md5 = true;</script>
        </body></html>"#;
        assert_eq!(parse_login_page(valid_html).unwrap(), "abc-999");

        let captcha_html = valid_html.replace("class=\"form-group hide\"", "class=\"form-group\"");
        assert_eq!(
            parse_login_page(&captcha_html),
            Err(PortalError::CaptchaRequired)
        );
    }

    #[test]
    fn parse_dashboard_account_extracts_whitelist_fields_only() {
        let html = r#"<script>(function (account) {
            showAccount(account);
        })({
            "useFlow": 535.203,
            "leftFlow": 553035.463,
            "internetDownFlow": 285092.923,
            "internetUpFlow": 79914.447,
            "userPassword": "fake-secret-never-read",
            "userGroup": { "ipMaxCount": 3, "userGroupDescription": "每月免费30G" }
        });</script>"#;
        let (traffic, max_online) = parse_dashboard_account(html).unwrap();
        assert!((traffic.campus_used_mb - 535.203).abs() < 0.001);
        assert!((traffic.campus_remaining_mb - 553035.463).abs() < 0.001);
        assert!((traffic.month_down_mb - 285092.923).abs() < 0.001);
        assert!((traffic.month_up_mb - 79914.447).abs() < 0.001);
        assert_eq!(traffic.plan_description, "每月免费30G");
        assert_eq!(max_online, Some(3));
    }

    #[test]
    fn parse_month_online_log_computes_real_duration_and_carrier_traffic() {
        let body = r#"{
            "total": 236,
            "summary": {
                "COU": 236,
                "INTERNETDOWNFLOW": 285803.96,
                "INTERNETUPFLOW": 80081.606
            },
            "rows": [
                {
                    "loginTime": 1795295793000,
                    "logoutTime": 1795295960000,
                    "userIp": "192.0.2.10",
                    "time": 1,
                    "flow": 0.848,
                    "internetDownFlow": 8.486,
                    "internetUpFlow": 2.72
                },
                {
                    "loginTime": 1795295966000,
                    "logoutTime": 1795297915000,
                    "userIp": "192.0.2.10",
                    "time": 0,
                    "flow": 0,
                    "internetDownFlow": 123.59,
                    "internetUpFlow": 163.191
                }
            ]
        }"#;
        let (sessions, down, up, count) = parse_month_online_log(body).unwrap();
        assert_eq!(count, Some(236));
        assert!((down.unwrap() - 285803.96).abs() < 0.01);
        assert!((up.unwrap() - 80081.606).abs() < 0.01);
        assert_eq!(sessions.len(), 2);
        // 按上线时间倒序排列
        assert_eq!(sessions[0].login_unix_ms, 1795295966000);
        assert_eq!(sessions[0].duration_secs, 1949);
        assert!((sessions[0].actual_total_mb() - 286.781).abs() < 0.01);
    }

    #[test]
    fn diagnosis_respects_multi_carrier_and_detects_real_conflicts() {
        let mut data = SelfServiceData {
            account_key: "k".into(),
            traffic: SelfServiceTraffic {
                campus_remaining_mb: 0.0,
                ..Default::default()
            },
            max_online_devices: Some(3),
            online_devices: vec![
                OnlineDevice {
                    ip: "192.0.2.1".into(),
                    login_time: "2026-11-22 05:00:00".into(),
                    login_unix: Some(1_000),
                    terminal_type: "PC".into(),
                    use_time_secs: Some(600),
                },
                OnlineDevice {
                    ip: "192.0.2.2".into(),
                    login_time: "2026-11-22 04:00:00".into(),
                    login_unix: Some(500),
                    terminal_type: "手机".into(),
                    use_time_secs: Some(1100),
                },
            ],
            recent_sessions: Vec::new(),
            fetched_at_unix: 1_600,
        };

        // 1. 仅 2/3 台且对方早已上线 -> 复核不误判为挤线
        let (_, msg) = build_trigger_diagnosis(
            &FetchTrigger::PostReconnect {
                dropped_at_unix: 1_550,
                local_campus_ip: "192.0.2.1".into(),
            },
            "2",
            &data,
        )
        .unwrap();
        assert!(msg.contains("未见并发挤线"));

        // 2. 校园用户且剩余流量 0 -> 离线诊断精准报流量耗尽
        let (warn, msg) = build_trigger_diagnosis(
            &FetchTrigger::LoginRejected {
                local_campus_ip: "192.0.2.1".into(),
            },
            "0",
            &data,
        )
        .unwrap();
        assert!(warn && msg.contains("流量已用尽"));

        // 3. 满 3/3 且被拒登 -> 精准列出占线 IP
        data.online_devices.push(OnlineDevice {
            ip: "192.0.2.3".into(),
            login_time: "2026-11-22 05:50:00".into(),
            login_unix: Some(1_540),
            terminal_type: "PC".into(),
            use_time_secs: Some(60),
        });
        let (warn, msg) = build_trigger_diagnosis(
            &FetchTrigger::LoginRejected {
                local_campus_ip: "192.0.2.9".into(),
            },
            "2",
            &data,
        )
        .unwrap();
        assert!(warn && msg.contains("在线终端已满") && msg.contains("192.0.2.3"));
    }
}
