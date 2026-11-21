//! 本机物理网卡流量采样与按日持久化存储。
//!
//! - 通过 Windows `GetIfTable2` 读取物理以太网/Wi-Fi 网卡的 64 位 `InOctets` / `OutOctets`
//! - 自动排除环回接口、过滤驱动以及 Clash/Mihomo TUN、Tailscale、VMware、Hyper-V 等虚拟网卡，避免双倍计费
//! - 仅在校园网判定在线期间累加增量；离线、重连或计数器回绕时安全重建基线
//! - 按「账号键 + 日期（YYYY-MM-DD）」原子写入 `%LOCALAPPDATA%\GXNUCampusLogin\traffic.json`，保留近 90 天

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{Datelike, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::logging;
use crate::models::{DailyTraffic, TrafficSummary};

const TRAFFIC_FILE: &str = "traffic.json";
const RETENTION_DAYS: i64 = 90;
const FLUSH_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_ACCOUNT_KEY: &str = "_default";

/// 磁盘持久化结构：按账号键分桶，内部按 `YYYY-MM-DD` 存 `(rx_bytes, tx_bytes)`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TrafficDisk {
    #[serde(default)]
    accounts: BTreeMap<String, BTreeMap<String, DayCounters>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct DayCounters {
    rx: u64,
    tx: u64,
}

pub fn account_key_of(student_id: &str) -> String {
    let trimmed = student_id.trim();
    if trimmed.is_empty() {
        DEFAULT_ACCOUNT_KEY.to_string()
    } else {
        trimmed.to_string()
    }
}

/// 读取当前处于连接状态的物理网卡（以太网 / Wi-Fi）的 `(InOctets, OutOctets)` 计数。
pub fn read_physical_nic_counters() -> HashMap<u64, (u64, u64)> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, MIB_IF_TABLE2,
    };

    let mut out = HashMap::new();
    let mut table_ptr: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    unsafe {
        if GetIfTable2(&mut table_ptr) != 0 || table_ptr.is_null() {
            return out;
        }
        let count = (*table_ptr).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table_ptr).Table.as_ptr(), count);
        for row in rows {
            let alias = wide_buf_to_string(&row.Alias);
            let desc = wide_buf_to_string(&row.Description);
            let flags = row.InterfaceAndOperStatusFlags._bitfield;
            if is_physical_network_interface(
                row.Type,
                row.TunnelType,
                row.OperStatus,
                flags,
                &alias,
                &desc,
                IF_TYPE_ETHERNET_CSMACD,
                IF_TYPE_IEEE80211,
            ) {
                out.insert(row.InterfaceLuid.Value, (row.InOctets, row.OutOctets));
            }
        }
        FreeMibTable(table_ptr as *const _);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn is_physical_network_interface(
    if_type: u32,
    tunnel_type: i32,
    oper_status: i32,
    status_flags: u8,
    alias: &str,
    description: &str,
    ethernet_type: u32,
    wifi_type: u32,
) -> bool {
    // IfOperStatusUp == 1
    if oper_status != 1 {
        return false;
    }
    if if_type != ethernet_type && if_type != wifi_type {
        return false;
    }
    // TUNNEL_TYPE_NONE == 0
    if tunnel_type != 0 {
        return false;
    }
    // MIB_IF_ROW2_0 bitfield: bit0 HardwareInterface, bit1 FilterInterface, bit2 ConnectorPresent
    let hardware = (status_flags & 0x01) != 0;
    let filter = (status_flags & 0x02) != 0;
    let connector = (status_flags & 0x04) != 0;
    if !hardware || filter || !connector {
        return false;
    }
    !is_virtual_adapter_name(alias) && !is_virtual_adapter_name(description)
}

fn is_virtual_adapter_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    const VIRTUAL_KEYWORDS: &[&str] = &[
        "loopback",
        "wintun",
        "mihomo",
        "clash",
        "sing-box",
        "nekobox",
        "tailscale",
        "wireguard",
        "zerotier",
        "vmware",
        "virtualbox",
        "vbox",
        "hyper-v",
        "vethernet",
        "virtual",
        "teredo",
        "isatap",
        "bluetooth",
        "npcap",
        "tap-windows",
        "openvpn",
        "vpn",
        "tun",
    ];
    VIRTUAL_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

fn wide_buf_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 守护进程持有的流量统计器（单写者），按账号和日期增量统计。
pub struct TrafficTracker {
    path: PathBuf,
    disk: TrafficDisk,
    baselines: HashMap<u64, (u64, u64)>,
    was_online: bool,
    dirty: bool,
    last_flush: Instant,
}

impl TrafficTracker {
    pub fn new(app_dir: &Path) -> Self {
        let path = app_dir.join(TRAFFIC_FILE);
        let mut disk = read_disk(&path).unwrap_or_else(|err| {
            logging::warn(&format!("[TRAFFIC_ERROR] 读取流量历史失败：{err}"));
            TrafficDisk::default()
        });
        let today = Local::now().date_naive();
        prune_old_days(&mut disk, today);
        Self {
            path,
            disk,
            baselines: HashMap::new(),
            was_online: false,
            dirty: false,
            last_flush: Instant::now(),
        }
    }

    /// 供 UI 进程只读加载初始流量摘要（绝不修改守护进程文件）。
    pub fn read_summary(app_dir: &Path, account_key: &str) -> TrafficSummary {
        let disk = read_disk(&app_dir.join(TRAFFIC_FILE)).unwrap_or_default();
        build_summary(&disk, account_key, Local::now().date_naive())
    }

    pub fn summary(&self, account_key: &str, today: NaiveDate) -> TrafficSummary {
        build_summary(&self.disk, account_key, today)
    }

    /// 处理一次网卡采样，返回 `(是否有新增流量, 最新摘要)`。
    pub fn tick(
        &mut self,
        online: bool,
        account_key: &str,
        today: NaiveDate,
        sample: &HashMap<u64, (u64, u64)>,
    ) -> (bool, TrafficSummary) {
        if !online {
            self.baselines.clone_from(sample);
            self.was_online = false;
            return (false, build_summary(&self.disk, account_key, today));
        }

        if !self.was_online {
            // 首次上线或从离线恢复：先建立干净基线，不把离线/启动前累计值算入。
            self.baselines.clone_from(sample);
            self.was_online = true;
            return (false, build_summary(&self.disk, account_key, today));
        }

        let mut delta_rx = 0u64;
        let mut delta_tx = 0u64;
        for (&luid, &(rx, tx)) in sample {
            if let Some(&(prev_rx, prev_tx)) = self.baselines.get(&luid) {
                if rx >= prev_rx {
                    delta_rx = delta_rx.saturating_add(rx - prev_rx);
                }
                if tx >= prev_tx {
                    delta_tx = delta_tx.saturating_add(tx - prev_tx);
                }
            }
        }
        self.baselines.clone_from(sample);

        let changed = delta_rx > 0 || delta_tx > 0;
        if changed {
            let date_key = format_date(today);
            let bucket = self
                .disk
                .accounts
                .entry(account_key.to_string())
                .or_default();
            let day = bucket.entry(date_key).or_default();
            day.rx = day.rx.saturating_add(delta_rx);
            day.tx = day.tx.saturating_add(delta_tx);
            self.dirty = true;
            if self.last_flush.elapsed() >= FLUSH_INTERVAL {
                self.flush_for(today);
            }
        }

        (changed, build_summary(&self.disk, account_key, today))
    }

    pub fn flush(&mut self) {
        self.flush_for(Local::now().date_naive());
    }

    fn flush_for(&mut self, today: NaiveDate) {
        if !self.dirty {
            return;
        }
        prune_old_days(&mut self.disk, today);
        if let Err(err) = write_disk(&self.path, &self.disk) {
            logging::warn(&format!("[TRAFFIC_ERROR] 保存流量统计失败：{err}"));
            return;
        }
        self.dirty = false;
        self.last_flush = Instant::now();
    }
}

fn format_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn prune_old_days(disk: &mut TrafficDisk, today: NaiveDate) {
    let cutoff = today - chrono::Duration::days(RETENTION_DAYS);
    let cutoff_str = format_date(cutoff);
    for days in disk.accounts.values_mut() {
        days.retain(|date, _| date.as_str() >= cutoff_str.as_str());
    }
    disk.accounts.retain(|_, days| !days.is_empty());
}

fn build_summary(disk: &TrafficDisk, account_key: &str, today: NaiveDate) -> TrafficSummary {
    let today_key = format_date(today);
    let month_prefix = format!("{:04}-{:02}-", today.year(), today.month());
    let bucket = disk.accounts.get(account_key);

    let (mut today_rx, mut today_tx) = (0u64, 0u64);
    let (mut month_rx, mut month_tx) = (0u64, 0u64);

    if let Some(days) = bucket {
        if let Some(day) = days.get(&today_key) {
            today_rx = day.rx;
            today_tx = day.tx;
        }
        for (date, counters) in days {
            if date.starts_with(&month_prefix) {
                month_rx = month_rx.saturating_add(counters.rx);
                month_tx = month_tx.saturating_add(counters.tx);
            }
        }
    }

    let mut last_7_days = Vec::with_capacity(7);
    for offset in (0..7).rev() {
        let d = today - chrono::Duration::days(offset);
        let key = format_date(d);
        let counters = bucket
            .and_then(|days| days.get(&key))
            .copied()
            .unwrap_or_default();
        last_7_days.push(DailyTraffic {
            date: key,
            rx_bytes: counters.rx,
            tx_bytes: counters.tx,
        });
    }

    TrafficSummary {
        today_rx_bytes: today_rx,
        today_tx_bytes: today_tx,
        month_rx_bytes: month_rx,
        month_tx_bytes: month_tx,
        last_7_days,
    }
}

fn read_disk(path: &Path) -> io::Result<TrafficDisk> {
    let text = match fs::read_to_string(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(TrafficDisk::default()),
        other => other?,
    };
    serde_json::from_str(&text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn write_disk(path: &Path, disk: &TrafficDisk) -> io::Result<()> {
    let dir = path.parent().expect("traffic file has a parent");
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(tmp.as_file_mut(), disk)?;
    tmp.write_all(b"\n")?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(entries: &[(u64, u64, u64)]) -> HashMap<u64, (u64, u64)> {
        entries.iter().map(|&(id, rx, tx)| (id, (rx, tx))).collect()
    }

    #[test]
    fn filters_out_virtual_and_tun_interfaces() {
        // Real Intel Wi-Fi adapter
        assert!(is_physical_network_interface(
            71,
            0,
            1,
            0x05,
            "WLAN",
            "Intel(R) Wi-Fi 6E AX211 160MHz",
            6,
            71,
        ));
        // Mihomo / Clash Wintun interface
        assert!(!is_physical_network_interface(
            6,
            0,
            1,
            0x05,
            "Mihomo",
            "Wintun Userspace Tunnel",
            6,
            71,
        ));
        // NDIS filter interface (bit1 set)
        assert!(!is_physical_network_interface(
            6,
            0,
            1,
            0x07,
            "Ethernet",
            "Realtek PCIe GbE-WFP LightWeight Filter",
            6,
            71,
        ));
        // Hyper-V vEthernet software switch
        assert!(!is_physical_network_interface(
            6,
            0,
            1,
            0x05,
            "vEthernet (WSL)",
            "Hyper-V Virtual Ethernet Adapter",
            6,
            71,
        ));
        // Tunnel interface
        assert!(!is_physical_network_interface(
            131,
            1,
            1,
            0x05,
            "Tailscale",
            "Tailscale Tunnel",
            6,
            71,
        ));
    }

    #[test]
    fn counts_only_online_deltas_and_handles_resets_and_rollups() {
        let dir = tempfile::tempdir().unwrap();
        let mut tracker = TrafficTracker::new(dir.path());
        let day1 = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let day2 = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();

        // Offline sample establishes baseline without counting.
        let (changed, s) = tracker.tick(false, "demo", day1, &sample(&[(1, 1000, 500)]));
        assert!(!changed);
        assert_eq!(s.today_rx_bytes, 0);

        // First online tick establishes online baseline without counting pre-online traffic.
        let (changed, s) = tracker.tick(true, "demo", day1, &sample(&[(1, 2000, 800)]));
        assert!(!changed);
        assert_eq!(s.today_rx_bytes, 0);

        // Second online tick counts exact delta.
        let (changed, s) = tracker.tick(true, "demo", day1, &sample(&[(1, 3500, 1100)]));
        assert!(changed);
        assert_eq!(s.today_rx_bytes, 1500);
        assert_eq!(s.today_tx_bytes, 300);

        // Counter reset (e.g. adapter disabled/re-enabled) does not underflow or inflate.
        let (changed, s) = tracker.tick(true, "demo", day1, &sample(&[(1, 100, 50)]));
        assert!(!changed);
        assert_eq!(s.today_rx_bytes, 1500);

        // Next day accumulates into day2 and rolls up into the same month + 7-day window.
        let (changed, s) = tracker.tick(true, "demo", day2, &sample(&[(1, 600, 250)]));
        assert!(changed);
        assert_eq!(s.today_rx_bytes, 500);
        assert_eq!(s.today_tx_bytes, 200);
        assert_eq!(s.month_rx_bytes, 2000);
        assert_eq!(s.month_tx_bytes, 500);
        assert_eq!(s.last_7_days.len(), 7);
        assert_eq!(s.last_7_days[5].date, "2026-11-20");
        assert_eq!(s.last_7_days[5].rx_bytes, 1500);
        assert_eq!(s.last_7_days[6].date, "2026-11-21");
        assert_eq!(s.last_7_days[6].rx_bytes, 500);

        // Persists cleanly to disk and reloads identically.
        tracker.flush_for(day2);
        let reloaded = TrafficTracker::new(dir.path());
        let summary = reloaded.summary("demo", day2);
        assert_eq!(summary.month_rx_bytes, 2000);
        assert_eq!(summary.today_rx_bytes, 500);
    }
}
