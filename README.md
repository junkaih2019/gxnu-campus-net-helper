<p align="center">
  <img src="docs/images/app-icon.png" width="96" height="96" alt="GXNU 校园网守护助手图标">
</p>
<h1 align="center">GXNU 校园网守护助手</h1>
<p align="center">广西师范大学校园网的 Windows 托盘工具。自动重连，查看延迟，追踪断网原因。</p>
<p align="center"><strong>简体中文</strong> · <a href="README.en.md" lang="en">English</a></p>
<p align="center">
  <a href="https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest"><strong>下载 Windows x64 版</strong></a>
  &nbsp;·&nbsp; <a href="CHANGELOG.md">更新记录</a>
  &nbsp;·&nbsp; <a href="LICENSE"><img src="docs/images/license-mit.svg" alt="MIT 许可证" align="absmiddle"></a>
</p>

## 界面预览

| 浅色主题 | 深色主题 |
| :---: | :---: |
| [![浅色主窗口，含门户延迟曲线与断网记录](docs/images/main-window-light.png)](docs/images/main-window-light.png) | [![深色主窗口，含门户延迟曲线与断网记录](docs/images/main-window.png)](docs/images/main-window.png) |

点击主窗口截图可查看原图。两张主窗口截图均使用 `DEMO` 账号和演示数据。

## 项目优势

- **Rust 原生构建，适合常驻。** 一个 EXE 包含托盘守护和按需打开的图形界面。关闭主窗口后，界面进程退出。项目记录的典型后台占用约为 **2.9 MB 私有内存、5.8 MB 工作集**；打开看板时，界面进程另计。[架构与测量口径](docs/ARCHITECTURE.md)
- **网络变化时及时复查，需要时自动重登。** 切换 Wi-Fi、插拔网线或从睡眠恢复，都会触发检查。助手等路由短暂稳定后核对校园网会话，符合认证条件就重登，并再次探测公网确认恢复。门户仍在线时，会提示出口故障，避免重复提交凭据。
- **能看出何时掉线、为何掉线。** 看板可查看最近 **1、7 或 24 小时**的门户响应延迟。将曲线上的异常时刻与断网记录对照，再按时间、级别或关键词筛选日志，就能区分认证过期、网关不可达等情况。

## 安装与使用

1. **下载并解压。** 在页首打开 Windows x64 下载页，将 ZIP 解压到固定目录，双击 `GXNU 校园网守护助手.exe`。
2. **首次登录。** 在 **账号** 页的 **账号与网络** 区域 中填写校园网学号、密码，选择对应运营商，点击 **保存并开始守护**。顶部显示当前连接状态和最近检测时间。
3. **设置自启。** 在 **监控** 中打开 **开机自动启动到托盘**，程序会在登录 Windows 时启动。想让它启动后就开始巡检，再打开 **启动后立即开始监控**。

### 日常操作

| 你想做什么 | 操作 |
| --- | --- |
| 放到后台，稍后再看 | 关闭主窗口，托盘守护继续运行。单击任务栏通知区域的应用图标即可重新打开。 |
| 查看上一次掉线的原因 | 在看板选择时间范围，查看 **断网记录与原因**，再到 **运行日志** 查找相同时刻的事件。 |
| 只检测一次 | 右键托盘图标，选择 **暂停监控**，然后点击 **立即检测**。本次检查只读，完成后仍保持暂停。 |
| 恢复监控或完全退出 | 在托盘菜单选择 **开始监控** 恢复巡检，或选择 **退出 GXNU 校园网守护助手** 结束程序。 |
| 切换界面主题 | 点击主窗口右上角的太阳或月亮图标，切换浅色与深色主题。 |

### 看懂任务栏托盘图标

图标右下角的圆点表示当前状态。将鼠标停在图标上可看到具体提示，尤其适合区分同为琥珀色的三个阶段。

<img src="docs/images/tray-menu.png" alt="Windows 任务栏中的助手图标带绿色圆点，右键菜单显示校园网已连接，网关延迟 59 ms" width="300">

图中任务栏 Wi-Fi 图标右下角的绿点表示联网正常。右键菜单同时显示「校园网已连接」和当次测得的网关延迟（59 ms）。

| 图标 | 悬停提示 | 含义 |
| :---: | --- | --- |
| <img src="docs/images/tray-online.png" width="28" alt="绿色圆点"> | 网络正常 | 公网探测通过，按设定间隔继续巡检。 |
| <img src="docs/images/tray-checking.png" width="28" alt="天蓝色圆点"> | 检测中 | 正在检测当前连接，等待本轮结果。 |
| <img src="docs/images/tray-recovery.png" width="28" alt="琥珀色圆点"> | 正在复核 / 正在登录 / 正在验证 | 分别表示复查异常、提交门户认证、登录后验证公网连通性。一次复核也可能直接恢复正常。 |
| <img src="docs/images/tray-offline.png" width="28" alt="红色圆点"> | 连接异常 | 已判定离线。打开主窗口查看原因；连续登录被拒时，请核对学号、密码和运营商。 |
| <img src="docs/images/tray-paused.png" width="28" alt="灰色圆点"> | 已暂停 / 已停止 | 自动监控未运行。暂停期间可手动检测，也可从托盘恢复监控。 |

<details>
<summary>从 v0.1.0 升级或校验下载文件</summary>

从 v0.1.0 升级时，先从托盘退出旧版，再运行新 EXE。原先开了自启的用户，可在新 EXE 所在目录执行以下 PowerShell 命令更新自启路径。

```powershell
& ".\GXNU 校园网守护助手.exe" --autostart on
```

Release 附有 `.sha256` 校验文件。以下命令以 v0.1.1 为例，输出应与同版本校验文件中的哈希值一致。

```powershell
Get-FileHash .\gxnu-campus-net-helper-0.1.1-windows-x64.zip -Algorithm SHA256
```

</details>

## 数据与隐私

密码保存在当前 Windows 用户的凭据管理器中。学号与设置保存在 `%LOCALAPPDATA%\GXNUCampusLogin\config.json`，日志和近期网络记录也在该应用目录下。反馈问题时，请只摘录相关日志，并遮盖学号、设备名、IP 和个人目录。[配置与常见问题](docs/CONFIGURATION.md)

## 命令行与源码构建

在 EXE 所在目录打开 PowerShell。常用参数如下。

| 参数 | 用途 |
| --- | --- |
| `--once` | 只读检测。退出码 0 表示在线，2 表示离线，3 表示连续采样时出口发生变化。 |
| `--minimized` / `--minimized --start` | 驻留托盘；加上 `--start` 可在后台开始或恢复监控。 |
| `--test-toast` | 发送一条测试系统通知。 |
| `--autostart status` | 查询开机自启状态。 |

例如，运行 `& ".\GXNU 校园网守护助手.exe" --once` 可做一次只读诊断。

<details>
<summary>在 Windows x64 上构建发行版</summary>

安装 Rust 和 MinGW-w64，将 `gcc`、`dlltool`、`windres` 所在目录加入 PATH，在仓库根目录运行以下命令。

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu --component rustfmt --component clippy
rustup override set stable-x86_64-pc-windows-gnu
.\build.ps1 verify
.\build.ps1 release
```

`verify` 执行格式检查、Clippy、测试和 Release 构建。`release` 输出 `target\release\GXNU 校园网守护助手.exe`。

</details>

[架构与诊断策略](docs/ARCHITECTURE.md) · [参与贡献](CONTRIBUTING.md) · [安全问题报告](SECURITY.md) · [MIT 许可证](LICENSE)
