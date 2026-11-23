//! 多校区 Dr.COM 认证网关自动探测、竞速选路与强制门户嗅探（育才 / 雁山 / 王城 / 自定义）。
//!
//! 设计要点：
//! 1. **零常态开销**：一旦锁定可用网关即缓存在内存中，日常 30s 巡检仅单点请求，不产生额外探测流量。
//! 2. **跨校区无感切换**：当出网路由改变（如合盖从育才去雁山/王城）或原网关失联时，自动失效缓存并重新竞速+嗅探。
//! 3. **双重安全防钓鱼门禁**：离线强制门户（Captive Portal 302 / HTML 跳转）嗅探到的地址，必须同时满足：
//!    - 主机名属于 `*.gxnu.edu.cn` 或师大校园内网/教育网段（拒绝咖啡厅、酒店、`192.168.0.1/1.1` 家用路由与外网域名）；
//!    - 请求 `/drcom/chkstatus` 返回合法的 Dr.COM JSONP 状态报文（确认确系 Dr.COM 认证控制器），绝不向非 Dr.COM 节点发送学号密码。

use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use url::Url;

use crate::campus_status::{parse_chkstatus_detail, CampusStatusInfo};
use crate::http;
use crate::logging;
pub use crate::models::normalize_custom_portal_origin;
use crate::models::CampusZone;

pub const YUCAI_HTTPS_ORIGIN: &str = "https://yc.gxnu.edu.cn";
pub const YUCAI_IP_ORIGIN: &str = "http://192.168.125.13";
pub const YANSHAN_IP_ORIGIN: &str = "http://192.168.125.21";

/// 用于在离线未认证态触发校园网 AC 302 重定向的纯明文 HTTP 探针地址。
const CAPTIVE_SNIFF_URL: &str = "http://www.msftconnecttest.com/connecttest.txt";
const CAPTIVE_EXPECT_BODY: &str = "Microsoft Connect Test";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPortal {
    /// 规范化网关源（无尾斜杠，如 `"https://yc.gxnu.edu.cn"` 或 `"http://192.168.125.21"`）。
    pub origin: String,
    /// 面向界面展示的简短可读标签（如 `"育才网关 (yc.gxnu.edu.cn)"`、`"雁山网关 (192.168.125.21)"`）。
    pub label: String,
    /// 简短校区名（如 `"育才网关"`、`"雁山网关"`、`"王城/校内网关"`），供主页状态栏紧凑展示。
    pub short_name: String,
    pub login_url: String,
    pub logout_url: String,
    pub chkstatus_url: String,
}

impl ResolvedPortal {
    pub fn from_origin(origin: impl Into<String>, hint_zone: Option<CampusZone>) -> Self {
        let clean = origin.into().trim_end_matches('/').to_string();
        let (short_name, label) = classify_origin_labels(&clean, hint_zone);
        Self {
            login_url: format!("{clean}/drcom/login"),
            logout_url: format!("{clean}/drcom/logout"),
            chkstatus_url: format!("{clean}/drcom/chkstatus"),
            origin: clean,
            label,
            short_name,
        }
    }
}

#[derive(Debug, Clone)]
struct GatewayConfig {
    zone: CampusZone,
    custom_origin: Option<String>,
    #[cfg(test)]
    test_candidates: Option<Vec<String>>,
}

pub struct PortalGatewayResolver {
    config: Mutex<GatewayConfig>,
    cached: Mutex<Option<ResolvedPortal>>,
}

impl Default for PortalGatewayResolver {
    fn default() -> Self {
        Self::new(CampusZone::Auto, "")
    }
}

impl PortalGatewayResolver {
    pub fn new(zone: CampusZone, custom_portal_url: &str) -> Self {
        Self {
            config: Mutex::new(GatewayConfig {
                zone,
                custom_origin: normalize_custom_portal_origin(custom_portal_url),
                #[cfg(test)]
                test_candidates: None,
            }),
            cached: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub fn with_test_candidates(candidates: Vec<String>) -> Self {
        Self {
            config: Mutex::new(GatewayConfig {
                zone: CampusZone::Auto,
                custom_origin: None,
                test_candidates: Some(candidates),
            }),
            cached: Mutex::new(None),
        }
    }

    /// 更新校区与自定义网关设置；若配置变化则清空旧网关缓存。
    pub fn update_config(&self, zone: CampusZone, custom_portal_url: &str) -> bool {
        let next_custom = normalize_custom_portal_origin(custom_portal_url);
        let mut cfg = self.config.lock().unwrap();
        let changed = cfg.zone != zone || cfg.custom_origin != next_custom;
        if changed {
            cfg.zone = zone;
            cfg.custom_origin = next_custom;
            drop(cfg);
            *self.cached.lock().unwrap() = None;
        }
        changed
    }

    /// 当出网路由发生切换时清空自动探测缓存，以便跨校区移动后重新识别当前网关。
    pub fn invalidate_cache(&self) {
        *self.cached.lock().unwrap() = None;
    }

    /// 当某个网关源请求失败时按源地址精确失效缓存，避免并发下误清刚切好的新网关。
    pub fn invalidate_if_origin(&self, failed_origin: &str) {
        let mut guard = self.cached.lock().unwrap();
        if guard.as_ref().is_some_and(|c| c.origin == failed_origin) {
            *guard = None;
        }
    }

    pub fn is_custom_mode(&self) -> bool {
        self.config.lock().unwrap().zone == CampusZone::Custom
    }

    /// 仅读取当前已缓存的网关端点（不触发网络探测）。
    pub fn cached_endpoints(&self) -> Option<ResolvedPortal> {
        self.cached.lock().unwrap().clone()
    }

    /// 返回当前已缓存网关（若有）；未缓存时执行一次快速探测，若完全离校则平滑回退到首选候选网关。
    pub fn resolve_endpoints(&self) -> ResolvedPortal {
        if let Some(cached) = self.cached.lock().unwrap().clone() {
            return cached;
        }
        if let Ok((resolved, _, _)) = self.probe_and_cache() {
            return resolved;
        }
        self.default_fallback_portal()
    }

    /// 强制重新探测当前网络环境下可用的校园网 Dr.COM 网关，测量真实 RTT 并刷新缓存。
    /// 返回 `(网关端点, 往返毫秒, 门户在线状态详情)`。
    pub fn probe_and_cache(&self) -> Result<(ResolvedPortal, u64, CampusStatusInfo), String> {
        let cfg = self.config.lock().unwrap().clone();

        if cfg.zone == CampusZone::Custom {
            let Some(origin) = cfg.custom_origin else {
                return Err(
                    "自定义网关地址为空或格式无效，请在设置中填写有效的 http(s):// 地址".into(),
                );
            };
            let portal = ResolvedPortal::from_origin(origin, Some(CampusZone::Custom));
            let agent = fast_portal_agent(3);
            return match probe_chkstatus_once(&agent, &portal) {
                Some((ms, info)) => {
                    *self.cached.lock().unwrap() = Some(portal.clone());
                    Ok((portal, ms, info))
                }
                None => Err(format!(
                    "自定义网关 {} 未返回有效的 Dr.COM 状态响应",
                    portal.origin
                )),
            };
        }

        #[cfg(test)]
        if let Some(custom_list) = &cfg.test_candidates {
            let refs: Vec<&str> = custom_list.iter().map(String::as_str).collect();
            return match discover_best_gateway(&refs, cfg.zone, false) {
                Some((portal, ms, info)) => {
                    *self.cached.lock().unwrap() = Some(portal.clone());
                    Ok((portal, ms, info))
                }
                None => Err("未探测到模拟测试网关".into()),
            };
        }

        let candidates = candidate_origins_for(cfg.zone);
        let need_captive_sniff = matches!(
            cfg.zone,
            CampusZone::Auto | CampusZone::Wangcheng | CampusZone::Yanshan
        );

        let discovered = discover_best_gateway(&candidates, cfg.zone, need_captive_sniff);
        match discovered {
            Some((portal, ms, info)) => {
                let prev_origin = self
                    .cached
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|p| p.origin.clone());
                if prev_origin.as_deref() != Some(portal.origin.as_str()) {
                    logging::info(&format!(
                        "[PORTAL_GATEWAY] 已锁定认证网关：{}（{} ms）",
                        portal.label, ms
                    ));
                }
                *self.cached.lock().unwrap() = Some(portal.clone());
                Ok((portal, ms, info))
            }
            None => Err("未探测到可达的师大校园网认证网关（请确认已连入 GXNU 校园网）".into()),
        }
    }

    fn default_fallback_portal(&self) -> ResolvedPortal {
        let cfg = self.config.lock().unwrap().clone();
        match cfg.zone {
            CampusZone::Custom => {
                let origin = cfg
                    .custom_origin
                    .unwrap_or_else(|| YUCAI_HTTPS_ORIGIN.to_string());
                ResolvedPortal::from_origin(origin, Some(CampusZone::Custom))
            }
            CampusZone::Yanshan => {
                ResolvedPortal::from_origin(YANSHAN_IP_ORIGIN, Some(CampusZone::Yanshan))
            }
            CampusZone::Wangcheng => {
                ResolvedPortal::from_origin(YUCAI_HTTPS_ORIGIN, Some(CampusZone::Wangcheng))
            }
            CampusZone::Auto | CampusZone::Yucai => {
                ResolvedPortal::from_origin(YUCAI_HTTPS_ORIGIN, Some(CampusZone::Yucai))
            }
        }
    }
}

fn candidate_origins_for(zone: CampusZone) -> Vec<&'static str> {
    match zone {
        CampusZone::Auto | CampusZone::Wangcheng => {
            vec![YUCAI_HTTPS_ORIGIN, YUCAI_IP_ORIGIN, YANSHAN_IP_ORIGIN]
        }
        CampusZone::Yucai => vec![YUCAI_HTTPS_ORIGIN, YUCAI_IP_ORIGIN],
        CampusZone::Yanshan => vec![YANSHAN_IP_ORIGIN, YUCAI_HTTPS_ORIGIN, YUCAI_IP_ORIGIN],
        CampusZone::Custom => Vec::new(),
    }
}

fn fast_portal_agent(timeout_secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(Duration::from_secs(timeout_secs + 1))
        .timeout_connect(Duration::from_millis(1500))
        .timeout_read(Duration::from_millis(timeout_secs * 1000))
        .build()
}

fn fast_no_redirect_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(Duration::from_millis(2200))
        .timeout_connect(Duration::from_millis(1200))
        .timeout_read(Duration::from_millis(1200))
        .redirects(0)
        .build()
}

pub(crate) fn probe_chkstatus_once(
    agent: &ureq::Agent,
    portal: &ResolvedPortal,
) -> Option<(u64, CampusStatusInfo)> {
    let url = format!("{}?callback=dr1002&v=4532&lang=zh", portal.chkstatus_url);
    let started = Instant::now();
    let resp = http::get_portal(agent, &url).ok()?;
    if !(200..300).contains(&resp.status) {
        return None;
    }
    let mut detail = parse_chkstatus_detail(&resp.body)?;
    detail.gateway_label = Some(portal.short_name.clone());
    let rtt_ms = started.elapsed().as_millis().max(1) as u64;
    Some((rtt_ms, detail))
}

/// 并行探测候选校区网关（可选同时发起离线强制门户 302/HTML 嗅探），择优返回响应最快且合法的 Dr.COM 网关。
fn discover_best_gateway(
    candidates: &[&str],
    zone: CampusZone,
    with_captive_sniff: bool,
) -> Option<(ResolvedPortal, u64, CampusStatusInfo)> {
    let mut results: Vec<(usize, ResolvedPortal, u64, CampusStatusInfo)> = Vec::new();

    std::thread::scope(|scope| {
        let mut handles = Vec::new();

        for (idx, &origin) in candidates.iter().enumerate() {
            let origin_owned = origin.to_string();
            handles.push(scope.spawn(move || {
                let agent = fast_portal_agent(2);
                let hint = if zone == CampusZone::Auto {
                    None
                } else {
                    Some(zone)
                };
                let portal = ResolvedPortal::from_origin(origin_owned, hint);
                probe_chkstatus_once(&agent, &portal).map(|(ms, info)| (idx + 1, portal, ms, info))
            }));
        }

        let sniff_handle = if with_captive_sniff {
            Some(scope.spawn(move || {
                let sniffed_origin = sniff_captive_redirect_origin()?;
                // 若嗅探到的地址已在候选列表中，交给候选线程或赋予最高优先级（0 = 当前真实拦截网关！）
                let agent = fast_portal_agent(2);
                let hint = if zone == CampusZone::Wangcheng {
                    Some(CampusZone::Wangcheng)
                } else {
                    None
                };
                let portal = ResolvedPortal::from_origin(sniffed_origin, hint);
                probe_chkstatus_once(&agent, &portal).map(|(ms, info)| (0usize, portal, ms, info))
            }))
        } else {
            None
        };

        if let Some(h) = sniff_handle {
            if let Ok(Some(hit)) = h.join() {
                results.push(hit);
            }
        }
        for h in handles {
            if let Ok(Some(hit)) = h.join() {
                results.push(hit);
            }
        }
    });

    // 排序规则：
    // 1) 强制门户直接嗅探命中的当前拦截 AC（rank == 0）最优先（100% 代表本网段真实认证控制器）；
    // 2) 若同为预设候选，优先选择同源 HTTPS（如 yc.gxnu.edu.cn 优于裸 IP 192.168.125.13），其次按 RTT 升序。
    results.sort_by_key(|(rank, portal, ms, _)| {
        let is_sniffed = if *rank == 0 { 0u8 } else { 1u8 };
        let https_pref = if portal.origin.starts_with("https://") {
            0u8
        } else {
            1u8
        };
        (is_sniffed, *rank, https_pref, *ms)
    });

    results
        .into_iter()
        .next()
        .map(|(_, portal, ms, info)| (portal, ms, info))
}

/// 发起一次不跟随重定向的明文 HTTP 探测，若被师大校园网 AC 302 跳转或注入跳转脚本，提取并校验网关源。
fn sniff_captive_redirect_origin() -> Option<String> {
    let agent = fast_no_redirect_agent();
    let resp = http::get_response(&agent, CAPTIVE_SNIFF_URL).ok()?;
    let status = resp.status();
    let location = resp.header("Location").map(str::to_string);
    let body = if status == 200 {
        http::read_text(resp, 8 * 1024).unwrap_or_default()
    } else {
        http::drain_small_body(resp);
        String::new()
    };
    extract_captive_portal_origin(location.as_deref(), &body)
}

/// 从强制门户 302 `Location` 头或 200 HTML 跳转页中提取受信的师大校园网网关 `scheme://host[:port]`。
pub fn extract_captive_portal_origin(
    location_header: Option<&str>,
    html_body: &str,
) -> Option<String> {
    if let Some(loc) = location_header.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(origin) = validate_captive_target_url(loc) {
            return Some(origin);
        }
    }

    let trimmed = html_body.trim();
    if trimmed.is_empty() || trimmed.contains(CAPTIVE_EXPECT_BODY) {
        return None;
    }

    // 解析常见的 Dr.COM 强制门户 HTML 注入跳转（如 `location.href="http://192.168.125.21/..."` 或 `url=http://...`）
    for prefix in ["http://", "https://"] {
        let mut rest = trimmed;
        while let Some(pos) = rest.find(prefix) {
            let sub = &rest[pos..];
            let end = sub
                .find(|ch: char| {
                    matches!(
                        ch,
                        '"' | '\'' | '>' | '<' | ' ' | '\r' | '\n' | '\t' | ';' | ')'
                    )
                })
                .unwrap_or(sub.len());
            let candidate = &sub[..end];
            if let Some(origin) = validate_captive_target_url(candidate) {
                return Some(origin);
            }
            rest = &sub[prefix.len()..];
        }
    }
    None
}

fn validate_captive_target_url(raw_url: &str) -> Option<String> {
    let parsed = Url::parse(raw_url.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let host = parsed.host_str()?;
    if !is_trusted_gxnu_portal_host(host) {
        return None;
    }
    Some(origin_string(&parsed))
}

/// 严格校验主机名是否属于师大校园网门户可信范围（防止在公共 Wi-Fi/家用路由下误将凭据发给外部强制门户）。
pub fn is_trusted_gxnu_portal_host(host: &str) -> bool {
    let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    if lower == "gxnu.edu.cn" || lower.ends_with(".gxnu.edu.cn") {
        return true;
    }
    if let Ok(ipv4) = lower.parse::<Ipv4Addr>() {
        let [a, b, c, _] = ipv4.octets();
        // 1) 192.168.125.0/24（师大育才 .13 / 雁山 .21 等 Dr.COM AC 专用网段；排除 192.168.0.x/1.x 家用路由）
        if a == 192 && b == 168 && c == 125 {
            return true;
        }
        // 2) 172.16.0.0/12（师大内网核心网段，如 172.16.x.x）
        if a == 172 && (16..=31).contains(&b) {
            return true;
        }
        // 3) 10.0.0.0/8（高校大型内网汇聚段）
        if a == 10 {
            return true;
        }
        // 4) 202.193.160.0/20（广西师范大学 CERNET 官方公网段 202.193.160.0 - 202.193.175.255）
        if a == 202 && b == 193 && (160..=175).contains(&c) {
            return true;
        }
    }
    false
}

fn origin_string(parsed: &Url) -> String {
    let host = parsed.host_str().unwrap_or_default();
    let host_fmt = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_ascii_lowercase()
    };
    match parsed.port() {
        Some(port) => format!("{}://{}:{}", parsed.scheme(), host_fmt, port),
        None => format!("{}://{}", parsed.scheme(), host_fmt),
    }
}

fn classify_origin_labels(origin: &str, hint_zone: Option<CampusZone>) -> (String, String) {
    let host = origin
        .split("://")
        .nth(1)
        .unwrap_or(origin)
        .trim_end_matches('/');

    if hint_zone == Some(CampusZone::Custom) {
        return ("自定义网关".to_string(), format!("自定义网关 ({host})"));
    }
    if host.eq_ignore_ascii_case("yc.gxnu.edu.cn") || host == "192.168.125.13" {
        if hint_zone == Some(CampusZone::Wangcheng) {
            return (
                "王城/育才网关".to_string(),
                format!("王城/育才共享网关 ({host})"),
            );
        }
        return ("育才网关".to_string(), format!("育才校区网关 ({host})"));
    }
    if host == "192.168.125.21" || host.eq_ignore_ascii_case("ys.gxnu.edu.cn") {
        return ("雁山网关".to_string(), format!("雁山校区网关 ({host})"));
    }
    if host.eq_ignore_ascii_case("wc.gxnu.edu.cn") || hint_zone == Some(CampusZone::Wangcheng) {
        return ("王城网关".to_string(), format!("王城校区网关 ({host})"));
    }
    ("校园网网关".to_string(), format!("自动识别网关 ({host})"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn normalizes_custom_portal_inputs_cleanly() {
        assert_eq!(
            normalize_custom_portal_origin("192.168.125.21").as_deref(),
            Some("http://192.168.125.21")
        );
        assert_eq!(
            normalize_custom_portal_origin("https://YC.GXNU.EDU.CN/drcom/login?callback=dr1004")
                .as_deref(),
            Some("https://yc.gxnu.edu.cn")
        );
        assert_eq!(
            normalize_custom_portal_origin("http://192.168.125.21:801/eportal/").as_deref(),
            Some("http://192.168.125.21:801")
        );
        assert!(normalize_custom_portal_origin("").is_none());
        assert!(normalize_custom_portal_origin("ftp://192.168.125.21").is_none());
        assert!(normalize_custom_portal_origin("http://user:pass@192.168.125.21").is_none());
    }

    #[test]
    fn whitelist_accepts_gxnu_campuses_and_rejects_external_or_home_routers() {
        assert!(is_trusted_gxnu_portal_host("yc.gxnu.edu.cn"));
        assert!(is_trusted_gxnu_portal_host("wc.gxnu.edu.cn"));
        assert!(is_trusted_gxnu_portal_host("ys.gxnu.edu.cn"));
        assert!(is_trusted_gxnu_portal_host("192.168.125.13"));
        assert!(is_trusted_gxnu_portal_host("192.168.125.21"));
        assert!(is_trusted_gxnu_portal_host("172.16.4.2"));
        assert!(is_trusted_gxnu_portal_host("202.193.160.33"));

        assert!(!is_trusted_gxnu_portal_host("192.168.1.1"));
        assert!(!is_trusted_gxnu_portal_host("192.168.0.1"));
        assert!(!is_trusted_gxnu_portal_host("portal.starbucks.com"));
        assert!(!is_trusted_gxnu_portal_host("gxnu.edu.cn.evil.com"));
    }

    #[test]
    fn captive_redirect_sniffer_extracts_trusted_origin_from_header_or_html() {
        assert_eq!(
            extract_captive_portal_origin(
                Some("http://192.168.125.21/a79.htm?wlanuserip=172.16.80.9"),
                ""
            )
            .as_deref(),
            Some("http://192.168.125.21")
        );
        assert_eq!(
            extract_captive_portal_origin(
                None,
                r#"<html><script>top.self.location.href='http://192.168.125.19/index.htm';</script></html>"#
            )
            .as_deref(),
            Some("http://192.168.125.19")
        );
        // 正常公网探测正文或非师大钓鱼重定向绝不采信
        assert!(extract_captive_portal_origin(None, "Microsoft Connect Test").is_none());
        assert!(extract_captive_portal_origin(Some("http://192.168.1.1/login.html"), "").is_none());
        assert!(extract_captive_portal_origin(
            Some("https://phishing.example.com/drcom/login"),
            ""
        )
        .is_none());
    }

    #[test]
    fn resolver_races_candidates_and_locks_onto_live_drcom_gateway() {
        // 候选 1：拒绝连接的死端口；候选 2：正常返回 dr1002 的模拟雁山网关
        let dead = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);

        let live = TcpListener::bind("127.0.0.1:0").unwrap();
        let live_port = live.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = live.accept().unwrap();
            http::mock::read_request(&mut stream);
            let body = r#"dr1002({"result":0,"v46ip":"172.16.99.8"});"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let dead_origin = format!("http://127.0.0.1:{dead_port}");
        let live_origin = format!("http://127.0.0.1:{live_port}");
        let hit = discover_best_gateway(
            &[dead_origin.as_str(), live_origin.as_str()],
            CampusZone::Auto,
            false,
        )
        .expect("should discover live mock Dr.COM gateway");
        server.join().unwrap();

        assert_eq!(hit.0.origin, live_origin);
        assert!(!hit.2.online);
        assert_eq!(hit.2.portal_ip.as_deref(), Some("172.16.99.8"));
    }

    #[test]
    fn authenticator_and_probe_seamlessly_fail_over_when_campus_changes() {
        use std::sync::Arc;

        // 模拟校区 A（如育才）仅响应第一次 chkstatus，随后失联（合盖去了雁山）
        let campus_a = TcpListener::bind("127.0.0.1:0").unwrap();
        let port_a = campus_a.local_addr().unwrap().port();
        let server_a = std::thread::spawn(move || {
            let (mut s, _) = campus_a.accept().unwrap();
            http::mock::read_request(&mut s);
            let body = r#"dr1002({"result":1,"v46ip":"172.16.24.10"});"#;
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            // 随后 campus_a 监听器被丢弃，后续连接将立即 ConnectionRefused
        });

        // 模拟校区 B（如雁山）在校区 A 失联后接管重探与 /drcom/login 认证请求
        let campus_b = TcpListener::bind("127.0.0.1:0").unwrap();
        let port_b = campus_b.local_addr().unwrap().port();
        let server_b = std::thread::spawn(move || {
            use std::io::Read;
            for _ in 0..3 {
                let Ok((mut s, _)) = campus_b.accept() else {
                    break;
                };
                let mut req_buf = Vec::new();
                let mut chunk = [0u8; 256];
                while let Ok(n) = s.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    req_buf.extend_from_slice(&chunk[..n]);
                    if req_buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let req = String::from_utf8_lossy(&req_buf);
                let is_login = req.contains("/drcom/login");
                let body = if is_login {
                    r#"dr1004({"result":1,"msg":"登录成功"});"#
                } else {
                    r#"dr1002({"result":0,"v46ip":"172.16.88.21"});"#
                };
                write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
                if is_login {
                    break;
                }
            }
        });

        let origin_a = format!("http://127.0.0.1:{port_a}");
        let origin_b = format!("http://127.0.0.1:{port_b}");
        let resolver = Arc::new(PortalGatewayResolver::with_test_candidates(vec![
            origin_a.clone(),
            origin_b.clone(),
        ]));

        let probe = crate::campus_status::CampusStatusProbe::with_resolver(Arc::clone(&resolver));
        let first = probe.check_detail().expect("initial campus A check");
        assert!(first.online);
        assert_eq!(resolver.resolve_endpoints().origin, origin_a);
        server_a.join().unwrap();

        // 此时校区 A 已关闭端口；发起认证时应自动感知 A 失联、重探切换到校区 B 并一次认证成功
        let auth = crate::authenticator::CampusAuthenticator::with_resolver(Arc::clone(&resolver));
        let res = auth.authenticate("20240001", "secret", "2", "172.16.88.21");
        assert!(
            res.success,
            "expected seamless failover login, got: {}",
            res.message
        );
        assert_eq!(resolver.resolve_endpoints().origin, origin_b);
        server_b.join().unwrap();
    }
}
