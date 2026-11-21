<p align="center">
  <img src="docs/images/app-icon.png" width="96" height="96" alt="GXNU 校园网守护助手图标">
</p>
<h1 align="center">GXNU 校园网守护助手</h1>
<p align="center">广西师范大学校园网（Dr.COM）Windows 托盘守护工具。断网自动重连、通断健康追踪、本机流量统计、多账号一键切换。</p>
<p align="center"><strong>简体中文</strong> · <a href="README.en.md" lang="en">English</a></p>
<p align="center">
  <a href="https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest"><strong>下载 Windows x64 便携版</strong></a>
  &nbsp;·&nbsp; <a href="CHANGELOG.md">更新记录</a>
  &nbsp;·&nbsp; <a href="docs/CONFIGURATION.md">配置说明</a>
  &nbsp;·&nbsp; <a href="docs/ARCHITECTURE.md">架构文档</a>
  &nbsp;·&nbsp; <a href="LICENSE">MIT 许可证</a>
</p>
<p align="center">
  <sub>Windows 10 / 11 x64 · 绿色单文件 · 免管理员权限 · Rust 原生构建</sub>
</p>

## 界面预览

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/main-window.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/images/main-window-light.png">
    <img src="docs/images/main-window-light.png" alt="GXNU 校园网守护助手 v0.2.0 主窗口：顶部状态与一键切号栏、中部连接稳定性胶囊时间轴与本机流量环形图、底部精简运行日志">
  </picture>
</p>

主窗口预览随 GitHub 浅色 / 深色模式自动切换。图中均使用 `DEMO` 演示账号与模拟数据。

<details>
<summary>查看浅色 / 深色双主题与设置中心预览</summary>

| 浅色主窗口 | 深色主窗口 |
| :---: | :---: |
| [![浅色主窗口](docs/images/main-window-light.png)](docs/images/main-window-light.png) | [![深色主窗口](docs/images/main-window.png)](docs/images/main-window.png) |

| 设置中心（浅色） | 设置中心（深色） |
| :---: | :---: |
| [![设置中心多账号管理浅色](docs/images/settings-light.png)](docs/images/settings-light.png) | [![设置中心多账号管理深色](docs/images/settings-dark.png)](docs/images/settings-dark.png) |

点击任意截图可查看原图。

</details>

## 核心特性

- **极轻常驻，关窗即释放内存。** 单文件双模式 Rust 原生架构。查看状态或修改设置时按需打开主窗口；点击关闭（`×`）后界面进程立即退出，完全释放 OpenGL 驱动与中文字库内存，仅保留极轻量的纯 Win32 托盘守护进程在后台静默运行。[架构与测量口径](docs/ARCHITECTURE.md)
- **分层诊断，智能防误报自动重连。** 切换 Wi-Fi、插拔网线或睡眠唤醒后，先等待路由稳定，再按「网卡 → 路由 → 公网探测 → 校园网门户」逐层排查：
  - 仅在确认校园网会话下线时才提交认证，并二次探测公网验证恢复。
  - 支持连续失败复核，过滤短暂抖动；门户仍在线时直接报告上游出口异常，不重复提交凭据。
  - 登录被拒时按 `5s → 15s → 30s → 60s → 300s` 递增退避，连续被拒自动提醒核对账号。
- **多账号管理、自定义备注与一键切号。** 支持保存多个校园网账号并自定义备注名（如「宿舍联通」「实验室电信」），主界面顶部状态条支持下拉一键切号：先校验目标账号凭据，再自动执行 Dr.COM 下线（`/drcom/logout`）→ 登录新账号 → 二阶段公网复验；各账号密码按 `GXNUCampusLogin/v5/<账号ID>` 独立加密隔离，自动平滑兼容旧版 `v4` 凭据。
- **连接稳定性与断联溯源一目了然。** 提供 **1h / 7h / 24h** 细粒度胶囊通断时间轴（绿色在线、黄色断点），鼠标悬停任意断点或断联明细行即弹出结构化详情小卡片，并支持时间轴与断联列表双向悬停联动。
- **本机物理网卡流量统计（可选展示）。** 自动采集物理出网接口增量流量并排除 Clash / Mihomo TUN 等虚拟网卡，支持今日 / 本月双模式自适应环形图与近 7 日统计；若走运营商免计费出口，可在「设置 → 外观与显示」一键隐藏流量卡，由连接稳定性双栏宽屏接管中央区域。
- **精简降噪运行日志。** 按 **`全部 / INFO / WARN / ERR`** 严格三层分级，自动过滤系统弹窗回执与内部握手中间态噪音，以自然中文呈现核心事件；超长内容自动限宽截断并在悬停时展示详情卡，单击任意行即可复制完整日志。
- **全局缩放、自由布局与 Windows 11 Fluent 设置中心。** 支持 `75%–125%` 全局界面缩放（快捷键 `Ctrl + / − / 0` 或 `Ctrl + 滚轮`）、窗口尺寸预设与自动记忆、主页横纵分隔条自由拖拽（双击恢复默认）；设置中心采用左侧分组目录（账号管理 / 监控与自启 / 外观与显示 / 通知与数据）与底部常驻保存栏。
- **对代理工具友好与隐私保护。** 探测与门户认证强制直连、忽略代理环境变量，自动跳过 Fake-IP TUN 与 Tailscale 虚拟地址；密码仅通过 Windows DPAPI 写入系统凭据管理器，不落盘明文，日志自动脱敏学号。

## 适用范围

| 项目 | 支持情况 |
| :--- | :--- |
| 认证门户 | 广西师范大学 Dr.COM 校园网认证系统（`https://yc.gxnu.edu.cn`） |
| 运营商出口 | 校园用户、中国电信、中国联通、中国移动 |
| 运行环境 | Windows 10 / 11 x64（绿色单文件，免安装、无需管理员权限） |

## 快速开始

1. **下载并解压。** 前往 [GitHub Releases](https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest) 下载最新 Windows x64 ZIP 压缩包，解压到**固定目录**（避开临时目录，以免影响自启路径），双击运行 `GXNU 校园网守护助手.exe`。
   > 首次运行如触发 Windows SmartScreen（「Windows 已保护你的电脑」），是因为个人开源单文件未购买商业代码签名。可先核对 Release 附带的 `.sha256` 校验值，再点击「更多信息 → 仍要运行」。
2. **添加账号并开始守护。** 首次运行时会自动进入 **设置中心 → 账号管理**。填写校园网学号、登录密码、可选备注名并选择运营商出口，点击右下角 **保存并开始守护** 后自动回到主页。
3. **添加多账号与一键切号（可选）。** 在 **设置 → 账号管理** 点击 **+ 新增账号** 可保存多个账号；回到主页后，点击顶部状态条里的账号胶囊（如 `宿舍联通 · 中国联通 ▾`）即可下拉一键下线并切换账号。
4. **设置开机自启与界面偏好（可选）。** 点击顶栏右侧 **设置**：
   - 在 **监控与自启** 中开启 **开机自动启动到托盘** 与 **启动后立即开始监控**，点击右下角 **保存设置**；
   - 在 **外观与显示** 中可随时调整浅/深色主题、界面缩放比例（`75%–125%`）、窗口尺寸预设以及是否在主页显示「本机流量」卡片。

<details>
<summary>从 v0.1.x 升级或校验下载文件</summary>

从 `v0.1.0` / `v0.1.1` 升级时：
1. 先右键旧版系统托盘图标选择 **退出 GXNU 校园网守护助手**；
2. 用新版 `GXNU 校园网守护助手.exe` 覆盖或放到固定目录后启动；
3. 原有单账号配置与 `GXNUCampusLogin/v4` 凭据会自动平滑迁移，无需重新配置。

如果原先开启过开机自启且更换了目录，可在新版 EXE 所在目录运行以下 PowerShell 命令刷新自启路径：

```powershell
& ".\GXNU 校园网守护助手.exe" --autostart on
```

每个 Release 均附带 `.sha256` 校验文件。下载后可在压缩包所在目录运行以下命令比对哈希（将 `<版本号>` 替换为实际版本）：

```powershell
Get-FileHash .\gxnu-campus-net-helper-<版本号>-windows-x64.zip -Algorithm SHA256
```

</details>

## 日常使用与托盘状态

### 常用操作

| 你想做什么 | 操作方式 |
| :--- | :--- |
| 放到后台常驻 | 关闭主窗口（`×`）即可释放界面内存，托盘守护继续运行。单击任务栏通知区中的应用图标即可重新打开。 |
| 一键切换校园网账号 | 点击主页顶部状态条中的账号胶囊，在下拉菜单中直接点击目标账号即可自动下线并切换登录。 |
| 随时手动复查 | 点击主页状态条或右键托盘菜单中的 **立即检测**。 |
| 仅做一次只读检测 | 点击 **暂停**（或托盘菜单 **暂停监控**），再点击 **立即检测**。本次检测只读、不自动登录，完成后仍保持暂停。 |
| 排查上次掉线原因 | 在主页切换 `1h / 7h / 24h`，悬停黄色断点或右侧断联明细查看结构化详情卡；在下方运行日志切换 `WARN / ERR` 或搜索关键字。 |
| 调整缩放与卡片比例 | 按 `Ctrl + / − / 0` 或 `Ctrl + 滚轮` 实时缩放界面；拖拽面板间的横纵分隔条自由调整比例（双击恢复默认）。 |
| 切换浅色 / 深色主题 | 点击标题栏右上角的太阳 / 月亮图标，或在 **设置 → 外观与显示** 中切换。 |
| 彻底退出程序 | 右键托盘图标选择 **退出 GXNU 校园网守护助手** 结束全部进程。 |

### 托盘菜单与状态图例

右键任务栏通知区中的蓝底独秀峰应用图标，可查看当前活跃账号备注、连接状态与实时网关延迟，并执行快捷操作（菜单样式自动同步浅/深色主题）：

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/tray-menu.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/images/tray-menu-light.png">
    <img src="docs/images/tray-menu-light.png" width="280" alt="GXNU 校园网守护助手托盘右键菜单">
  </picture>
</p>

应用图标右下角的圆点代表当前状态，鼠标悬停可查看完整提示（格式为 `GXNU 校园网守护助手：<状态>`）：

| 图标 | 悬停状态 | 含义 |
| :---: | :--- | :--- |
| <img src="docs/images/tray-online.png" width="28" alt="绿色圆点"> | 网络正常 | 公网探测通过，按设定间隔继续巡检。 |
| <img src="docs/images/tray-checking.png" width="28" alt="天蓝色圆点"> | 检测中 | 正在检测当前网络连接，等待本轮结果。 |
| <img src="docs/images/tray-recovery.png" width="28" alt="琥珀色圆点"> | 正在复核 / 正在登录 / 正在验证 | 依次表示复查异常、提交门户认证、登录后复验公网连通性；复核通过也可直接恢复正常。 |
| <img src="docs/images/tray-offline.png" width="28" alt="红色圆点"> | 连接异常 | 已判定离线。打开主窗口查看原因；若提示连续登录被拒，请核对学号、密码和运营商出口。 |
| <img src="docs/images/tray-paused.png" width="28" alt="灰色圆点"> | 已暂停 / 已停止 | 自动监控未运行。暂停期间仍可手动检测，或从托盘恢复监控。 |

## 常见问题

**Q：关闭主窗口后程序退出吗？如何彻底退出？**  
点击主窗口右上角 `×` 只会关闭图形界面进程并释放全部 OpenGL 与字库内存，轻守护进程仍留在系统托盘持续监测。要彻底退出，请右键托盘图标选择 **退出 GXNU 校园网守护助手**。

**Q：同时开启 Clash / Mihomo 等代理（含 TUN 模式）会影响使用或流量统计吗？**  
不会。公网探测和门户认证均直连系统路由，忽略代理环境变量，并自动跳过 Fake-IP TUN 等虚拟网卡地址；本机流量采样同样自动排除虚拟隧道网卡，只统计真实物理网卡在校园网在线期间的增量。

**Q：多账号密码和运行数据存在哪里？如何保护隐私？**  
各账号密码按 ID 独立加密保存在当前 Windows 用户的凭据管理器中（目标名 `GXNUCampusLogin/v5/<账号ID>`，并自动兼容旧版 `GXNUCampusLogin/v4`）。账号配置、界面偏好、流量统计、日志和最近 48 小时遥测保存在 `%LOCALAPPDATA%\GXNUCampusLogin\`。提交 Issue 时请只摘录相关日志，并手动遮盖个人学号、设备名与路径。

**Q：如何彻底卸载？**  
1. 若开启了开机自启，先在 **设置 → 监控与自启** 关闭 **开机自动启动到托盘** 并保存（或在 EXE 目录执行 `--autostart off`）。
2. 右键托盘图标选择 **退出 GXNU 校园网守护助手**。
3. 删除程序 EXE 文件与 `%LOCALAPPDATA%\GXNUCampusLogin\` 目录；如需一并清除已保存密码，在「控制面板 → 凭据管理器 → Windows 凭据」中删除 `GXNUCampusLogin/` 相关条目。

更多配置字段（`config.json`、`ui_prefs.json`、`traffic.json`）与环境变量说明请参阅 [配置与使用说明](docs/CONFIGURATION.md)。

## 进阶：命令行与源码构建

### 命令行参数

在 EXE 所在目录打开 PowerShell，可使用以下常用参数：

| 参数 | 用途 |
| :--- | :--- |
| `--once` | 执行一次只读诊断（不占用单实例锁，也不提交凭据）。退出码：`0` 在线、`2` 离线、`3` 连续采样期间出口发生变化。 |
| `--minimized` | 静默启动并驻留托盘，不弹出主窗口。 |
| `--start` | 启动后立即开始或恢复监控，可与 `--minimized` 组合为 `--minimized --start`。 |
| `--autostart on \| off \| status` | 开启、关闭或查询开机自启状态（开启时会将当前 EXE 路径写入 `HKCU\...\Run`）。 |
| `--test-toast` | 发送一条测试 Windows 系统通知。 |

> **提示：** 发行版 EXE 为 Windows GUI 子系统程序，在 PowerShell 中直接运行不会自动回显标准输出或阻塞等待。如需查看输出并获取真实退出码，请通过管道调用，例如：
> ```powershell
> & ".\GXNU 校园网守护助手.exe" --once | Out-Host; $LASTEXITCODE
> ```

<details>
<summary>在 Windows x64 上从源码构建</summary>

安装 Rust 与 64 位 MinGW-w64，将 `gcc`、`dlltool`、`windres` 所在目录加入 `PATH`（或通过环境变量 `MINGW_BIN` 指定），在仓库根目录运行：

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu --component rustfmt --component clippy
rustup override set stable-x86_64-pc-windows-gnu
.\build.ps1 verify
.\build.ps1 release
```

`verify` 依次执行格式检查、Clippy、测试与 Release 构建；`release` 会嵌入图标与版本信息并输出 `target\release\GXNU 校园网守护助手.exe`。

</details>

## 相关文档与反馈

- [架构与诊断策略](docs/ARCHITECTURE.md) — 双模式进程分离、IPC 协议、分层诊断状态机与退避策略
- [配置与使用说明](docs/CONFIGURATION.md) — 运行时文件、`config.json` 多账号字段、调试环境变量与 FAQ
- [更新记录](CHANGELOG.md) · [参与贡献](CONTRIBUTING.md) · [反馈问题 (Issues)](https://github.com/junkaih2019/gxnu-campus-net-helper/issues) · [安全问题报告](SECURITY.md) · [MIT 许可证](LICENSE)
