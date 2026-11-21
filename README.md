<p align="center">
  <img src="docs/images/app-icon.png" width="96" height="96" alt="GXNU 校园网守护助手图标">
</p>
<h1 align="center">GXNU 校园网守护助手</h1>
<p align="center">广西师范大学校园网（Dr.COM）Windows 托盘守护工具。断网自动重连、延迟趋势监控、断网原因溯源。</p>
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
    <img src="docs/images/main-window-light.png" alt="GXNU 校园网守护助手主窗口：左侧为账号与运营商设置，右侧为 24 小时校园网延迟曲线、断网记录与运行日志">
  </picture>
</p>

主窗口预览随 GitHub 浅色 / 深色模式自动切换。图中均使用 `DEMO` 演示账号与模拟数据。

<details>
<summary>查看浅色 / 深色双主题对比</summary>

| 浅色主题 | 深色主题 |
| :---: | :---: |
| [![浅色主窗口，含门户延迟曲线、断网记录与运行日志](docs/images/main-window-light.png)](docs/images/main-window-light.png) | [![深色主窗口，含门户延迟曲线、断网记录与运行日志](docs/images/main-window.png)](docs/images/main-window.png) |

点击任意截图可查看原图。

</details>

## 核心特性

- **极轻常驻，关窗即释放内存。** 单文件双模式 Rust 原生架构。需要查看曲线或修改设置时按需打开主窗口；点击关闭（`×`）后界面进程立即退出，完全释放 OpenGL 驱动与中文字库内存（约 114 MB），仅保留 **~2.9 MB 私有内存（~5.8 MB 工作集）**、后台接近 0% CPU 的纯 Win32 托盘守护进程。[架构与测量口径](docs/ARCHITECTURE.md)
- **分层诊断，智能防误报自动重连。** 切换 Wi-Fi、插拔网线或睡眠唤醒后，先等待路由稳定，再按「网卡 → 路由 → 公网探测 → 校园网门户」逐层排查：
  - 仅在确认校园网会话下线时才提交认证，并二次探测公网验证恢复。
  - 支持连续失败复核，避免单次网络抖动误判；门户仍在线时直接报告上游出口异常，不重复提交凭据。
  - 登录被拒时按 `5s → 15s → 30s → 60s → 300s` 递增退避，连续被拒自动提醒核对账号。
- **对 Clash / Mihomo / TUN 等代理友好。** 探测与门户认证强制直连、忽略环境变量代理，避免代理内核重启被误判为断网；认证时自动避开 `198.18.0.0/15`（Fake-IP TUN）与 `100.64.0.0/10`（Tailscale）虚拟地址，并采用完整 HTTP 应用层响应测量网关延迟，防止 TUN 虚拟网卡本地秒回产生“假 1 ms”。
- **延迟趋势与断网原因可溯源。** 看板提供最近 **1 / 7 / 24 小时**的校园网网关往返延迟曲线与共轴通断轨，对照右侧**断网记录与原因**清单，再在下方**运行日志**按级别（`INFO` / `WARN` / `ERR`）、时段和关键字筛选，轻松区分认证过期、网关不可达、DNS 故障或出口异常。
- **原生托盘体验与系统通知。** 自绘 Windows 11 Fluent 风格托盘右键菜单（自动同步浅/深色主题）与 5 态圆点图标；断网时弹出 Windows 原生通知告知原因，恢复后自动汇总断网时长。
- **凭据加密与防窥屏保护。** 密码仅通过 Windows DPAPI 写入当前用户的 **Windows 凭据管理器**，不写入配置文件或日志；已保存密码时输入框保持空白且不显示明文预览按钮；日志仅保留学号前 4 位。

## 适用范围

| 项目 | 支持情况 |
| :--- | :--- |
| 认证门户 | 广西师范大学 Dr.COM 校园网认证系统（`https://yc.gxnu.edu.cn`） |
| 运营商出口 | 校园用户、中国电信、中国联通、中国移动 |
| 运行环境 | Windows 10 / 11 x64（绿色单文件，免安装、无需管理员权限） |

## 快速开始

1. **下载并解压。** 前往 [GitHub Releases](https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest) 下载最新 Windows x64 ZIP 压缩包，解压到**固定目录**（建议避开临时目录，以免影响自启路径），双击运行 `GXNU 校园网守护助手.exe`。
   > 首次运行如触发 Windows SmartScreen（「Windows 已保护你的电脑」），是因为个人开源单文件未购买商业代码签名。你可以先核对 Release 附带的 `.sha256` 校验值，再点击「更多信息 → 仍要运行」。
2. **填写账号并启动守护。** 首次运行时会自动展开左侧设置面板的 **账号** 页。在 **账号与网络** 中填写校园网学号、密码并选择运营商出口，点击右上角 **保存并开始守护**。顶部状态栏会立即显示连接状态、在线时长与最近检测延迟。
3. **设置开机自启（可选）。** 如需开机后静默常驻托盘，先点击顶部 **设置** 展开左侧面板（已保存凭据时默认折叠），切换到 **监控** 页，打开 **开机自动启动到托盘**；若希望启动后直接开始巡检，同时打开 **启动后立即开始监控**，最后点击右上角 **保存设置** 使其生效。

<details>
<summary>从 v0.1.0 升级或校验下载文件</summary>

从 `v0.1.0` 升级时，先右键旧版托盘图标彻底退出，再运行新版 EXE。原先开启过开机自启的用户，可在新版 EXE 所在目录运行以下 PowerShell 命令刷新自启路径：

```powershell
& ".\GXNU 校园网守护助手.exe" --autostart on
```

每个 Release 均附带 `.sha256` 校验文件。下载后可在压缩包所在目录运行以下命令，比对输出哈希与校验文件是否一致（将 `<版本号>` 替换为实际版本）：

```powershell
Get-FileHash .\gxnu-campus-net-helper-<版本号>-windows-x64.zip -Algorithm SHA256
```

</details>

## 日常使用与托盘状态

### 常用操作

| 你想做什么 | 操作方式 |
| :--- | :--- |
| 放到后台常驻 | 关闭主窗口（`×`）即可释放界面内存，托盘守护继续运行。单击任务栏通知区中的应用图标即可重新打开。 |
| 随时手动复查 | 点击主窗口顶部或右键托盘菜单中的 **立即检测**。 |
| 仅做一次只读检测 | 右键托盘图标选择 **暂停监控**，再点击 **立即检测**。本次检测只读、不自动登录，完成后仍保持暂停。 |
| 恢复监控或彻底退出 | 右键托盘图标选择 **开始监控** 恢复巡检，或选择 **退出 GXNU 校园网守护助手** 结束全部进程。 |
| 排查上次掉线原因 | 在主窗口切换 `1h / 7h / 24h`，查看 **断网记录与原因** 的时刻，再到 **运行日志** 按级别、时段或关键字筛选。 |
| 切换主题与宽屏看板 | 点击标题栏右上角的太阳 / 月亮图标切换浅色与深色主题；点击侧栏图标或 **收起设置** 可收起左侧设置面板。 |

### 托盘菜单与状态图例

右键任务栏通知区中的蓝底独秀峰应用图标，可查看当前状态、实时网关延迟并执行快捷操作（菜单样式随应用主题自动切换）：

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/tray-menu.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/images/tray-menu-light.png">
    <img src="docs/images/tray-menu-light.png" width="280" alt="GXNU 校园网守护助手托盘右键菜单，显示校园网已连接、网关延迟 24 ms 及快捷操作项">
  </picture>
</p>

应用图标右下角的圆点代表当前状态，鼠标悬停可查看完整提示（格式为 `GXNU 校园网守护助手：<状态>`），便于区分同为琥珀色的三个恢复阶段：

| 图标 | 悬停状态 | 含义 |
| :---: | :--- | :--- |
| <img src="docs/images/tray-online.png" width="28" alt="绿色圆点"> | 网络正常 | 公网探测通过，按设定间隔继续巡检。 |
| <img src="docs/images/tray-checking.png" width="28" alt="天蓝色圆点"> | 检测中 | 正在检测当前网络连接，等待本轮结果。 |
| <img src="docs/images/tray-recovery.png" width="28" alt="琥珀色圆点"> | 正在复核 / 正在登录 / 正在验证 | 依次表示复查异常、提交门户认证、登录后复验公网连通性；复核通过也可直接恢复正常。 |
| <img src="docs/images/tray-offline.png" width="28" alt="红色圆点"> | 连接异常 | 已判定离线。打开主窗口查看原因；若提示连续登录被拒，请核对学号、密码和运营商出口。 |
| <img src="docs/images/tray-paused.png" width="28" alt="灰色圆点"> | 已暂停 / 已停止 | 自动监控未运行。暂停期间仍可手动检测，或从托盘恢复监控。 |

## 常见问题

**Q：关闭主窗口后程序退出吗？如何彻底退出？**  
点击主窗口右上角 `×` 只会关闭图形界面进程并释放全部 OpenGL 与字库内存，约 3 MB 的守护进程仍留在系统托盘持续监测。要彻底退出，请右键托盘图标选择 **退出 GXNU 校园网守护助手**。

**Q：同时开启 Clash / Mihomo 等代理（含 TUN 模式）会影响使用吗？**  
不会。公网探测和门户认证均直连系统路由，忽略代理环境变量，并自动跳过 Fake-IP TUN 等虚拟网卡地址。切换 TUN 瞬间触发的短暂抖动会进入「网络切换中」自动复查，不会误报断网；若门户显示仍在线，则会提示公网出口或 DNS 异常而不会反复重登。

**Q：账号密码和运行数据存在哪里？如何保护隐私？**  
密码保存在当前 Windows 用户的凭据管理器中（目标名 `GXNUCampusLogin/v4`）。学号、偏好设置、日志和最近 48 小时遥测保存在 `%LOCALAPPDATA%\GXNUCampusLogin\`。提交 Issue 反馈问题时，请只摘录相关日志，并手动遮盖学号、设备名、IP 和个人目录路径。

**Q：如何彻底卸载？**  
1. 若开启了开机自启，先在 **监控** 页关闭 **开机自动启动到托盘** 并保存（或在 EXE 目录执行 `--autostart off`）。
2. 右键托盘图标选择 **退出 GXNU 校园网守护助手**。
3. 删除程序 EXE 文件与 `%LOCALAPPDATA%\GXNUCampusLogin\` 目录；如需一并清除已保存密码，在「控制面板 → 凭据管理器 → Windows 凭据」中删除 `GXNUCampusLogin/v4`。

更多配置字段（`config.json`）、环境变量与排障细节请参阅 [配置与使用说明](docs/CONFIGURATION.md)。

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
- [配置与使用说明](docs/CONFIGURATION.md) — 运行时文件、`config.json` 字段范围、调试环境变量与 FAQ
- [更新记录](CHANGELOG.md) · [参与贡献](CONTRIBUTING.md) · [反馈问题 (Issues)](https://github.com/junkaih2019/gxnu-campus-net-helper/issues) · [安全问题报告](SECURITY.md) · [MIT 许可证](LICENSE)
