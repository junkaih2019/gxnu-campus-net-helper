<p align="center">
  <img src="docs/images/app-icon.png" width="96" height="96" alt="GXNU Campus Network Helper icon">
</p>
<h1 align="center">GXNU Campus Network Helper</h1>
<p align="center">A Windows tray app for Guangxi Normal University’s campus network. Reconnect automatically, track latency, and see what caused an outage.</p>
<p align="center"><a href="README.md" lang="zh-CN">简体中文</a> · <strong>English</strong></p>
<p align="center">
  <a href="https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest"><strong>Download for Windows x64</strong></a>
  &nbsp;·&nbsp; <a href="CHANGELOG.md">Changelog (Chinese)</a>
  &nbsp;·&nbsp; <a href="LICENSE"><img src="docs/images/license-mit.svg" alt="MIT License" align="absmiddle"></a>
</p>

## Screenshots

| Light theme | Dark theme |
| :---: | :---: |
| [![Light dashboard with portal latency and outage history](docs/images/main-window-light.png)](docs/images/main-window-light.png) | [![Dark dashboard with portal latency and outage history](docs/images/main-window.png)](docs/images/main-window.png) |

Select a dashboard screenshot to view it at full size. Both dashboard screenshots use the `DEMO` account and sample network history.

## Why use it

- **A small Rust process stays in the tray.** One Windows executable contains the background monitor and the on-demand dashboard. Closing the dashboard ends its process. The project recorded about **2.9 MB of private memory and a 5.8 MB working set** for the background process alone. The open dashboard uses additional memory. [Architecture and memory figures (Chinese)](docs/ARCHITECTURE.md)
- **Prompt reconnection when the network changes.** Switching Wi-Fi, plugging or unplugging Ethernet, or waking the computer triggers a check. Once the route settles, the helper checks the campus session, signs in when needed, and verifies internet access. If the portal still reports an active session, it reports an upstream fault and avoids repeated sign-ins.
- **An outage history you can investigate.** View **1, 7, or 24 hours** of campus portal latency. Match a drop on the chart to an outage record, then filter the log by time, severity, or keyword to distinguish an expired session from an unreachable gateway.

## Get started

1. **Download and extract.** Follow the Windows x64 download link above, extract the ZIP to a folder you plan to keep, and run `GXNU 校园网守护助手.exe`.
2. **Set up your account.** Under **账号与网络** (Account and network), enter your campus student ID and password, choose your carrier, and select **保存并开始守护** (Save and start monitoring). The top of the dashboard shows your connection status and last check time.
3. **Set startup preferences.** Under **监控** (Monitoring), enable **开机自动启动到托盘** to launch the app in the tray when you sign in to Windows. Enable **启动后立即开始监控** as well if you want checks to start on launch.

### Everyday controls

| Task | How to do it |
| --- | --- |
| Keep monitoring in the background | Close the dashboard. The monitor stays in the tray. Click its icon in the Windows notification area to reopen the dashboard. |
| Investigate the last outage | Choose a time range, find the event under **断网记录与原因** (Outages and causes), then check **运行日志** (Log) for the same time. |
| Run a single check | Right-click the tray icon, select **暂停监控** (Pause), then **立即检测** (Check now). The check is read-only and monitoring stays paused. |
| Resume monitoring or quit | Select **开始监控** (Start monitoring) from the tray, or **退出 GXNU 校园网守护助手** (Exit) to quit completely. |
| Switch themes | Click the sun or moon in the upper-right corner of the dashboard to switch between light and dark themes. |

### Tray icon states

The dot in the lower-right corner of the tray icon shows the current state. Hover over the icon for the exact status, including which of the three amber stages is active.

<img src="docs/images/tray-menu.png" alt="Windows taskbar with a green dot on the helper icon. The open menu reports an active campus connection and 59 ms gateway latency." width="300">

The green dot on the helper’s Wi-Fi icon means the connection is online. The open menu confirms **校园网已连接** (Campus network connected) and shows the gateway latency measured at that moment, 59 ms.

| Icon | Tooltip in the app | Meaning |
| :---: | --- | --- |
| <img src="docs/images/tray-online.png" width="28" alt="Green dot"> | 网络正常 · Online | An internet probe succeeded. Routine checks continue at the configured interval. |
| <img src="docs/images/tray-checking.png" width="28" alt="Sky-blue dot"> | 检测中 · Checking | A connection check is in progress. |
| <img src="docs/images/tray-recovery.png" width="28" alt="Amber dot"> | 正在复核 / 正在登录 / 正在验证 | Rechecking a failure, signing in to the portal, or verifying internet access after sign-in, respectively. A recheck can also return directly to online. |
| <img src="docs/images/tray-offline.png" width="28" alt="Red dot"> | 连接异常 · Offline | Open the dashboard to see the cause. If the portal repeatedly rejects sign-in, check the student ID, password, and carrier. |
| <img src="docs/images/tray-paused.png" width="28" alt="Gray dot"> | 已暂停 / 已停止 · Paused or stopped | Automatic checks are inactive. While paused, you can run a single check or resume monitoring from the tray. |

<details>
<summary>Upgrade from v0.1.0 or verify a download</summary>

When upgrading from v0.1.0, quit the old version from its tray menu before opening the new executable. If you had enabled startup at sign-in, run this PowerShell command from the new executable’s folder to update its registered path.

```powershell
& ".\GXNU 校园网守护助手.exe" --autostart on
```

Each release includes a `.sha256` file. For v0.1.1, run the following command and compare the hash with the one in the matching checksum file.

```powershell
Get-FileHash .\gxnu-campus-net-helper-0.1.1-windows-x64.zip -Algorithm SHA256
```

</details>

## Data and privacy

Passwords are stored in Windows Credential Manager for the current user. Your student ID and preferences are saved in `%LOCALAPPDATA%\GXNUCampusLogin\config.json`. The application directory also contains logs and recent network history. When reporting an issue, share only the relevant log lines and redact student IDs, device names, IP addresses, and personal paths. [Configuration and FAQ (Chinese)](docs/CONFIGURATION.md)

## Command line and source builds

Open PowerShell in the executable’s folder to use these options.

| Option | Effect |
| --- | --- |
| `--once` | Run one read-only check. Exit code 0 means online, 2 means offline, and 3 means the route changed during successive samples. |
| `--minimized` / `--minimized --start` | Start in the tray. Add `--start` to begin or resume monitoring in the background. |
| `--test-toast` | Send a test Windows notification. |
| `--autostart status` | Show whether startup at sign-in is enabled. |

For example, run `& ".\GXNU 校园网守护助手.exe" --once` to check the connection once.

<details>
<summary>Build the Windows x64 release from source</summary>

Install Rust and MinGW-w64. Put the directory containing `gcc`, `dlltool`, and `windres` on PATH, then run the following commands from the repository root.

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu --component rustfmt --component clippy
rustup override set stable-x86_64-pc-windows-gnu
.\build.ps1 verify
.\build.ps1 release
```

`verify` runs the formatting check, Clippy, tests, and a release build. `release` produces `target\release\GXNU 校园网守护助手.exe`.

</details>

[Architecture and diagnostics (Chinese)](docs/ARCHITECTURE.md) · [Contributing (Chinese)](CONTRIBUTING.md) · [Security reports](SECURITY.md) · [MIT License](LICENSE)
