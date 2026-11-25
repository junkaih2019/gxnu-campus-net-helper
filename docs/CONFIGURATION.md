# 配置、数据文件与使用说明

## 1. 运行时文件位置

所有数据均位于当前用户本地应用数据目录：`%LOCALAPPDATA%\GXNUCampusLogin\`

| 文件 / 目标 | 作用 |
| :--- | :--- |
| `config.json` | 多账号配置列表（含备注名、学号、运营商、月流量上限）、活跃账号 ID 及监控与通知参数 |
| Windows 凭据 `GXNUCampusLogin/v5/<账号ID>` | 按账号 ID 独立隔离保存的 DPAPI 加密密码（读取时自动兼容旧版 `GXNUCampusLogin/v4`） |
| `logs\app.log`（及 `.1` ~ `.5`） | 原始结构化运行日志，单文件上限 2 MB、保留 5 份轮转 |
| `metrics.jsonl` | 最近 48 小时（上限 5000 条）通断与诊断遥测记录；首次启动自动从 `app.log` 回填历史 |
| `traffic.json` | 按账号与日期记录的近 90 天物理网卡增量流量统计 |
| `ui_prefs.json` | 界面偏好（浅/深色主题、缩放比例、窗口大小、是否展示主页流量卡与分隔条比例） |
| `update_cache.json` | 自动更新最近检查时间戳与远端新版本元数据缓存 |
| `daemon.json` | 轻守护进程运行期间的本地 IPC 端口与会话令牌（退出时自动清理） |

---

## 2. `config.json` 字段说明

```json
{
  "student_id": "DEMO0001",
  "carrier": "2",
  "check_interval": 30,
  "heartbeat_interval": 600,
  "offline_interval": 15,
  "confirm_failures": 2,
  "auto_start_monitoring": false,
  "auto_boot": false,
  "notify_toast": true,
  "notify_wechat": false,
  "notify_quiet_minutes": 3,
  "monthly_quota_gb": 0,
  "auto_check_updates": true,
  "skipped_update_version": "",
  "accounts": [
    {
      "id": "default",
      "remark": "宿舍联通",
      "student_id": "DEMO0001",
      "carrier": "2",
      "monthly_quota_gb": 0
    }
  ],
  "active_account_id": "default"
}
```

| 字段 | 类型 | 合法范围 | 默认值 | 说明 |
| :--- | :--- | :--- | :--- | :--- |
| `accounts` | array | — | `[]` | 多账号配置列表，每项包含 `id`、`remark`（备注名，留空且同一学号配置多出口时自动按出口名称显示）、`student_id`、`carrier`（`"0"` 校园网 / `"1"` 中国电信 / `"2"` 中国联通 / `"3"` 中国移动 / `"4"` 广电网络）、`monthly_quota_gb`；同一学号修改密码时自动同步该学号下其余出口的加密凭据；旧版单账号配置启动时自动迁入 |
| `active_account_id` | string | — | `""` | 当前活跃账号的 `id` |
| `student_id` / `carrier` / `monthly_quota_gb` | — | — | — | 当前活跃账号的镜像字段（向后兼容旧版配置） |
| `check_interval` | integer | `10` ~ `3600` | `30` | 在线巡检间隔（秒） |
| `heartbeat_interval` | integer | `60` ~ `86400` | `600` | 持续在线时写入 `[HEALTH_CHECK]` 心跳日志的间隔（秒，界面按分钟展示） |
| `offline_interval` | integer | `5` ~ `300` | `15` | 离线状态下复检间隔（秒） |
| `confirm_failures` | integer | `1` ~ `5` | `2` | 连续失败达到此次数后才判定离线并触发自动重连 |
| `auto_start_monitoring` | boolean | — | `false` | 启动程序后是否自动进入监控状态 |
| `auto_boot` | boolean | — | `false` | 开机自动启动到系统托盘（以注册表 `HKCU\...\Run\GXNUCampusLoginRS` 为准） |
| `auto_check_updates` | boolean | — | `true` | 联网状态下每 12 小时静默检查 GitHub Release 新版本 |
| `skipped_update_version` | string | — | `""` | 用户选择跳过提醒的版本号；为空表示不跳过 |
| `notify_toast` | boolean | — | `true` | 断网与自动恢复时弹出 Windows 系统通知 |
| `notify_wechat` | boolean | — | `false` | 企业微信推送预留开关 |
| `notify_quiet_minutes` | integer | `1` ~ `60` | `3` | 远程推送预留的静默时间（分钟） |

---

## 3. 环境变量（调试 / 启动覆盖）

| 变量名 | 可选值 | 说明 |
| :--- | :--- | :--- |
| `GXNU_THEME` | `light` / `dark` | 临时覆盖界面启动主题（优先级高于 `ui_prefs.json`） |
| `GXNU_ZOOM` | `0.75` ~ `1.25` | 临时覆盖界面缩放比例 |
| `GXNU_SHOW_TRAFFIC` | `0` / `1` | 临时覆盖主页是否展示「本机流量」卡片 |
| `GXNU_VIEW` | `home` / `settings` | 指定启动时打开主页还是设置中心 |
| `GXNU_SETTINGS_TAB` | `account` / `monitor` / `display` / `notify` | 指定进入设置中心时的默认目录页 |
| `GXNU_NO_TAKEOVER` | `1` | 禁用新版自动接管后台守护进程与自动改写开机自启路径（开发测试隔离用） |

---

## 4. 安全与隐私设计

- **按账号隔离加密**：各账号密码通过 Windows `CredWriteW` 分别写入 `GXNUCampusLogin/v5/<账号ID>`，不写入 `config.json` 与 `app.log`，保存失败时自动回滚原凭据。
- **防旁人窥屏**：已保存密码时，设置中心密码框保持空白并提示「已保存，留空不修改」，不显示明文预览按钮；仅当你输入新密码时才出现预览开关，保存后立即清空。
- **日志脱敏**：日志仅记录学号前 4 位（如 `2024***`），所有认证 URL 与凭据均经过脱敏处理。
