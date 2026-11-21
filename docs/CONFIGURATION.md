# 配置、数据文件与使用说明

## 1. 运行时文件位置

所有数据均位于当前用户本地应用数据目录：`%LOCALAPPDATA%\GXNUCampusLogin\`

| 文件 / 目标 | 作用 |
| :--- | :--- |
| `config.json` | 学号、运营商、检测间隔、复核次数和通知偏好 |
| Windows 凭据 `GXNUCampusLogin/v4` | 使用 DPAPI/凭据管理器加密保存学号与密码 |
| `logs\app.log`（及 `.1` ~ `.5`） | 运行日志，单文件上限 2 MB、保留 5 份轮转 |
| `metrics.jsonl` | 最近 48 小时（上限 5000 条）延迟与断网遥测记录；首次启动自动从 `app.log` 回填历史 |
| `ui_prefs.json` | 图形界面主题偏好（`light` / `dark`） |
| `daemon.json` | 轻守护进程运行期间的本地 IPC 端口与会话令牌（退出时自动清理） |

---

## 2. `config.json` 字段说明

```json
{
  "student_id": "demo-student",
  "carrier": "2",
  "check_interval": 30,
  "heartbeat_interval": 600,
  "offline_interval": 15,
  "confirm_failures": 2,
  "auto_start_monitoring": false,
  "auto_boot": false,
  "notify_toast": true,
  "notify_wechat": false,
  "notify_quiet_minutes": 3
}
```

| 字段 | 类型 | 合法范围 | 默认值 | 说明 |
| :--- | :--- | :--- | :--- | :--- |
| `student_id` | string | — | `""` | 校园网学号 |
| `carrier` | string | `"0"` ~ `"3"` | `"2"` | 运营商出口：`0` 校园用户、`1` 中国电信、`2` 中国联通、`3` 中国移动 |
| `check_interval` | integer | `10` ~ `3600` | `30` | 在线巡检间隔（秒） |
| `heartbeat_interval` | integer | `60` ~ `86400` | `600` | 持续在线时写入 `[HEALTH_CHECK]` 心跳日志的间隔（秒，界面按分钟展示） |
| `offline_interval` | integer | `5` ~ `300` | `15` | 离线状态下复检间隔（秒） |
| `confirm_failures` | integer | `1` ~ `5` | `2` | 连续失败达到此次数后才判定离线并触发自动重连 |
| `auto_start_monitoring` | boolean | — | `false` | 启动程序后是否自动进入监控状态 |
| `auto_boot` | boolean | — | `false` | 开机自动启动到系统托盘（以注册表 `HKCU\...\Run\GXNUCampusLoginRS` 为准） |
| `notify_toast` | boolean | — | `true` | 断网与自动恢复时弹出 Windows 系统通知 |
| `notify_wechat` | boolean | — | `false` | 企业微信推送预留开关 |
| `notify_quiet_minutes` | integer | `1` ~ `60` | `3` | 远程推送预留的静默时间（分钟） |

---

首次打开时，如果 Windows 凭据管理器已有与配置匹配的凭据，可以直接开始监控。缺少匹配凭据时，界面展开账号页，填写后可点击“保存并开始守护”。这次启动只影响当前守护会话。开机自启仍由单独的选项控制。

保存后界面先提示已写入磁盘，收到守护的回执后提示生效。若配置已被另一个程序修改、热加载失败或连接超时，界面显示具体结果。遇到连续登录拒绝时，系统提醒核对账号设置。物理网络故障提示等待恢复，门户请求失败提示下次重试时间。

更换已保存的学号时，需要同时输入新账号的密码。修改同一账号的其他设置可以留空密码。新保存的凭据带有 UTF-16LE 编码标记。极早期 Rust 记录缺少编码标记，如果升级后密码仍被拒绝，重新保存一次即可消除编码歧义。

遥测仅由守护进程写入和清理。一次持续断网按一个事件计数，未达到复核次数的短暂抖动只保留采样。旧版心跳日志回填通断状态，只有明确记录的门户往返耗时才进入延迟统计。

## 3. 环境变量（调试 / 启动覆盖）

| 变量名 | 可选值 | 说明 |
| :--- | :--- | :--- |
| `GXNU_THEME` | `light` / `dark` | 临时覆盖界面启动主题（优先级高于 `ui_prefs.json`） |
| `GXNU_SETTINGS_TAB` | `account` / `monitor` / `notify` | 指定展开左侧设置栏时的默认标签页 |
| `GXNU_COLLAPSE` | `0` / `1` | 有匹配凭据时默认折叠（`1`），设为 `0` 可自动展开。首次缺少凭据时优先展开账号页 |

---

## 4. 安全与隐私设计

- **密码零明文落地**：密码只通过 Windows `CredWriteW` 存入系统凭据管理器，不写入 `config.json` 与 `app.log`。
- **防旁人窥屏**：已保存密码时，设置面板密码框保持空白并提示「已保存，留空不修改」，**不渲染眼睛预览图标**；仅当你键入新密码时才出现预览开关，保存后立即清空并重新隐藏。
- **日志脱敏**：日志仅记录学号前 4 位（如 `2024***`），所有认证 URL 与凭据均经过脱敏过滤。

---

## 5. 常见问题

### Q1：关闭主窗口后程序退出吗？如何完全退出？
点击主窗口右上角 `×` 会**关闭界面进程以释放全部 OpenGL 与字库内存**，但 ~3 MB 的轻守护进程仍留在系统托盘继续监测网络。若要完全退出程序，请**右键点击任务栏右下角的蓝色独秀峰托盘图标 →「退出校园网守护」**。

### Q2：开启了 Mihomo / Clash 等代理（含 TUN 模式）会影响校园网认证吗？
公网探测和校园门户请求使用系统路由。开关 TUN 可能短暂改变出口和 DNS。守护会进入“网络切换中”并复查，短暂失败的原因可在 `[NETWORK_SWITCH]` 日志中查看。若异常持续，门户会话在线时会提示公网出口或 DNS 故障。门户明确下线时会按原退避策略自动登录。


### Q3：Windows 资源管理器里的 `.exe` 图标还是旧的？
`target\release\gxnu-guard.exe` 已内置 6 档分辨率图标资源；若 Windows 资源管理器缓存未及时刷新，可按 `F5` 刷新目录或重启一次 `explorer.exe`。
