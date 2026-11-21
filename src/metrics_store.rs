//! 监控指标持久化与统计分析（独立于 `config.json` 与 `app.log`）。
//!
//! - 每次网络巡检与认证结果追加写入 `%LOCALAPPDATA%\GXNUCampusLogin\metrics.jsonl`
//! - 启动时自动从现有 `app.log` / `app.log.1` 回填历史记录，首次打开即有曲线
//! - 自动保留最近 48 小时（上限 5000 条），文件体积极小

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{Local, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};

const RETENTION_SECS: i64 = 48 * 3600;
const MAX_STORED_POINTS: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRange {
    H1,
    H7,
    H24,
}

impl TimeRange {
    pub const ALL: [TimeRange; 3] = [TimeRange::H1, TimeRange::H7, TimeRange::H24];

    pub fn hours(self) -> i64 {
        match self {
            Self::H1 => 1,
            Self::H7 => 7,
            Self::H24 => 24,
        }
    }

    pub fn secs(self) -> i64 {
        self.hours() * 3600
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::H1 => "1h",
            Self::H7 => "7h",
            Self::H24 => "24h",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::H1 => 0,
            Self::H7 => 1,
            Self::H24 => 2,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        match idx {
            0 => Self::H1,
            1 => Self::H7,
            _ => Self::H24,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetryPoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_id: Option<String>,
    /// Unix 时间戳（秒）。
    pub ts: i64,
    /// 门户的 HTTP 往返耗时（毫秒）。0 表示缺测或不适用。
    pub ms: u64,
    /// 是否公网在线。
    pub online: bool,
    /// 旧文件缺少复核标记，只能根据连续的离线采样推算断网次数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_offline: Option<bool>,
    /// 状态或断网原因码（例如 `ONLINE`、`AUTH_EXPIRED`、`INTERNET_TIMEOUT`）。
    pub reason: String,
    /// 事件类型：`check` / `login_ok` / `login_fail`。
    #[serde(default = "default_event")]
    pub event: String,
}

fn default_event() -> String {
    "check".to_string()
}

impl TelemetryPoint {
    pub fn check(ts: i64, ms: u64, online: bool, reason: impl Into<String>) -> Self {
        Self {
            sample_id: None,
            ts,
            ms,
            online,
            confirmed_offline: None,
            reason: reason.into(),
            event: "check".to_string(),
        }
    }

    pub fn auth_event(ts: i64, success: bool, reason: impl Into<String>) -> Self {
        Self {
            sample_id: None,
            ts,
            ms: 0,
            online: success,
            confirmed_offline: None,
            reason: reason.into(),
            event: if success { "login_ok" } else { "login_fail" }.to_string(),
        }
    }

    pub fn local_hms(&self) -> String {
        Local
            .timestamp_opt(self.ts, 0)
            .single()
            .map(|dt| dt.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "--:--:--".to_string())
    }

    pub fn local_hm(&self) -> String {
        Local
            .timestamp_opt(self.ts, 0)
            .single()
            .map(|dt| dt.format("%H:%M").to_string())
            .unwrap_or_else(|| "--:--".to_string())
    }

    pub fn local_md_hm(&self) -> String {
        Local
            .timestamp_opt(self.ts, 0)
            .single()
            .map(|dt| dt.format("%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-- --:--".to_string())
    }
}

#[derive(Debug, Clone, Default)]
pub struct RangeStats {
    pub check_count: usize,
    pub online_count: usize,
    pub uptime_pct: Option<f64>,
    pub avg_ms: Option<u64>,
    pub min_ms: Option<u64>,
    pub max_ms: Option<u64>,
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
    pub jitter_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotStatus {
    NoData,
    Online,
    Degraded,
    Offline,
}

#[derive(Debug, Clone)]
pub struct TimelineSlot {
    pub start_ts: i64,
    pub end_ts: i64,
    pub status: SlotStatus,
    pub checks: usize,
    pub online_checks: usize,
    pub avg_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct HistogramBucket {
    pub label: &'static str,
    pub count: usize,
    pub is_error: bool,
}

#[derive(Default)]
struct FileBounds {
    count: usize,
    oldest: Option<i64>,
}

impl FileBounds {
    fn include(&mut self, ts: i64) {
        self.count += 1;
        self.oldest = Some(self.oldest.map_or(ts, |old| old.min(ts)));
    }
}

/// The daemon owns the only writer. The UI reads a snapshot of the file.
pub struct MetricsStore {
    path: PathBuf,
    bounds: Mutex<Option<FileBounds>>,
}

impl MetricsStore {
    pub fn new(app_dir: &Path) -> Self {
        let store = Self {
            path: app_dir.join("metrics.jsonl"),
            bounds: Mutex::new(None),
        };
        if let Err(e) = store.maintain(None) {
            crate::logging::warn(&format!("[METRICS_ERROR] 加载历史失败：{e}"));
        }
        store
    }

    pub fn history(app_dir: &Path) -> Vec<TelemetryPoint> {
        match read_jsonl(&app_dir.join("metrics.jsonl")) {
            Ok(mut points) => {
                retain_history(&mut points, Local::now().timestamp());
                points
            }
            Err(e) => {
                crate::logging::warn(&format!("[METRICS_ERROR] 读取历史失败：{e}"));
                Vec::new()
            }
        }
    }

    pub fn append(&self, point: &TelemetryPoint) {
        if let Err(e) = self.maintain(Some(point)) {
            crate::logging::warn(&format!("[METRICS_ERROR] 保存采样失败：{e}"));
        }
    }

    fn maintain(&self, point: Option<&TelemetryPoint>) -> io::Result<()> {
        let now = Local::now().timestamp();
        let cutoff = now - RETENTION_SECS;
        let mut state = self.bounds.lock().unwrap();
        let point = point.filter(|p| p.ts >= cutoff);
        if let (Some(bounds), Some(p)) = (state.as_mut(), point) {
            if bounds.count < MAX_STORED_POINTS && bounds.oldest.is_none_or(|ts| ts >= cutoff) {
                let mut file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?;
                let mut line = serde_json::to_vec(p)?;
                line.push(b'\n');
                let old_size = file.metadata()?.len();
                if let Err(err) = file.write_all(&line) {
                    file.set_len(old_size)?;
                    return Err(err);
                }
                bounds.include(p.ts);
                return Ok(());
            }
        }

        let dir = self.path.parent().expect("metrics file has a parent");
        fs::create_dir_all(dir)?;
        let mut points = read_jsonl(&self.path)?;
        let original_len = points.len();
        let mut changed = point.is_some() || (state.is_none() && self.path.exists());
        if state.is_none() {
            let mut seen: HashSet<(String, i64)> =
                points.iter().map(|p| (p.event.clone(), p.ts)).collect();
            for p in parse_history_from_logs(&dir.join("logs/app.log"), cutoff) {
                if !(-2..=2)
                    .any(|offset| seen.contains(&(p.event.clone(), p.ts.saturating_add(offset))))
                {
                    seen.insert((p.event.clone(), p.ts));
                    points.push(p);
                    changed = true;
                }
            }
        }
        points.extend(point.cloned());
        retain_history(&mut points, now);
        if changed || points.len() != original_len {
            let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
            {
                let mut writer = BufWriter::new(tmp.as_file_mut());
                for p in &points {
                    serde_json::to_writer(&mut writer, p)?;
                    writer.write_all(b"\n")?;
                }
                writer.flush()?;
            }
            tmp.persist(&self.path)?;
        }
        *state = Some(FileBounds {
            count: points.len(),
            oldest: points.first().map(|p| p.ts),
        });
        Ok(())
    }
}

fn retain_history(points: &mut Vec<TelemetryPoint>, now: i64) {
    points.retain(|p| p.ts >= now - RETENTION_SECS);
    points.sort_by_key(|p| p.ts);
    points.drain(..points.len().saturating_sub(MAX_STORED_POINTS));
}

fn read_jsonl(path: &Path) -> io::Result<Vec<TelemetryPoint>> {
    let file = match fs::File::open(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        other => other?,
    };
    let mut points = Vec::new();
    for line in BufReader::new(file).lines() {
        if let Ok(p) = serde_json::from_str(&line?) {
            points.push(p);
        }
    }
    Ok(points)
}

/// 从 `app.log.1` 和 `app.log` 解析历史检测耗时与状态切换，让新装版首开即有曲线。
pub fn parse_history_from_logs(log_path: &Path, cutoff_ts: i64) -> Vec<TelemetryPoint> {
    let mut out = Vec::new();
    let backup = PathBuf::from(format!("{}.1", log_path.display()));
    for file_path in [backup, log_path.to_path_buf()] {
        let Ok(text) = fs::read_to_string(&file_path) else {
            continue;
        };
        for line in text.lines() {
            if let Some(pt) = parse_log_telemetry_line(line) {
                if pt.ts >= cutoff_ts {
                    out.push(pt);
                }
            }
        }
    }
    out
}

pub fn parse_log_telemetry_line(line: &str) -> Option<TelemetryPoint> {
    if line.len() < 24 {
        return None;
    }
    let stamp = line.get(0..19)?;
    let naive = NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S").ok()?;
    let ts = Local.from_local_datetime(&naive).single()?.timestamp();

    if line.contains("[HEALTH_CHECK]") {
        // Older heartbeats contain the duration of the whole scan, not a portal RTT.
        let rtt = line
            .split("网关延迟=")
            .nth(1)
            .and_then(|s| s.split_once("ms"))
            .and_then(|(ms, _)| ms.trim().parse().ok())
            .unwrap_or(0);
        return Some(TelemetryPoint::check(ts, rtt, true, "ONLINE"));
    }
    if line.contains("[LOGIN_VERIFIED]") || line.contains("[NETWORK_RECOVERED]") {
        return Some(TelemetryPoint::auth_event(ts, true, "ONLINE"));
    }
    if line.contains("[PORTAL_REJECTED]") || line.contains("[LOGIN_REJECTED]") {
        return Some(TelemetryPoint::auth_event(ts, false, "LOGIN_REJECTED"));
    }
    if line.contains("[STATE_CHANGE]") && line.contains("→ OFFLINE") {
        let reason = line
            .split("原因=")
            .nth(1)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("OFFLINE");
        return Some(TelemetryPoint::check(ts, 0, false, reason));
    }
    None
}

/// 过滤指定时间窗口内的全部采样点。
pub fn filter_range(points: &[TelemetryPoint], now: i64, range: TimeRange) -> Vec<&TelemetryPoint> {
    let start = now - range.secs();
    points
        .iter()
        .filter(|p| p.ts >= start && p.ts <= now + 5)
        .collect()
}

pub struct OutageSummary<'a> {
    pub starts: Vec<&'a TelemetryPoint>,
    pub recoveries: usize,
}

pub fn outages_in_range(
    points: &[TelemetryPoint],
    now: i64,
    range: TimeRange,
) -> OutageSummary<'_> {
    let start = now - range.secs();
    let mut summary = OutageSummary {
        starts: Vec::new(),
        recoveries: 0,
    };
    let mut offline = false;
    for p in points.iter().filter(|p| p.ts <= now + 5) {
        match p.event.as_str() {
            "check" if !p.online && p.confirmed_offline.unwrap_or(true) => {
                if !offline && p.ts >= start {
                    summary.starts.push(p);
                }
                offline = true;
            }
            "check" | "login_ok" if p.online => {
                if offline && p.ts >= start {
                    summary.recoveries += 1;
                }
                offline = false;
            }
            _ => {}
        }
    }
    summary
}

/// 计算时间窗口内的核心监控指标（可用率、均值、P50/P95、抖动、断网与重连次数）。
pub fn compute_stats(points: &[&TelemetryPoint]) -> RangeStats {
    let mut stats = RangeStats::default();
    let mut online_latencies: Vec<u64> = Vec::new();
    let mut consecutive_diffs: Vec<u64> = Vec::new();
    let mut prev_ms: Option<u64> = None;

    for pt in points {
        match pt.event.as_str() {
            "login_ok" | "login_fail" => {}
            _ => {
                stats.check_count += 1;
                if pt.online {
                    stats.online_count += 1;
                    if pt.ms > 0 {
                        online_latencies.push(pt.ms);
                        if let Some(prev) = prev_ms {
                            consecutive_diffs.push(pt.ms.abs_diff(prev));
                        }
                        prev_ms = Some(pt.ms);
                    }
                } else {
                    prev_ms = None;
                }
            }
        }
    }

    if stats.check_count > 0 {
        stats.uptime_pct = Some((stats.online_count as f64 / stats.check_count as f64) * 100.0);
    }

    if !online_latencies.is_empty() {
        let sum: u64 = online_latencies.iter().sum();
        stats.avg_ms = Some(sum / online_latencies.len() as u64);
        online_latencies.sort_unstable();
        stats.min_ms = online_latencies.first().copied();
        stats.max_ms = online_latencies.last().copied();
        stats.p50_ms = Some(percentile(&online_latencies, 0.50));
        stats.p95_ms = Some(percentile(&online_latencies, 0.95));
    }

    if !consecutive_diffs.is_empty() {
        let sum: u64 = consecutive_diffs.iter().sum();
        stats.jitter_ms = Some(sum / consecutive_diffs.len() as u64);
    }

    stats
}

fn percentile(sorted: &[u64], frac: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * frac).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// 计算探测耗时分布桶（用于柱状图）。
pub fn latency_histogram(points: &[&TelemetryPoint]) -> [HistogramBucket; 4] {
    let mut b_fast = 0usize; // < 300ms
    let mut b_good = 0usize; // 300..700ms
    let mut b_slow = 0usize; // >= 700ms
    let mut b_err = 0usize; // offline / timeout

    for pt in points {
        if pt.event != "check" {
            continue;
        }
        if !pt.online {
            b_err += 1;
        } else if pt.ms == 0 {
            continue;
        } else if pt.ms < 300 {
            b_fast += 1;
        } else if pt.ms < 700 {
            b_good += 1;
        } else {
            b_slow += 1;
        }
    }

    [
        HistogramBucket {
            label: "<300ms",
            count: b_fast,
            is_error: false,
        },
        HistogramBucket {
            label: "300-700",
            count: b_good,
            is_error: false,
        },
        HistogramBucket {
            label: ">700ms",
            count: b_slow,
            is_error: false,
        },
        HistogramBucket {
            label: "异常/超时",
            count: b_err,
            is_error: true,
        },
    ]
}

/// 将指定时间窗口划分为 `slot_count` 个等距槽，用于绘制 Uptime 状态时间轴色带。
pub fn uptime_timeline(
    points: &[&TelemetryPoint],
    now: i64,
    range: TimeRange,
    slot_count: usize,
) -> Vec<TimelineSlot> {
    let count = slot_count.max(1);
    let total = range.secs().max(count as i64);
    let start = now - total;
    let step = total as f64 / count as f64;

    let mut slots: Vec<TimelineSlot> = (0..count)
        .map(|i| TimelineSlot {
            start_ts: start + (i as f64 * step).round() as i64,
            end_ts: start + ((i + 1) as f64 * step).round() as i64,
            status: SlotStatus::NoData,
            checks: 0,
            online_checks: 0,
            avg_ms: None,
        })
        .collect();

    let mut sums = vec![0u64; count];
    let mut ms_counts = vec![0usize; count];

    for pt in points {
        if pt.event != "check" || pt.ts < start || pt.ts > now + 5 {
            continue;
        }
        let idx = (((pt.ts - start) as f64 / step).floor() as isize).clamp(0, (count - 1) as isize)
            as usize;
        slots[idx].checks += 1;
        if pt.online {
            slots[idx].online_checks += 1;
            if pt.ms > 0 {
                sums[idx] += pt.ms;
                ms_counts[idx] += 1;
            }
        }
    }

    for (idx, slot) in slots.iter_mut().enumerate() {
        if ms_counts[idx] > 0 {
            slot.avg_ms = Some(sums[idx] / ms_counts[idx] as u64);
        }
        slot.status = if slot.checks == 0 {
            SlotStatus::NoData
        } else if slot.online_checks == 0 {
            SlotStatus::Offline
        } else if slot.online_checks < slot.checks {
            SlotStatus::Degraded
        } else {
            SlotStatus::Online
        };
    }

    slots
}

/// 计算滑动指数移动均值（EMA），用于叠加平滑趋势线。
pub fn ema_series(values: &[f32], alpha: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(values.len());
    let mut prev: Option<f32> = None;
    for &v in values {
        let next = match prev {
            Some(p) => p + alpha * (v - p),
            None => v,
        };
        out.push(next);
        prev = Some(next);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_health_check_and_state_change_lines() {
        let ok_line = "2000-01-01 12:00:00 INFO [HEALTH_CHECK] 网络正常；IP=192.0.2.42；网关=192.0.2.1；检测耗时=0.57s；心跳间隔=30分钟";
        let pt = parse_log_telemetry_line(ok_line).expect("should parse HEALTH_CHECK");
        assert!(pt.online);
        assert_eq!(pt.ms, 0);
        assert_eq!(pt.reason, "ONLINE");

        let off_line = "2000-01-01 12:01:00 INFO [STATE_CHANGE] SUSPECT → OFFLINE；认证过期；原因=AUTH_EXPIRED";
        let pt2 = parse_log_telemetry_line(off_line).expect("should parse OFFLINE");
        assert!(!pt2.online);
        assert_eq!(pt2.reason, "AUTH_EXPIRED");

        let login_line = "2000-01-01 12:01:15 INFO [LOGIN_VERIFIED] 校园网登录成功，公网已恢复";
        let pt3 = parse_log_telemetry_line(login_line).expect("should parse LOGIN_VERIFIED");
        assert_eq!(pt3.event, "login_ok");
    }

    #[test]
    fn stats_percentiles_jitter_and_timeline_work() {
        let now = 1_700_000_000;
        let raw = vec![
            TelemetryPoint::check(now - 3000, 200, true, "ONLINE"),
            TelemetryPoint::check(now - 2400, 400, true, "ONLINE"),
            TelemetryPoint::check(now - 1800, 600, true, "ONLINE"),
            TelemetryPoint::check(now - 1200, 0, false, "AUTH_EXPIRED"),
            TelemetryPoint::auth_event(now - 1150, true, "ONLINE"),
            TelemetryPoint::check(now - 600, 800, true, "ONLINE"),
        ];
        let filtered = filter_range(&raw, now, TimeRange::H1);
        let stats = compute_stats(&filtered);
        assert_eq!(stats.check_count, 5);
        assert_eq!(stats.online_count, 4);
        let outages = outages_in_range(&raw, now, TimeRange::H1);
        assert_eq!(outages.starts.len(), 1);
        assert_eq!(outages.recoveries, 1);
        assert_eq!(stats.avg_ms, Some(500));
        assert_eq!(stats.min_ms, Some(200));
        assert_eq!(stats.max_ms, Some(800));
        assert_eq!(stats.jitter_ms, Some(200));

        let hist = latency_histogram(&filtered);
        assert_eq!(hist[0].count, 1); // <300ms (200)
        assert_eq!(hist[1].count, 2); // 300-700 (400, 600)
        assert_eq!(hist[2].count, 1); // >700 (800)
        assert_eq!(hist[3].count, 1); // error

        let slots = uptime_timeline(&filtered, now, TimeRange::H1, 12);
        assert_eq!(slots.len(), 12);
        assert!(slots.iter().any(|s| s.status == SlotStatus::Offline));
        assert!(slots.iter().any(|s| s.status == SlotStatus::Online));
    }

    #[test]
    fn ema_smooths_series() {
        let smoothed = ema_series(&[100.0, 200.0, 200.0], 0.5);
        assert_eq!(smoothed.len(), 3);
        assert!((smoothed[0] - 100.0).abs() < 1e-4);
        assert!((smoothed[1] - 150.0).abs() < 1e-4);
        assert!((smoothed[2] - 175.0).abs() < 1e-4);
    }

    #[test]
    fn daemon_retains_only_recent_5000_points_and_ui_read_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.jsonl");
        let now = Local::now().timestamp();
        let sample = |ts| TelemetryPoint::check(ts, 1, true, "ONLINE");
        let mut file = BufWriter::new(fs::File::create(&path).unwrap());
        for ts in std::iter::once(now - RETENTION_SECS - 5).chain(now - 5000..now) {
            writeln!(file, "{}", serde_json::to_string(&sample(ts)).unwrap()).unwrap();
        }
        drop(file);
        let store = MetricsStore::new(dir.path());
        store.append(&sample(now));
        let saved = read_jsonl(&path).unwrap();
        assert_eq!(saved.len(), MAX_STORED_POINTS);
        assert_eq!(saved[0].ts, now - 4999);
        assert_eq!(saved.last().unwrap().ts, now);
        let before = fs::read(&path).unwrap();
        assert_eq!(MetricsStore::history(dir.path()).len(), MAX_STORED_POINTS);
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn count_confirmed_episodes_across_samples_and_window_boundaries() {
        let now = 10_000;
        let cutoff = now - 3600;
        let mut suspect = TelemetryPoint::check(now - 90, 0, false, "AUTH_EXPIRED");
        suspect.confirmed_offline = Some(false);
        let points = [
            TelemetryPoint::check(cutoff - 10, 0, false, "LINK_DOWN"),
            TelemetryPoint::check(cutoff + 10, 0, false, "LINK_DOWN"),
            TelemetryPoint::check(now - 100, 0, true, "ONLINE"),
            suspect,
            TelemetryPoint::check(now - 80, 0, true, "ONLINE"),
            TelemetryPoint::check(now - 50, 0, false, "LINK_DOWN"),
            TelemetryPoint::check(now - 40, 0, false, "LINK_DOWN"),
            TelemetryPoint::auth_event(now - 30, true, "ONLINE"),
            TelemetryPoint::check(now - 29, 0, true, "ONLINE"),
        ];
        let outages = outages_in_range(&points, now, TimeRange::H1);
        assert_eq!(outages.starts.len(), 1);
        assert_eq!(outages.starts[0].ts, now - 50);
        assert_eq!(outages.recoveries, 2);
        let filtered = filter_range(&points, now, TimeRange::H1);
        assert_eq!(compute_stats(&filtered).avg_ms, None);
        assert_eq!(latency_histogram(&filtered)[0].count, 0);
    }

    #[test]
    fn reading_old_history_cannot_compact_or_overwrite_the_daemons_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.jsonl");
        let now = Local::now().timestamp();
        let old = TelemetryPoint::check(now - RETENTION_SECS - 50, 5, true, "ONLINE");
        let body = format!("{}\n", serde_json::to_string(&old).unwrap()).repeat(201);
        fs::write(&path, &body).unwrap();
        assert!(MetricsStore::history(dir.path()).is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), body);
        let _writer = MetricsStore::new(dir.path());
        assert!(read_jsonl(&path).unwrap().is_empty());
        assert_eq!(fs::metadata(path).unwrap().len(), 0);
    }
}
