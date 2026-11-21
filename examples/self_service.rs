//! GXNU 校园网自助服务 HTTP 探针。
//! 运行：`.\build.ps1 run --locked --example self_service`
//! 流量采用门户标注的 M。首页记录是门户提供的近期上网记录。

#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/credentials.rs"]
mod credentials;
#[allow(dead_code)]
#[path = "../src/http.rs"]
mod http;
#[allow(dead_code)]
#[path = "../src/logging.rs"]
mod logging;
#[allow(dead_code)]
#[path = "../src/metrics_store.rs"]
mod metrics_store;
#[allow(dead_code)]
#[path = "../src/models.rs"]
mod models;

use credentials::StoredCredential;
use scraper::{Html, Selector};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::{net::IpAddr, process::ExitCode};
use ureq::{Agent, Response};
use url::Url;

const ORIGIN: &str = "http://172.16.4.2";
const ENTRY: &str = "http://172.16.4.2/Self/";
const LOGIN: &str = "http://172.16.4.2/Self/login/?302=LI";
const VERIFY: &str = "http://172.16.4.2/Self/login/verify";
const DASHBOARD: &str = "http://172.16.4.2/Self/dashboard";
const SESSION_IMAGE: &str = "http://172.16.4.2/Self/login/randomCode";

#[derive(Serialize)]
struct Snapshot {
    traffic: Traffic,
    online_sessions: Vec<OnlineSession>,
    login_history: Vec<LoginRecord>,
}

#[derive(Deserialize, Serialize)]
struct Traffic {
    #[serde(rename(deserialize = "useFlow"))]
    used_m: f64,
    #[serde(rename(deserialize = "leftFlow"))]
    remaining_m: f64,
}

#[derive(Deserialize, Serialize)]
struct OnlineSession {
    ip: IpAddr,
    /// 门户直接返回的本地时间。
    #[serde(rename(deserialize = "loginTime"))]
    login_time: String,
}

#[derive(Serialize)]
struct LoginRecord {
    login_unix_ms: i64,
    logout_unix_ms: Option<i64>,
    ip: IpAddr,
    duration_minutes: f64,
    traffic_m: f64,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("自助服务查询失败：{err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let settings = config::SettingsStore::new()
        .load_strict()
        .map_err(|_| "无法读取守护助手的配置")?;
    let account_id = settings
        .active_account()
        .map_or(settings.active_account_id.as_str(), |profile| &profile.id);
    let credential = credentials::load_for_account(account_id, &settings.student_id)?
        .ok_or("当前账号没有已保存的 Windows 凭据")?;

    // 独立的直连会话，Cookie 只保存在 Agent 内存中。
    let agent = http::no_redirect_agent(8);
    let dashboard = login(&agent, &credential)?;
    let traffic = read_traffic(&dashboard)?;
    let online_sessions = dashboard_json(&agent, "getOnlineList", "读取在线设备")?;
    let history = dashboard_json(&agent, "getLoginHistory", "读取近期上网记录")?;
    let snapshot = Snapshot {
        traffic,
        online_sessions,
        login_history: read_login_history(history)?,
    };

    let json = serde_json::to_string_pretty(&snapshot).map_err(|_| "无法生成查询结果 JSON")?;
    println!("{json}");
    Ok(())
}

fn login(agent: &Agent, credential: &StoredCredential) -> Result<String, String> {
    let response = checked_response("打开自助服务", agent.get(ENTRY).call())?;
    require_redirect(&response, DASHBOARD, "打开自助服务")?;
    let response = checked_response("打开账户首页", agent.get(DASHBOARD).call())?;
    require_redirect(&response, LOGIN, "打开账户首页")?;

    let page = get_page(agent, LOGIN, "打开登录表单")?;
    let document = Html::parse_document(&page);
    let form = document
        .select(&selector("form[method=post][action]"))
        .next()
        .ok_or("未找到自助服务登录表单")?;
    let action = form.value().attr("action").unwrap_or_default();
    if !matches_destination(LOGIN, action, VERIFY) {
        return Err("登录表单提交地址与预期不符".into());
    }

    if form.select(&selector("#randomDiv.hide")).next().is_none() {
        return Err("门户要求验证码，无法自动提交".into());
    }
    let checkcode = form
        .select(&selector("input[type=hidden][name=checkcode]"))
        .next()
        .and_then(|input| input.value().attr("value"))
        .filter(|value| !value.is_empty())
        .ok_or("登录表单缺少 checkcode")?;
    if !page.contains("var md5 = true;") {
        return Err("登录页的密码处理方式已变化".into());
    }

    // 登录页加载时会请求该资源。沿用同一个会话，验证码字段保持空白。
    let image = checked_response(
        "加载登录表单资源",
        agent.get(SESSION_IMAGE).set("Referer", LOGIN).call(),
    )?;
    if image.status() != 200 {
        return Err(format!("登录表单资源返回 HTTP {}", image.status()));
    }
    drop(image);

    // 门户的旧版 MD5 脚本将字符单元按 8 位偏移装入消息块。
    let password_bytes = credential
        .password
        .encode_utf16()
        .map(u8::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "当前密码含有超出门户单字节 MD5 兼容范围的字符")?;
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
                ("checkcode", checkcode),
                ("account", &credential.username),
                ("password", &digest),
                ("code", ""),
            ]),
    )?;
    require_redirect(&response, DASHBOARD, "验证登录结果")?;
    get_page(agent, DASHBOARD, "读取登录后的账户首页")
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

fn require_redirect(response: &Response, expected: &str, stage: &str) -> Result<(), String> {
    let destination = response.header("Location").unwrap_or_default();
    if !matches!(response.status(), 302 | 303)
        || !matches_destination(response.get_url(), destination, expected)
    {
        return Err(format!(
            "{stage}时门户未跳转到预期页面（HTTP {}）",
            response.status()
        ));
    }
    Ok(())
}

fn selector(pattern: &str) -> Selector {
    Selector::parse(pattern).expect("静态 HTML 选择器")
}

fn checked_response(
    stage: &str,
    result: Result<Response, ureq::Error>,
) -> Result<Response, String> {
    match result {
        Ok(response) => Ok(response),
        Err(ureq::Error::Status(status, _)) => Err(format!("{stage}时门户返回 HTTP {status}")),
        Err(ureq::Error::Transport(error)) => {
            Err(format!("{stage}时连接失败（{:?}）", error.kind()))
        }
    }
}

fn get_page(agent: &Agent, url: &str, stage: &str) -> Result<String, String> {
    let response = checked_response(stage, agent.get(url).call())?;
    if response.status() != 200 {
        return Err(format!("{stage}时门户返回 HTTP {}", response.status()));
    }
    response
        .into_string()
        .map_err(|_| format!("{stage}时无法读取响应"))
}

fn read_traffic(page: &str) -> Result<Traffic, String> {
    let document = Html::parse_document(page);
    // 账户数据作为内嵌脚本的 JSON 参数下发。只反序列化流量字段。
    for script in document
        .select(&selector("script:not([src])"))
        .flat_map(|element| element.text())
    {
        for (index, delimiter) in script.match_indices("})(") {
            let json = &script[index + delimiter.len()..];
            let mut values = serde_json::Deserializer::from_str(json).into_iter::<Traffic>();
            if let Some(Ok(traffic)) = values.next() {
                return Ok(traffic);
            }
        }
    }
    Err("门户首页缺少已用流量或可用流量".into())
}

fn dashboard_json<T: DeserializeOwned>(
    agent: &Agent,
    endpoint: &str,
    stage: &str,
) -> Result<T, String> {
    let body = get_page(agent, &format!("{DASHBOARD}/{endpoint}"), stage)?;
    serde_json::from_str(&body).map_err(|_| format!("{stage}时收到无法识别的数据"))
}

fn read_login_history(rows: Vec<Vec<Value>>) -> Result<Vec<LoginRecord>, String> {
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            read_login_record(row).ok_or_else(|| format!("第 {} 条近期上网记录格式变化", index + 1))
        })
        .collect()
}

fn read_login_record(row: &[Value]) -> Option<LoginRecord> {
    // 首页表格的前六列按固定顺序返回。
    let [login, logout, ip, _mac, duration, traffic, ..] = row else {
        return None;
    };
    Some(LoginRecord {
        login_unix_ms: login.as_i64()?,
        logout_unix_ms: if logout.is_null() {
            None
        } else {
            Some(logout.as_i64()?)
        },
        ip: ip.as_str()?.parse().ok()?,
        duration_minutes: duration.as_f64()?,
        traffic_m: traffic.as_f64()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn output_contains_only_requested_portal_fields() {
        let dashboard = r#"<script>(function (account) {
            showAccount(account);
        })({"useFlow":12.25,"leftFlow":1024.75,"userPassword":"fake-secret"});</script>"#;
        let online =
            r#"[{"ip":"192.0.2.1","loginTime":"2026-11-22 07:30:00","sessionId":"fake-session"}]"#;
        let history = json!([[1000, 2000, "192.0.2.2", "fake-mac", 1, 0.25, 7]]);
        let snapshot = Snapshot {
            traffic: read_traffic(dashboard).unwrap(),
            online_sessions: serde_json::from_str(online).unwrap(),
            login_history: read_login_history(serde_json::from_value(history).unwrap()).unwrap(),
        };
        assert_eq!(
            serde_json::to_value(snapshot).unwrap(),
            json!({
                "traffic": { "used_m": 12.25, "remaining_m": 1024.75 },
                "online_sessions": [{ "ip": "192.0.2.1", "login_time": "2026-11-22 07:30:00" }],
                "login_history": [{
                    "login_unix_ms": 1000,
                    "logout_unix_ms": 2000,
                    "ip": "192.0.2.2",
                    "duration_minutes": 1.0,
                    "traffic_m": 0.25,
                }],
            })
        );
    }
}
