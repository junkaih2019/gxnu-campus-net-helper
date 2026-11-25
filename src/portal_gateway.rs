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
const CAPTIVE_BODY_LIMIT: usize = 8 * 1024;
const STALE_GATEWAY: &str = "校区设置或网络出口已变化，需重新探测网关";

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

    pub(crate) fn is_https_alternative_to(&self, origin: &str) -> bool {
        if !self.origin.starts_with("https://") || !origin.starts_with("http://") {
            return false;
        }
        let (Ok(secure), Ok(plain)) = (Url::parse(&self.origin), Url::parse(origin)) else {
            return false;
        };
        let secure = secure.host_str().unwrap_or_default().trim_end_matches('.');
        let plain = plain.host_str().unwrap_or_default().trim_end_matches('.');
        !secure.is_empty()
            && (secure == plain || (secure == "yc.gxnu.edu.cn" && plain == "192.168.125.13"))
    }
}

#[derive(Debug, Clone)]
struct GatewayConfig {
    zone: CampusZone,
    custom_origin: Option<String>,
    #[cfg(test)]
    test_candidates: Option<Vec<String>>,
}

struct GatewayState {
    config: GatewayConfig,
    cached: Option<ResolvedPortal>,
    revision: u64,
}

impl GatewayState {
    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.cached = None;
    }
}

pub struct PortalGatewayResolver {
    state: Mutex<GatewayState>,
}

impl Default for PortalGatewayResolver {
    fn default() -> Self {
        Self::new(CampusZone::Auto, "")
    }
}

impl PortalGatewayResolver {
    pub fn new(zone: CampusZone, custom_portal_url: &str) -> Self {
        Self {
            state: Mutex::new(GatewayState {
                config: GatewayConfig {
                    zone,
                    custom_origin: normalize_custom_portal_origin(custom_portal_url),
                    #[cfg(test)]
                    test_candidates: None,
                },
                cached: None,
                revision: 0,
            }),
        }
    }

    #[cfg(test)]
    pub fn with_test_candidates(candidates: Vec<String>) -> Self {
        let resolver = Self::default();
        resolver.state.lock().unwrap().config.test_candidates = Some(candidates);
        resolver
    }

    /// 更新校区与自定义网关设置；若配置变化则清空旧网关缓存。
    pub fn update_config(&self, zone: CampusZone, custom_portal_url: &str) -> bool {
        let next_custom = normalize_custom_portal_origin(custom_portal_url);
        let mut state = self.state.lock().unwrap();
        let changed = state.config.zone != zone || state.config.custom_origin != next_custom;
        if changed {
            state.config.zone = zone;
            state.config.custom_origin = next_custom;
            state.invalidate();
        }
        changed
    }

    /// 当出网路由发生切换时清空自动探测缓存，以便跨校区移动后重新识别当前网关。
    pub fn invalidate_cache(&self) {
        self.state.lock().unwrap().invalidate();
    }

    /// 当某个网关源请求失败时按源地址精确失效缓存，避免并发下误清刚切好的新网关。
    pub fn invalidate_if_origin(&self, failed_origin: &str) {
        let mut state = self.state.lock().unwrap();
        if state
            .cached
            .as_ref()
            .is_some_and(|portal| portal.origin == failed_origin)
        {
            state.invalidate();
        }
    }

    pub fn is_custom_mode(&self) -> bool {
        self.state.lock().unwrap().config.zone == CampusZone::Custom
    }

    /// 仅读取当前已缓存的网关端点（不触发网络探测）。
    pub fn cached_endpoints(&self) -> Option<ResolvedPortal> {
        self.state.lock().unwrap().cached.clone()
    }

    /// 返回当前已缓存网关（若有）；未缓存时执行一次快速探测，若完全离校则平滑回退到首选候选网关。
    pub fn resolve_endpoints(&self) -> ResolvedPortal {
        if let Some(cached) = self.cached_endpoints() {
            return cached;
        }
        if let Ok((resolved, _, _)) = self.probe_and_cache() {
            return resolved;
        }
        Self::default_fallback_portal(&self.state.lock().unwrap().config)
    }

    /// 明文门户必须在当前连接上返回有效的 Dr.COM 状态才允许认证。
    pub(crate) fn resolve_login_endpoints(&self) -> Result<ResolvedPortal, String> {
        let (cached, revision) = {
            let state = self.state.lock().unwrap();
            (state.cached.clone(), state.revision)
        };
        if let Some(portal) = cached {
            if portal.origin.starts_with("https://") {
                return Ok(portal);
            }
            let verified = probe_chkstatus_once(&fast_portal_agent(2), &portal).is_some();
            let mut state = self.state.lock().unwrap();
            if state.revision != revision
                || state
                    .cached
                    .as_ref()
                    .is_some_and(|current| current.origin != portal.origin)
            {
                return Err(STALE_GATEWAY.into());
            }
            if verified {
                return Ok(portal);
            }
            if state
                .cached
                .as_ref()
                .is_some_and(|current| current.origin == portal.origin)
            {
                state.invalidate();
            }
            if state.config.zone == CampusZone::Custom {
                return Err("自定义明文网关未返回有效的 Dr.COM 状态响应".into());
            }
        }
        let revision = self.state.lock().unwrap().revision;
        match self.probe_and_cache() {
            Ok((portal, _, _)) => Ok(portal),
            Err(err) => {
                let state = self.state.lock().unwrap();
                if state.revision != revision {
                    return Err(STALE_GATEWAY.into());
                }
                let fallback = Self::default_fallback_portal(&state.config);
                if fallback.origin.starts_with("https://") {
                    Ok(fallback)
                } else {
                    Err(err)
                }
            }
        }
    }

    /// 强制重新探测当前网络环境下可用的校园网 Dr.COM 网关，测量真实 RTT 并刷新缓存。
    /// 返回 `(网关端点, 往返毫秒, 门户在线状态详情)`。
    pub fn probe_and_cache(&self) -> Result<(ResolvedPortal, u64, CampusStatusInfo), String> {
        self.probe_and_cache_with_hint(None)
    }

    pub(crate) fn probe_and_cache_with_hint(
        &self,
        captive_origin: Option<&str>,
    ) -> Result<(ResolvedPortal, u64, CampusStatusInfo), String> {
        let (config, revision) = {
            let state = self.state.lock().unwrap();
            (state.config.clone(), state.revision)
        };
        let (portal, ms, info) = probe_configured_gateway(&config, captive_origin)?;
        let mut state = self.state.lock().unwrap();
        if state.revision != revision {
            return Err(STALE_GATEWAY.into());
        }
        let changed = state
            .cached
            .as_ref()
            .is_none_or(|previous| previous.origin != portal.origin);
        state.cached = Some(portal.clone());
        drop(state);
        if changed {
            logging::info(&format!(
                "[PORTAL_GATEWAY] 已锁定认证网关：{}（{} ms）",
                portal.label, ms
            ));
        }
        Ok((portal, ms, info))
    }

    fn default_fallback_portal(cfg: &GatewayConfig) -> ResolvedPortal {
        match cfg.zone {
            CampusZone::Custom => {
                let origin = cfg
                    .custom_origin
                    .clone()
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

fn probe_configured_gateway(
    config: &GatewayConfig,
    captive_origin: Option<&str>,
) -> Result<(ResolvedPortal, u64, CampusStatusInfo), String> {
    if config.zone == CampusZone::Custom {
        let origin = config
            .custom_origin
            .as_deref()
            .ok_or("自定义网关地址为空或格式无效，请在设置中填写有效的 http(s):// 地址")?;
        let portal = ResolvedPortal::from_origin(origin, Some(CampusZone::Custom));
        let agent = fast_portal_agent(3);
        let (ms, info) = probe_chkstatus_once(&agent, &portal)
            .ok_or_else(|| format!("自定义网关 {} 未返回有效的 Dr.COM 状态响应", portal.origin))?;
        return Ok((portal, ms, info));
    }

    #[cfg(test)]
    if let Some(custom_list) = &config.test_candidates {
        let refs: Vec<&str> = custom_list.iter().map(String::as_str).collect();
        return discover_best_gateway(&refs, config.zone, false, None)
            .ok_or_else(|| "未探测到模拟测试网关".into());
    }

    let candidates = candidate_origins_for(config.zone);
    let need_captive_sniff = matches!(
        config.zone,
        CampusZone::Auto | CampusZone::Wangcheng | CampusZone::Yanshan
    );
    let observed_origin = captive_origin.and_then(validate_captive_target_url);
    discover_best_gateway(
        &candidates,
        config.zone,
        need_captive_sniff,
        observed_origin.as_deref(),
    )
    .ok_or_else(|| "未探测到可达的师大校园网认证网关（请确认已连入 GXNU 校园网）".into())
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
        .redirects(0)
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
    observed_origin: Option<&str>,
) -> Option<(ResolvedPortal, u64, CampusStatusInfo)> {
    let observed_origin = observed_origin.filter(|_| with_captive_sniff);
    let mut results: Vec<(usize, ResolvedPortal, u64, CampusStatusInfo)> = Vec::new();

    std::thread::scope(|scope| {
        let mut handles = Vec::new();

        for (idx, &origin) in candidates.iter().enumerate() {
            let rank = if observed_origin == Some(origin) {
                0
            } else {
                idx + 1
            };
            let origin_owned = origin.to_string();
            handles.push(scope.spawn(move || {
                let agent = fast_portal_agent(2);
                let hint = if zone == CampusZone::Auto {
                    None
                } else {
                    Some(zone)
                };
                let portal = ResolvedPortal::from_origin(origin_owned, hint);
                probe_chkstatus_once(&agent, &portal).map(|(ms, info)| (rank, portal, ms, info))
            }));
        }

        let sniff_handle = if with_captive_sniff
            && !observed_origin.is_some_and(|origin| candidates.contains(&origin))
        {
            Some(scope.spawn(move || {
                let sniffed_origin = observed_origin
                    .map(str::to_owned)
                    .or_else(sniff_captive_redirect_origin)?;
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

    select_verified_gateway(results)
}

fn select_verified_gateway(
    mut results: Vec<(usize, ResolvedPortal, u64, CampusStatusInfo)>,
) -> Option<(ResolvedPortal, u64, CampusStatusInfo)> {
    // 跳转线索可以提升同一网关的优先级。若该网关支持 HTTPS，优先采用其 TLS 状态证据。
    let https_alias =
        results
            .iter()
            .find(|(rank, _, _, _)| *rank == 0)
            .and_then(|(_, observed, _, _)| {
                results
                    .iter()
                    .position(|(_, portal, _, _)| portal.is_https_alternative_to(&observed.origin))
            });
    if let Some(index) = https_alias {
        results[index].0 = 0;
    }
    results
        .into_iter()
        .min_by_key(|(rank, portal, ms, _)| (*rank, !portal.origin.starts_with("https://"), *ms))
        .map(|(_, portal, ms, info)| (portal, ms, info))
}

/// 发起一次不跟随重定向的明文 HTTP 探测，若被师大校园网 AC 302 跳转或注入跳转脚本，提取并校验网关源。
fn sniff_captive_redirect_origin() -> Option<String> {
    let agent = fast_no_redirect_agent();
    let resp = http::get_response(&agent, http::CONNECT_TEST_URL).ok()?;
    let origin = extract_captive_portal_origin(resp.header("Location"), "");
    if origin.is_some() || resp.status() != 200 {
        http::drain_small_body(resp);
        return origin;
    }
    let mut prefix = [0; CAPTIVE_BODY_LIMIT];
    let (len, error) = http::read_prefix_with_error(resp.into_reader(), &mut prefix);
    captive_origin_from_prefix(&prefix[..len], error.is_some() || len == prefix.len())
}

pub(crate) fn captive_origin_from_prefix(prefix: &[u8], incomplete: bool) -> Option<String> {
    let text = String::from_utf8_lossy(prefix);
    let body = if incomplete {
        // 截断处可能落在主机名或端口中间，仅解析已读到终止符的 URL。
        &text[..=text.rfind(captive_url_delimiter)?]
    } else {
        text.as_ref()
    };
    extract_captive_portal_origin(None, body)
}

fn captive_url_delimiter(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\'' | '>' | '<' | ' ' | '\r' | '\n' | '\t' | ';' | ')'
    )
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
    if trimmed.is_empty() || trimmed.contains(http::CONNECT_TEST_BODY) {
        return None;
    }

    // 解析常见的 Dr.COM 强制门户 HTML 注入跳转（如 `location.href="http://192.168.125.21/..."` 或 `url=http://...`）
    for prefix in ["http://", "https://"] {
        let mut rest = trimmed;
        while let Some(pos) = rest.find(prefix) {
            let sub = &rest[pos..];
            let end = sub.find(captive_url_delimiter).unwrap_or(sub.len());
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
    fn incomplete_captive_url_is_not_used_as_a_candidate() {
        let fragment = b"<script>location.href='http://192.168.125.21";
        assert!(captive_origin_from_prefix(fragment, true).is_none());
    }

    #[test]
    fn plaintext_auth_rejects_a_redirecting_chkstatus_even_with_a_cached_gateway() {
        use std::sync::Arc;

        for (had_cache, current) in [(false, true), (true, true), (false, false)] {
            let destination = TcpListener::bind("127.0.0.1:0").unwrap();
            destination.set_nonblocking(true).unwrap();
            let location = destination.local_addr().unwrap();
            let (origin, server) = http::mock::serve(move |stream| {
                write!(
                    stream,
                    "HTTP/1.1 302 Found\r\nLocation: http://{location}/status\r\nContent-Length: 0\r\n\r\n"
                )
                .unwrap();
            });
            let resolver = Arc::new(PortalGatewayResolver::new(CampusZone::Custom, &origin));
            if had_cache {
                resolver.state.lock().unwrap().cached = Some(ResolvedPortal::from_origin(
                    origin,
                    Some(CampusZone::Custom),
                ));
            }
            let auth = crate::authenticator::CampusAuthenticator::with_resolver(resolver);
            let result =
                auth.authenticate_if("fixture-user", "fixture-value", "2", "192.0.2.1", || {
                    current
                });
            server.join().unwrap();
            if current {
                assert_eq!(
                    result.unwrap().reason,
                    crate::models::DisconnectReason::PortalUnavailable
                );
            } else {
                assert!(
                    result.is_none(),
                    "cancelled lookup must not count as an auth result"
                );
            }
            assert_eq!(
                destination.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }

    #[test]
    fn verified_https_alias_keeps_precedence_over_plaintext_redirect() {
        let verified = |rank, origin, online| {
            (
                rank,
                ResolvedPortal::from_origin(origin, None),
                1,
                CampusStatusInfo {
                    online,
                    portal_ip: None,
                    gateway_label: None,
                },
            )
        };
        for plaintext in [
            YUCAI_IP_ORIGIN,
            "http://yc.gxnu.edu.cn",
            "http://yc.gxnu.edu.cn.",
            "http://192.168.125.13:801",
        ] {
            let (portal, _, status) = select_verified_gateway(vec![
                verified(0, plaintext, false),
                verified(1, YUCAI_HTTPS_ORIGIN, true),
            ])
            .unwrap();
            assert_eq!(portal.origin, YUCAI_HTTPS_ORIGIN);
            assert!(status.online);
            assert!(portal.is_https_alternative_to(plaintext));
        }
        let (portal, _, _) = select_verified_gateway(vec![
            verified(0, YANSHAN_IP_ORIGIN, false),
            verified(1, YUCAI_HTTPS_ORIGIN, true),
        ])
        .unwrap();
        assert_eq!(portal.origin, YANSHAN_IP_ORIGIN);
    }

    #[test]
    fn outdated_discovery_cannot_restore_an_invalidated_gateway() {
        use std::sync::{mpsc, Arc};

        for change_config in [false, true] {
            let (started, waiting) = mpsc::channel();
            let (release, gate) = mpsc::channel();
            let (old_origin, old_server) = http::mock::serve(move |stream| {
                started.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(4)).unwrap();
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n{\"result\":0}")
                    .unwrap();
            });
            let resolver = Arc::new(PortalGatewayResolver::new(CampusZone::Custom, &old_origin));
            let pending = Arc::clone(&resolver);
            let discovery = std::thread::spawn(move || pending.probe_and_cache());
            waiting.recv_timeout(Duration::from_secs(3)).unwrap();
            let current_origin = if change_config {
                let (current, server) = http::mock::serve(|stream| {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n{\"result\":1}")
                        .unwrap();
                });
                assert!(resolver.update_config(CampusZone::Custom, &current));
                assert_eq!(resolver.probe_and_cache().unwrap().0.origin, current);
                server.join().unwrap();
                Some(current)
            } else {
                resolver.invalidate_cache();
                None
            };
            release.send(()).unwrap();
            old_server.join().unwrap();
            assert_eq!(discovery.join().unwrap().unwrap_err(), STALE_GATEWAY);
            assert_eq!(
                resolver.cached_endpoints().map(|portal| portal.origin),
                current_origin
            );
        }
    }

    #[test]
    fn a_verified_observed_origin_takes_priority_over_default_candidates() {
        let status_response = |body: &'static str| {
            http::mock::serve(move |stream| {
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            })
        };
        let (default_origin, default_server) = status_response(r#"{"result":1}"#);
        let (observed_origin, observed_server) = status_response(r#"{"result":0}"#);
        let result = discover_best_gateway(
            &[&default_origin, &observed_origin],
            CampusZone::Auto,
            true,
            Some(&observed_origin),
        );
        default_server.join().unwrap();
        observed_server.join().unwrap();
        let (portal, _, status) = result.unwrap();
        assert_eq!(portal.origin, observed_origin);
        assert!(!status.online);
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
            None,
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
