//! GitHub Release 自动更新器：
//! - **零 API 限流版本发现**：优先走 `/releases/latest/download/latest.json` 与 `/releases/latest` 302 重定向，
//!   规避校园网共用公网 NAT IP 下的 GitHub REST API 60 次/小时匿名限流；兼容无 `latest.json` 的手动发版回退。
//! - **代理感知 + 直连回退**：自动读取环境变量或 Windows 系统代理（Clash / Mihomo），失败时自动回退直连。
//! - **流式 SHA-256 与 PE 校验**：内置纯 Rust FIPS 180-4 SHA-256 校验器，拒绝不完整或非 PE 文件。
//! - **Windows Rename-Swap 原位无感覆盖**：利用 NTFS 允许对运行中 `.exe` 改名的特性，将当前程序改名为 `.old` 后写入新版并交接重启；
//!   若位于 `Program Files` 等受保护目录则自动降级为单次 UAC `runas` 提权覆盖。

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::logging;
use crate::models::{ReleaseInfo, UpdateState, UpdateStatus};

pub const CURRENT_VERSION: &str = match option_env!("GXNU_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub const GITHUB_REPO_WEB: &str = "https://github.com/junkaih2019/gxnu-campus-net-helper";
pub const GITHUB_RELEASES_PAGE: &str =
    "https://github.com/junkaih2019/gxnu-campus-net-helper/releases";
const GITHUB_API_BASE: &str = "https://api.github.com/repos/junkaih2019/gxnu-campus-net-helper";

/// 后台静默自动检查周期：12 小时。
const AUTO_CHECK_INTERVAL_SECS: i64 = 12 * 3600;
/// 后台自动检查遇到网络失败后的最小重试间隔：2 小时（避免离线或抽风时反复请求）。
const AUTO_RETRY_ON_ERROR_SECS: i64 = 2 * 3600;
/// 守护进程首次联网稳定后的延迟检查秒数。
const INITIAL_ONLINE_DELAY: Duration = Duration::from_secs(20);
/// 合法 Windows 发行版 `.exe` 的最小/最大体积保护阈值（512 KB ~ 80 MB）。
const MIN_EXE_BYTES: u64 = 512 * 1024;
const MAX_EXE_BYTES: u64 = 80 * 1024 * 1024;
const UPDATE_CACHE_FILE: &str = "update_cache.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedUpdateMeta {
    #[serde(default)]
    last_checked_unix: Option<i64>,
    #[serde(default)]
    cached_release: Option<ReleaseInfo>,
}

#[derive(Debug, Deserialize)]
struct LatestManifest {
    version: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    asset_name: String,
    download_url: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    notes: String,
}

#[derive(Debug, Deserialize)]
struct GithubApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    /// GitHub Releases API 返回的 `"sha256:<hex>"` 字段。
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubApiRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubApiAsset>,
}

// ── 版本号解析与比较（支持 3 段 `0.2.3` 与 4 段小版本号 `0.2.2.2`）────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedVersion {
    nums: Vec<u32>,
    prerelease: Option<String>,
}

fn parse_version(raw: &str) -> Option<ParsedVersion> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']).trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_build = trimmed.split('+').next().unwrap_or(trimmed);
    let (core, prerelease) = match without_build.split_once('-') {
        Some((c, pre)) => (
            c.trim(),
            Some(pre.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()),
        ),
        None => (without_build.trim(), None),
    };
    let mut nums = Vec::new();
    for part in core.split('.') {
        let n = part.trim().parse::<u32>().ok()?;
        nums.push(n);
    }
    if nums.is_empty() {
        return None;
    }
    while nums.len() > 1 && nums.last() == Some(&0) {
        nums.pop();
    }
    Some(ParsedVersion { nums, prerelease })
}

pub fn normalize_version_str(raw: &str) -> String {
    raw.trim().trim_start_matches(['v', 'V']).trim().to_string()
}

/// 比较两个版本号（支持 3 段与 4 段语义版本，正式版高于同数字预发布版）。
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    let va = parse_version(a)?;
    let vb = parse_version(b)?;
    let max_len = va.nums.len().max(vb.nums.len());
    for i in 0..max_len {
        let na = va.nums.get(i).copied().unwrap_or(0);
        let nb = vb.nums.get(i).copied().unwrap_or(0);
        match na.cmp(&nb) {
            Ordering::Equal => continue,
            non_eq => return Some(non_eq),
        }
    }
    match (&va.prerelease, &vb.prerelease) {
        (None, None) => Some(Ordering::Equal),
        (None, Some(_)) => Some(Ordering::Greater),
        (Some(_), None) => Some(Ordering::Less),
        (Some(pa), Some(pb)) => Some(pa.cmp(pb)),
    }
}

/// 判断 `candidate` 是否严格新于 `current`，且不是预发布测试版。
pub fn is_newer_stable_version(current: &str, candidate: &str) -> bool {
    let Some(cand_parsed) = parse_version(candidate) else {
        return false;
    };
    if cand_parsed.prerelease.is_some() {
        return false;
    }
    compare_versions(candidate, current) == Some(Ordering::Greater)
}

// ── 纯 Rust 流式 SHA-256（FIPS 180-4，零外部依赖、零 unsafe）───────────────────

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_bytes: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0u8; 64],
            buf_len: 0,
            total_bytes: 0,
        }
    }

    fn update(&mut self, mut input: &[u8]) {
        self.total_bytes = self.total_bytes.wrapping_add(input.len() as u64);
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            if input.len() < need {
                self.buf[self.buf_len..self.buf_len + input.len()].copy_from_slice(input);
                self.buf_len += input.len();
                return;
            }
            self.buf[self.buf_len..64].copy_from_slice(&input[..need]);
            let block = self.buf;
            self.compress(&block);
            self.buf_len = 0;
            input = &input[need..];
        }
        while input.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&input[..64]);
            self.compress(&block);
            input = &input[64..];
        }
        if !input.is_empty() {
            self.buf[..input.len()].copy_from_slice(input);
            self.buf_len = input.len();
        }
    }

    fn finalize_hex(mut self) -> String {
        let total_bits = self.total_bytes.wrapping_mul(8);
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > 56 {
            for b in &mut self.buf[self.buf_len..64] {
                *b = 0;
            }
            let block = self.buf;
            self.compress(&block);
            self.buf_len = 0;
        }
        for b in &mut self.buf[self.buf_len..56] {
            *b = 0;
        }
        self.buf[56..64].copy_from_slice(&total_bits.to_be_bytes());
        let final_block = self.buf;
        self.compress(&final_block);

        let mut out = String::with_capacity(64);
        for word in self.state {
            out.push_str(&format!("{word:08x}"));
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, &chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(chunk);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize_hex()
}

fn normalize_sha256(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    let hex = s
        .strip_prefix("sha256:")
        .or_else(|| s.strip_prefix("SHA256:"))
        .unwrap_or(s)
        .trim();
    if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(hex.to_ascii_lowercase())
    } else {
        None
    }
}

// ── 网络客户端构建（支持 Windows 系统代理 + 失败自动回退直连）──────────────────

fn read_windows_system_proxy() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    let enabled: u32 = key.get_value("ProxyEnable").ok()?;
    if enabled == 0 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    let trimmed = server.trim();
    if trimmed.is_empty() {
        return None;
    }
    let picked = if trimmed.contains('=') {
        trimmed
            .split(';')
            .find_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                let k = k.trim().to_ascii_lowercase();
                (k == "https" || k == "http").then_some(v.trim())
            })
            .unwrap_or("")
    } else {
        trimmed
    };
    if picked.is_empty() {
        return None;
    }
    if picked.contains("://") {
        Some(picked.to_string())
    } else {
        Some(format!("http://{picked}"))
    }
}

fn build_update_agents(connect_secs: u64, read_secs: u64, redirects: u32) -> Vec<ureq::Agent> {
    let mut agents = Vec::new();
    if let Some(sys_proxy) = read_windows_system_proxy() {
        if let Ok(proxy) = ureq::Proxy::new(&sys_proxy) {
            agents.push(
                ureq::AgentBuilder::new()
                    .proxy(proxy)
                    .timeout_connect(Duration::from_secs(connect_secs))
                    .timeout_read(Duration::from_secs(read_secs))
                    .redirects(redirects)
                    .build(),
            );
        }
    } else {
        agents.push(
            ureq::AgentBuilder::new()
                .try_proxy_from_env(true)
                .timeout_connect(Duration::from_secs(connect_secs))
                .timeout_read(Duration::from_secs(read_secs))
                .redirects(redirects)
                .build(),
        );
    }
    agents.push(
        ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .timeout_connect(Duration::from_secs(connect_secs))
            .timeout_read(Duration::from_secs(read_secs))
            .redirects(redirects)
            .build(),
    );
    agents
}

fn user_agent_header() -> String {
    format!("gxnu-campus-net-helper/{CURRENT_VERSION}")
}

fn is_trusted_github_url(raw: &str, allow_local_http: bool) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    if allow_local_http && url.scheme() == "http" && url.host_str() == Some("127.0.0.1") {
        return true;
    }
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str().map(|h| h.to_ascii_lowercase()) else {
        return false;
    };
    host == "github.com"
        || host == "api.github.com"
        || host == "objects.githubusercontent.com"
        || host == "release-assets.githubusercontent.com"
        || host.ends_with(".githubusercontent.com")
}

// ── GitHub Release 检查逻辑 ──────────────────────────────────────────────────

pub fn check_for_updates(current_version: &str) -> Result<Option<ReleaseInfo>, String> {
    check_for_updates_with_bases(current_version, GITHUB_REPO_WEB, GITHUB_API_BASE, false)
}

fn check_for_updates_with_bases(
    current_version: &str,
    web_base: &str,
    api_base: &str,
    allow_local_http: bool,
) -> Result<Option<ReleaseInfo>, String> {
    let follow_agents = build_update_agents(8, 12, 5);
    let no_redir_agents = build_update_agents(8, 10, 0);

    // 1. 优先尝试静态 `latest.json`（走下载通道，完全无 60 次/小时 REST API 限流，且自带 SHA-256）
    let manifest_url = format!("{web_base}/releases/latest/download/latest.json");
    let mut saw_network_ok = false;
    for agent in &follow_agents {
        match agent
            .get(&manifest_url)
            .set("User-Agent", &user_agent_header())
            .call()
        {
            Ok(resp) if resp.status() == 200 => {
                saw_network_ok = true;
                if let Ok(body) = resp.into_string() {
                    if let Ok(manifest) = serde_json::from_str::<LatestManifest>(&body) {
                        if let Some(info) = release_from_manifest(
                            current_version,
                            manifest,
                            web_base,
                            allow_local_http,
                        ) {
                            return Ok(Some(info));
                        }
                        let ver = normalize_version_str(&body_version_hint(&body));
                        if !ver.is_empty() && !is_newer_stable_version(current_version, &ver) {
                            return Ok(None);
                        }
                    }
                }
                break;
            }
            Err(ureq::Error::Status(404, _)) => {
                // 仓库发版未附带 latest.json，继续走 302 探测 + API 回退
                saw_network_ok = true;
                break;
            }
            Err(ureq::Error::Status(_, _)) => {
                break;
            }
            Err(ureq::Error::Transport(_)) => continue,
            Ok(_) => break,
        }
    }

    // 2. 请求 `/releases/latest` 的 302 Location 头（零 API 配额消耗即可判断是否有新版本）
    let latest_page_url = format!("{web_base}/releases/latest");
    for agent in &no_redir_agents {
        match agent
            .get(&latest_page_url)
            .set("User-Agent", &user_agent_header())
            .call()
        {
            Ok(resp) if matches!(resp.status(), 301 | 302 | 303 | 307 | 308) => {
                saw_network_ok = true;
                if let Some(loc) = resp.header("Location") {
                    if let Some(tag) = extract_tag_from_location(loc) {
                        if !is_newer_stable_version(current_version, &tag) {
                            return Ok(None);
                        }
                    }
                }
                break;
            }
            Err(ureq::Error::Status(404, _)) => {
                // 仓库尚无任何 Release
                return Ok(None);
            }
            Err(ureq::Error::Transport(_)) => continue,
            _ => break,
        }
    }

    // 3. 仅当确有新版（或未拿到 302）时，调用 GitHub Releases API 拉取 `.exe` 下载链接与更新说明
    let api_url = format!("{api_base}/releases/latest");
    let mut last_err = String::new();
    for agent in &follow_agents {
        match agent
            .get(&api_url)
            .set("User-Agent", &user_agent_header())
            .set("Accept", "application/vnd.github+json")
            .call()
        {
            Ok(resp) if resp.status() == 200 => {
                let body = resp
                    .into_string()
                    .map_err(|_| "读取 GitHub Release 响应失败".to_string())?;
                let parsed: GithubApiRelease = serde_json::from_str(&body)
                    .map_err(|_| "解析 GitHub Release 元数据失败".to_string())?;
                if parsed.draft || parsed.prerelease {
                    return Ok(None);
                }
                if !is_newer_stable_version(current_version, &parsed.tag_name) {
                    return Ok(None);
                }
                let Some(exe_asset) = parsed
                    .assets
                    .iter()
                    .find(|a| a.name.to_ascii_lowercase().ends_with(".exe"))
                else {
                    return Err("最新 Release 中未找到可下载的 .exe 安装包".to_string());
                };
                if !is_trusted_github_url(&exe_asset.browser_download_url, allow_local_http) {
                    return Err("Release 下载地址不在受信任的 GitHub 域名下".to_string());
                }
                let html_url = if is_trusted_github_url(&parsed.html_url, allow_local_http) {
                    parsed.html_url
                } else {
                    format!("{GITHUB_REPO_WEB}/releases/tag/{}", parsed.tag_name)
                };
                return Ok(Some(ReleaseInfo {
                    version: normalize_version_str(&parsed.tag_name),
                    tag: parsed.tag_name,
                    asset_name: exe_asset.name.clone(),
                    download_url: exe_asset.browser_download_url.clone(),
                    html_url,
                    sha256: normalize_sha256(exe_asset.digest.as_deref()),
                    size_bytes: (exe_asset.size > 0).then_some(exe_asset.size),
                    published_at: parsed.published_at.unwrap_or_default(),
                    notes: parsed.body.unwrap_or_default().trim().to_string(),
                }));
            }
            Err(ureq::Error::Status(404, _)) => return Ok(None),
            Err(ureq::Error::Status(403, _)) => {
                last_err = "GitHub 接口访问受限（校园网共享 IP 触发限流），请稍后重试或前往 GitHub 页面下载"
                    .to_string();
                break;
            }
            Err(ureq::Error::Status(code, _)) => {
                last_err = format!("GitHub 返回 HTTP {code}");
                break;
            }
            Err(ureq::Error::Transport(_)) => {
                last_err = if saw_network_ok {
                    "连接 GitHub Release 接口超时，请稍后重试".to_string()
                } else {
                    "无法连接 GitHub（校园网直连可能受阻），可开启代理后重试或打开浏览器下载"
                        .to_string()
                };
                continue;
            }
            Ok(resp) => {
                last_err = format!("GitHub 返回非预期状态 HTTP {}", resp.status());
                break;
            }
        }
    }

    Err(if last_err.is_empty() {
        "检查更新失败，请检查网络连接".to_string()
    } else {
        last_err
    })
}

fn body_version_hint(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("version")
                .or_else(|| v.get("tag"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default()
}

fn release_from_manifest(
    current_version: &str,
    m: LatestManifest,
    web_base: &str,
    allow_local_http: bool,
) -> Option<ReleaseInfo> {
    let ver_cand = if !m.version.trim().is_empty() {
        m.version.trim()
    } else {
        m.tag.trim()
    };
    if !is_newer_stable_version(current_version, ver_cand) {
        return None;
    }
    if !is_trusted_github_url(&m.download_url, allow_local_http) {
        return None;
    }
    let clean_ver = normalize_version_str(ver_cand);
    let tag = if m.tag.trim().is_empty() {
        format!("v{clean_ver}")
    } else {
        m.tag.trim().to_string()
    };
    let html_url = if is_trusted_github_url(&m.html_url, allow_local_http) {
        m.html_url
    } else {
        format!("{web_base}/releases/tag/{tag}")
    };
    let asset_name = if m.asset_name.trim().is_empty() {
        format!("gxnu-campus-net-helper-{tag}.exe")
    } else {
        m.asset_name.trim().to_string()
    };
    Some(ReleaseInfo {
        version: clean_ver,
        tag,
        asset_name,
        download_url: m.download_url,
        html_url,
        sha256: normalize_sha256(m.sha256.as_deref()),
        size_bytes: m.size.filter(|&n| n > 0),
        published_at: m.published_at,
        notes: m.notes.trim().to_string(),
    })
}

fn extract_tag_from_location(location: &str) -> Option<String> {
    let marker = "/releases/tag/";
    let idx = location.find(marker)?;
    let rest = &location[idx + marker.len()..];
    let tag = rest.split(['?', '#', '/']).next().unwrap_or("").trim();
    (!tag.is_empty()).then(|| tag.to_string())
}

// ── 流式下载、校验与原位 Rename-Swap 覆盖安装 ──────────────────────────────────

fn stream_download_and_verify<F>(
    release: &ReleaseInfo,
    dest_file_path: &Path,
    allow_local_http: bool,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, Option<u64>),
{
    if !is_trusted_github_url(&release.download_url, allow_local_http) {
        return Err("更新包地址未通过安全域名校验".to_string());
    }

    let agents = build_update_agents(12, 45, 6);
    let mut response_opt = None;
    let mut last_err = String::new();
    for agent in &agents {
        match agent
            .get(&release.download_url)
            .set("User-Agent", &user_agent_header())
            .call()
        {
            Ok(resp) if resp.status() == 200 => {
                response_opt = Some(resp);
                break;
            }
            Ok(resp) => {
                last_err = format!("下载服务器返回 HTTP {}", resp.status());
            }
            Err(ureq::Error::Status(code, _)) => {
                last_err = format!("下载更新包失败（HTTP {code}）");
            }
            Err(ureq::Error::Transport(_)) => {
                last_err = "下载连接中断或超时，请重试或点击浏览器手动下载".to_string();
                continue;
            }
        }
    }
    let Some(resp) = response_opt else {
        return Err(if last_err.is_empty() {
            "无法连接 GitHub 下载更新包".to_string()
        } else {
            last_err
        });
    };

    let header_len = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0);
    let expected_total = header_len.or(release.size_bytes);
    if let Some(total) = expected_total {
        if !(MIN_EXE_BYTES..=MAX_EXE_BYTES).contains(&total) {
            return Err(format!("更新包大小异常（{total} 字节），已中止下载"));
        }
    }

    let mut out = File::create(dest_file_path).map_err(|e| format!("创建临时更新文件失败：{e}"))?;
    let mut reader = resp.into_reader();
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 32 * 1024];
    let mut downloaded: u64 = 0;
    let mut magic = [0u8; 2];
    let mut last_emit = Instant::now();

    on_progress(0, expected_total);

    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            let _ = fs::remove_file(dest_file_path);
            format!("下载过程中连接中断：{e}")
        })?;
        if n == 0 {
            break;
        }
        if downloaded < 2 {
            for (i, &b) in buf[..n].iter().enumerate() {
                let pos = (downloaded as usize) + i;
                if pos < 2 {
                    magic[pos] = b;
                }
            }
        }
        downloaded = downloaded.saturating_add(n as u64);
        if downloaded > MAX_EXE_BYTES {
            drop(out);
            let _ = fs::remove_file(dest_file_path);
            return Err("更新包体积超出安全上限，已中止下载".to_string());
        }
        hasher.update(&buf[..n]);
        if let Err(e) = out.write_all(&buf[..n]) {
            drop(out);
            let _ = fs::remove_file(dest_file_path);
            return Err(format!("写入更新文件失败：{e}"));
        }
        if last_emit.elapsed() >= Duration::from_millis(100) {
            last_emit = Instant::now();
            on_progress(downloaded, expected_total);
        }
    }

    if let Err(e) = out.sync_all() {
        drop(out);
        let _ = fs::remove_file(dest_file_path);
        return Err(format!("刷写磁盘失败：{e}"));
    }
    drop(out);

    if downloaded < MIN_EXE_BYTES {
        let _ = fs::remove_file(dest_file_path);
        return Err("下载的文件不完整（体积过小），已丢弃".to_string());
    }
    if let Some(total) = expected_total {
        if downloaded != total {
            let _ = fs::remove_file(dest_file_path);
            return Err(format!(
                "下载未完成（已收 {downloaded} / 预期 {total} 字节），请重试"
            ));
        }
    }
    if magic != *b"MZ" {
        let _ = fs::remove_file(dest_file_path);
        return Err("下载的文件不是有效的 Windows 可执行程序（缺少 MZ 头）".to_string());
    }

    let actual_sha = hasher.finalize_hex();
    if let Some(expected_sha) = &release.sha256 {
        if !actual_sha.eq_ignore_ascii_case(expected_sha) {
            let _ = fs::remove_file(dest_file_path);
            return Err("更新包 SHA-256 完整性校验未通过，已拦截损坏文件".to_string());
        }
    }

    on_progress(downloaded, Some(downloaded));
    Ok(())
}

fn sibling_path_with_suffix(exe: &Path, suffix: &str) -> PathBuf {
    let mut os = exe.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

/// 将已校验的新版 `.new` 原子替换到 `target_exe`（先将运行中的 `target_exe` 改名为 `.old`）。
/// 若第二步改名失败则立即将 `.old` 改回 `target_exe` 无损回滚。
pub fn atomic_swap_executable(target_exe: &Path, staged_new_exe: &Path) -> Result<PathBuf, String> {
    let old_exe = sibling_path_with_suffix(target_exe, ".old");
    if old_exe.exists() {
        let _ = fs::remove_file(&old_exe);
    }
    if target_exe.exists() {
        fs::rename(target_exe, &old_exe)
            .map_err(|e| format!("无法重命名当前程序以腾出更新位置：{e}"))?;
    }
    if let Err(err) = fs::rename(staged_new_exe, target_exe) {
        if old_exe.exists() {
            let _ = fs::rename(&old_exe, target_exe);
        }
        return Err(format!("写入新版可执行文件失败（已回滚原程序）：{err}"));
    }
    Ok(old_exe)
}

/// 启动时静默清理上次原位自更新遗留的 `<current_exe>.old` 与临时目录安装包。
pub fn cleanup_old_artifacts(app_dir: &Path) {
    if let Ok(exe) = std::env::current_exe() {
        let old_exe = sibling_path_with_suffix(&exe, ".old");
        if old_exe.exists() {
            let _ = fs::remove_file(&old_exe);
        }
        let new_exe = sibling_path_with_suffix(&exe, ".new");
        if new_exe.exists() {
            let _ = fs::remove_file(&new_exe);
        }
    }
    let stage_dir = app_dir.join("updates");
    if stage_dir.exists() {
        let _ = fs::remove_dir_all(&stage_dir);
    }
}

/// 等待指定旧进程退出（最多等待 `timeout`，用于 `--after-update` / `--apply-update` 平滑交接互斥体）。
pub fn wait_for_pid_exit(pid: u32, timeout: Duration) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    if pid == 0 || pid == std::process::id() {
        return;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !handle.is_null() {
            let _ = WaitForSingleObject(handle, timeout.as_millis() as u32);
            CloseHandle(handle);
        }
    }
}

/// 受保护目录（如 `C:\Program Files`）下由 UAC 提权拉起的 `--apply-update <target_exe> <old_pid>` 处理器。
pub fn run_elevated_apply_update(target_exe: &Path, wait_pid: u32) -> Result<(), String> {
    wait_for_pid_exit(wait_pid, Duration::from_secs(12));
    let self_exe = std::env::current_exe().map_err(|e| format!("无法获取更新器路径：{e}"))?;
    let same_dir_new = sibling_path_with_suffix(target_exe, ".new");
    fs::copy(&self_exe, &same_dir_new).map_err(|e| format!("复制更新包到目标目录失败：{e}"))?;
    atomic_swap_executable(target_exe, &same_dir_new)?;
    let _ = std::process::Command::new(target_exe)
        .args(["--after-update", "0"])
        .spawn()
        .map_err(|e| format!("启动更新后的程序失败：{e}"))?;
    Ok(())
}

/// 使用 Windows Shell 打开受信任的 HTTPS 网页链接（无控制台黑框闪现）。
pub fn open_browser_url(url: &str) {
    use std::ptr::null_mut;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    if !is_trusted_github_url(url, false) {
        return;
    }
    let op: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            null_mut(),
            op.as_ptr(),
            target.as_ptr(),
            null_mut(),
            null_mut(),
            SW_SHOWNORMAL,
        );
    }
}

fn launch_uac_apply_update(
    staged_exe: &Path,
    target_exe: &Path,
    daemon_pid: u32,
) -> Result<(), String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let file: Vec<u16> = staged_exe
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let params_str = format!("--apply-update \"{}\" {}", target_exe.display(), daemon_pid);
    let params: Vec<u16> = params_str
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let ret = unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            null_mut(),
            SW_SHOWNORMAL,
        )
    };
    if (ret as isize) <= 32 {
        Err("未获得管理员授权，已取消覆盖受保护目录中的程序".to_string())
    } else {
        Ok(())
    }
}

// ── 守护进程侧低开销自动更新调度器 ───────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkerTask {
    Check { manual: bool },
    Install,
}

struct ManagerInner {
    auto_enabled: bool,
    skipped_version: String,
    state: UpdateState,
    pending_task: Option<WorkerTask>,
    busy: bool,
    stopped: bool,
    last_error_auto_unix: Option<i64>,
}

type StateSink = Arc<dyn Fn(UpdateState) + Send + Sync>;
type QuitTrigger = Arc<dyn Fn() + Send + Sync>;

pub struct UpdateManager {
    app_dir: PathBuf,
    inner: Arc<(Mutex<ManagerInner>, Condvar)>,
}

impl UpdateManager {
    pub fn new(
        app_dir: PathBuf,
        auto_enabled: bool,
        skipped_version: String,
        is_online: Arc<AtomicBool>,
        on_state: StateSink,
        on_quit_for_restart: QuitTrigger,
    ) -> Self {
        let meta = load_persisted_meta(&app_dir);
        let initial_release = meta
            .cached_release
            .filter(|r| is_newer_stable_version(CURRENT_VERSION, &r.version));
        let initial_status = if initial_release.is_some() {
            UpdateStatus::Available
        } else {
            UpdateStatus::Idle
        };
        let initial_state = UpdateState {
            status: initial_status,
            current_version: CURRENT_VERSION.to_string(),
            release: initial_release,
            downloaded_bytes: 0,
            total_bytes: None,
            last_checked_unix: meta.last_checked_unix,
            message: String::new(),
        };

        let pair = Arc::new((
            Mutex::new(ManagerInner {
                auto_enabled,
                skipped_version,
                state: initial_state,
                pending_task: None,
                busy: false,
                stopped: false,
                last_error_auto_unix: None,
            }),
            Condvar::new(),
        ));

        let worker_pair = Arc::clone(&pair);
        let worker_dir = app_dir.clone();
        let _ = std::thread::Builder::new()
            .name("auto-updater".to_string())
            .spawn(move || {
                run_updater_loop(
                    worker_dir,
                    worker_pair,
                    is_online,
                    on_state,
                    on_quit_for_restart,
                );
            });

        Self {
            app_dir,
            inner: pair,
        }
    }

    pub fn current_state(&self) -> UpdateState {
        self.inner.0.lock().unwrap().state.clone()
    }

    pub fn sync_preferences(&self, auto_enabled: bool, skipped_version: &str) {
        let (lock, cv) = &*self.inner;
        let mut g = lock.lock().unwrap();
        let became_enabled = !g.auto_enabled && auto_enabled;
        g.auto_enabled = auto_enabled;
        g.skipped_version = normalize_version_str(skipped_version);
        if became_enabled {
            cv.notify_all();
        }
    }

    pub fn request_check(&self, manual: bool) {
        let (lock, cv) = &*self.inner;
        let mut g = lock.lock().unwrap();
        if g.busy {
            return;
        }
        g.pending_task = Some(WorkerTask::Check { manual });
        cv.notify_all();
    }

    pub fn request_install(&self) -> Result<(), String> {
        let (lock, cv) = &*self.inner;
        let mut g = lock.lock().unwrap();
        if g.busy {
            return Err("更新任务正在执行中，请稍候".to_string());
        }
        if g.state.release.is_none() {
            return Err("当前没有可安装的新版本，请先检查更新".to_string());
        }
        g.pending_task = Some(WorkerTask::Install);
        cv.notify_all();
        Ok(())
    }

    pub fn shutdown(&self) {
        let (lock, cv) = &*self.inner;
        if let Ok(mut g) = lock.lock() {
            g.stopped = true;
            cv.notify_all();
        }
        let _ = &self.app_dir;
    }
}

fn run_updater_loop(
    app_dir: PathBuf,
    pair: Arc<(Mutex<ManagerInner>, Condvar)>,
    is_online: Arc<AtomicBool>,
    on_state: StateSink,
    on_quit_for_restart: QuitTrigger,
) {
    let boot_instant = Instant::now();
    let (lock, cv) = &*pair;

    loop {
        let task = {
            let mut g = lock.lock().unwrap();
            if g.stopped {
                break;
            }
            if let Some(t) = g.pending_task.take() {
                g.busy = true;
                Some(t)
            } else {
                let now = Local::now().timestamp();
                let online = is_online.load(AtomicOrdering::Acquire);
                let ready_after_boot = boot_instant.elapsed() >= INITIAL_ONLINE_DELAY;
                let due_by_time = g
                    .state
                    .last_checked_unix
                    .is_none_or(|last| now - last >= AUTO_CHECK_INTERVAL_SECS);
                let error_cooled_down = g
                    .last_error_auto_unix
                    .is_none_or(|err_ts| now - err_ts >= AUTO_RETRY_ON_ERROR_SECS);

                if g.auto_enabled && online && ready_after_boot && due_by_time && error_cooled_down
                {
                    g.busy = true;
                    Some(WorkerTask::Check { manual: false })
                } else {
                    let wait_dur = if g.auto_enabled && online && !ready_after_boot {
                        INITIAL_ONLINE_DELAY.saturating_sub(boot_instant.elapsed())
                    } else {
                        Duration::from_secs(600)
                    };
                    let (g_after, _) = cv.wait_timeout(g, wait_dur).unwrap();
                    if g_after.stopped {
                        break;
                    }
                    None
                }
            }
        };

        let Some(task) = task else {
            continue;
        };

        match task {
            WorkerTask::Check { manual } => {
                if manual {
                    let next = {
                        let mut g = lock.lock().unwrap();
                        g.state.status = UpdateStatus::Checking;
                        g.state.message = "正在连接 GitHub 检查新版本…".to_string();
                        g.state.clone()
                    };
                    on_state(next);
                }

                let res = check_for_updates(CURRENT_VERSION);
                let now = Local::now().timestamp();
                let next = {
                    let mut g = lock.lock().unwrap();
                    g.busy = false;
                    match res {
                        Ok(Some(release)) => {
                            g.last_error_auto_unix = None;
                            g.state.last_checked_unix = Some(now);
                            g.state.status = UpdateStatus::Available;
                            g.state.downloaded_bytes = 0;
                            g.state.total_bytes = release.size_bytes;
                            g.state.message = format!("发现新版本 v{}", release.version);
                            let is_skipped = release.version == g.skipped_version;
                            if manual || !is_skipped {
                                logging::info(&format!(
                                    "[UPDATER] 发现新版本 {}（当前 v{}）",
                                    release.tag, CURRENT_VERSION
                                ));
                            }
                            g.state.release = Some(release.clone());
                            save_persisted_meta(
                                &app_dir,
                                &PersistedUpdateMeta {
                                    last_checked_unix: Some(now),
                                    cached_release: Some(release),
                                },
                            );
                        }
                        Ok(None) => {
                            g.last_error_auto_unix = None;
                            g.state.last_checked_unix = Some(now);
                            g.state.status = UpdateStatus::UpToDate;
                            g.state.release = None;
                            g.state.downloaded_bytes = 0;
                            g.state.total_bytes = None;
                            g.state.message = format!("当前已是最新版本（v{CURRENT_VERSION}）");
                            if manual {
                                logging::info(&format!(
                                    "[UPDATER] 检查完成：当前 v{CURRENT_VERSION} 已是最新版本"
                                ));
                            }
                            save_persisted_meta(
                                &app_dir,
                                &PersistedUpdateMeta {
                                    last_checked_unix: Some(now),
                                    cached_release: None,
                                },
                            );
                        }
                        Err(err) => {
                            g.last_error_auto_unix = Some(now);
                            if manual {
                                g.state.status = UpdateStatus::Error;
                                g.state.message = err.clone();
                                logging::warn(&format!("[UPDATER] 手动检查更新失败：{err}"));
                            } else if g.state.release.is_some() {
                                g.state.status = UpdateStatus::Available;
                            } else {
                                g.state.status = UpdateStatus::Idle;
                            }
                        }
                    }
                    g.state.clone()
                };
                on_state(next);
            }
            WorkerTask::Install => {
                let Some(release) = ({
                    let mut g = lock.lock().unwrap();
                    let rel = g.state.release.clone();
                    if let Some(r) = &rel {
                        g.state.status = UpdateStatus::Downloading;
                        g.state.downloaded_bytes = 0;
                        g.state.total_bytes = r.size_bytes;
                        g.state.message = format!("正在下载 {}…", r.asset_name);
                    } else {
                        g.busy = false;
                    }
                    rel
                }) else {
                    continue;
                };
                on_state(lock.lock().unwrap().state.clone());

                let install_res = perform_self_update(&app_dir, &release, |dl, total| {
                    let snap = {
                        let mut g = lock.lock().unwrap();
                        g.state.status = UpdateStatus::Downloading;
                        g.state.downloaded_bytes = dl;
                        g.state.total_bytes = total;
                        g.state.clone()
                    };
                    on_state(snap);
                });

                match install_res {
                    Ok(()) => {
                        let snap = {
                            let mut g = lock.lock().unwrap();
                            g.busy = false;
                            g.state.status = UpdateStatus::Applying;
                            g.state.message =
                                format!("更新包就绪，正在重启至 v{}…", release.version);
                            g.state.clone()
                        };
                        logging::info(&format!(
                            "[UPDATER] 新版本 v{} 已原位替换完成，正在交接重启守护进程",
                            release.version
                        ));
                        on_state(snap);
                        save_persisted_meta(
                            &app_dir,
                            &PersistedUpdateMeta {
                                last_checked_unix: Some(Local::now().timestamp()),
                                cached_release: None,
                            },
                        );
                        on_quit_for_restart();
                        break;
                    }
                    Err(err) => {
                        logging::warn(&format!("[UPDATER] 自动更新失败：{err}"));
                        let snap = {
                            let mut g = lock.lock().unwrap();
                            g.busy = false;
                            g.state.status = UpdateStatus::Error;
                            g.state.message = err;
                            g.state.clone()
                        };
                        on_state(snap);
                    }
                }
            }
        }
    }
}

fn perform_self_update<F>(
    app_dir: &Path,
    release: &ReleaseInfo,
    on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, Option<u64>),
{
    let target_exe = std::env::current_exe().map_err(|e| format!("无法获取当前程序路径：{e}"))?;
    let same_dir_new = sibling_path_with_suffix(&target_exe, ".new");
    let daemon_pid = std::process::id();

    // 尝试直接在原程序同目录创建 `.new`（桌面、D 盘等常规目录无需 UAC 即可原位无感替换）
    let direct_writable = File::create(&same_dir_new).is_ok();
    if direct_writable {
        let _ = fs::remove_file(&same_dir_new);
        stream_download_and_verify(release, &same_dir_new, false, on_progress)?;
        atomic_swap_executable(&target_exe, &same_dir_new)?;
        std::process::Command::new(&target_exe)
            .args(["--after-update", &daemon_pid.to_string()])
            .spawn()
            .map_err(|e| format!("原位覆盖成功，但拉起新版进程失败：{e}"))?;
        Ok(())
    } else {
        // 原目录受系统保护（如 Program Files）：先下载校验至 %LOCALAPPDATA%\GXNUCampusLogin\updates\，再请求一次 UAC 提权覆盖
        let stage_dir = app_dir.join("updates");
        fs::create_dir_all(&stage_dir).map_err(|e| format!("无法创建更新暂存目录：{e}"))?;
        let staged_exe = stage_dir.join(format!("gxnu-update-{}.exe", release.version));
        stream_download_and_verify(release, &staged_exe, false, on_progress)?;
        launch_uac_apply_update(&staged_exe, &target_exe, daemon_pid)
    }
}

fn load_persisted_meta(app_dir: &Path) -> PersistedUpdateMeta {
    fs::read_to_string(app_dir.join(UPDATE_CACHE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_persisted_meta(app_dir: &Path, meta: &PersistedUpdateMeta) {
    if let Ok(json) = serde_json::to_string_pretty(meta) {
        let _ = fs::write(app_dir.join(UPDATE_CACHE_FILE), json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn sha256_matches_nist_vectors() {
        assert_eq!(
            sha256_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_bytes(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn version_comparison_supports_three_and_four_segments_and_skips_prerelease() {
        assert_eq!(compare_versions("0.2.2", "v0.2.2.0"), Some(Ordering::Equal));
        assert!(is_newer_stable_version("0.2.2", "v0.2.2.1"));
        assert!(is_newer_stable_version("0.2.2.2", "v0.2.2.3"));
        assert!(is_newer_stable_version("0.2.2.9", "v0.2.3"));
        assert!(!is_newer_stable_version("0.2.3", "v0.2.2.9"));
        assert!(!is_newer_stable_version("0.2.2", "v0.2.3-beta.1"));
    }

    #[test]
    fn atomic_swap_replaces_target_and_leaves_old_backup() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gxnu-campus-net-helper.exe");
        let staged = sibling_path_with_suffix(&target, ".new");
        fs::write(&target, b"MZ-old-binary").unwrap();
        fs::write(&staged, b"MZ-new-binary").unwrap();

        let old_path = atomic_swap_executable(&target, &staged).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"MZ-new-binary");
        assert_eq!(fs::read(&old_path).unwrap(), b"MZ-old-binary");
        assert!(!staged.exists());
    }

    #[test]
    fn manifest_and_download_verify_sha256_and_pe_header_end_to_end() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");

        // 构造大于 512KB 的模拟合法 PE 二进制载荷（以 `MZ` 开头）
        let mut fake_pe = vec![b'A'; (MIN_EXE_BYTES as usize) + 4096];
        fake_pe[0] = b'M';
        fake_pe[1] = b'Z';
        let expected_sha = sha256_bytes(&fake_pe);
        let dl_url = format!("{base}/download/gxnu-campus-net-helper-v0.2.3.exe");
        let manifest_json = serde_json::json!({
            "version": "0.2.3",
            "tag": "v0.2.3",
            "asset_name": "gxnu-campus-net-helper-v0.2.3.exe",
            "download_url": dl_url,
            "html_url": format!("{base}/releases/tag/v0.2.3"),
            "sha256": expected_sha,
            "size": fake_pe.len(),
            "notes": "- 新增自动更新"
        })
        .to_string();

        let pe_for_srv = fake_pe.clone();
        let srv = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut req_buf = [0u8; 2048];
                let n = stream.read(&mut req_buf).unwrap_or(0);
                let line = String::from_utf8_lossy(&req_buf[..n]);
                if line.starts_with("GET /releases/latest/download/latest.json ") {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                        manifest_json.len()
                    );
                    stream.write_all(hdr.as_bytes()).unwrap();
                    stream.write_all(manifest_json.as_bytes()).unwrap();
                } else {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                        pe_for_srv.len()
                    );
                    stream.write_all(hdr.as_bytes()).unwrap();
                    stream.write_all(&pe_for_srv).unwrap();
                }
            }
        });

        let info = check_for_updates_with_bases("0.2.2.2", &base, &base, true)
            .unwrap()
            .expect("should detect v0.2.3");
        assert_eq!(info.version, "0.2.3");
        assert_eq!(info.asset_name, "gxnu-campus-net-helper-v0.2.3.exe");

        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("downloaded.exe.new");
        let mut last_prog = 0u64;
        stream_download_and_verify(&info, &dest, true, |dl, _| {
            last_prog = dl;
        })
        .unwrap();
        srv.join().unwrap();

        assert_eq!(last_prog, fake_pe.len() as u64);
        assert_eq!(fs::read(&dest).unwrap().len(), fake_pe.len());
    }
}
