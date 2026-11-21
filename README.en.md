<p align="center">
  <img src="docs/images/app-icon.png" width="96" height="96" alt="GXNU Campus Network Helper icon">
</p>
<h1 align="center">GXNU Campus Network Helper</h1>
<p align="center">A lightweight Windows tray app for Guangxi Normal University’s Dr.COM campus network. Reconnect automatically, track gateway latency, and diagnose outages.</p>
<p align="center"><a href="README.md" lang="zh-CN">简体中文</a> · <strong>English</strong></p>
<p align="center">
  <a href="https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest"><strong>Download Portable Windows x64</strong></a>
  &nbsp;·&nbsp; <a href="CHANGELOG.md">Changelog (Chinese)</a>
  &nbsp;·&nbsp; <a href="docs/CONFIGURATION.md">Configuration (Chinese)</a>
  &nbsp;·&nbsp; <a href="docs/ARCHITECTURE.md">Architecture (Chinese)</a>
  &nbsp;·&nbsp; <a href="LICENSE">MIT License</a>
</p>
<p align="center">
  <sub>Windows 10 / 11 x64 · Single portable executable · No admin required · Built in Rust</sub>
</p>

## Preview

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/main-window.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/images/main-window-light.png">
    <img src="docs/images/main-window-light.png" alt="GXNU Campus Network Helper dashboard showing account settings on the left and 24-hour portal latency, outage history, and live logs on the right">
  </picture>
</p>

The preview adapts automatically to GitHub’s light or dark theme. Screenshots use the `DEMO` account and simulated network data.

<details>
<summary>Compare light and dark themes side by side</summary>

| Light theme | Dark theme |
| :---: | :---: |
| [![Light dashboard with portal latency, outage history, and logs](docs/images/main-window-light.png)](docs/images/main-window-light.png) | [![Dark dashboard with portal latency, outage history, and logs](docs/images/main-window.png)](docs/images/main-window.png) |

Select either screenshot to view it at full size.

</details>

## Features

- **Tiny background footprint; closing the window frees UI memory.** A single Rust executable runs two separate modes. Open the dashboard on demand; closing it (`×`) exits the UI process immediately and releases OpenGL driver and CJK font memory (~114 MB), leaving only the pure Win32 tray monitor using about **2.9 MB private memory (5.8 MB working set)** and near-0% idle CPU. [Architecture and measurement details (Chinese)](docs/ARCHITECTURE.md)
- **Layered diagnostics with false-positive protection.** Switching Wi-Fi, plugging/unplugging Ethernet, or waking from sleep triggers a check once the route settles, inspecting adapter → route → public internet probes → campus portal in order:
  - Signs in only when the campus session is confirmed offline, then re-verifies public connectivity.
  - Requires consecutive failed checks to filter brief blips; reports an upstream outage without re-submitting credentials if the portal still reports an active session.
  - Backs off progressively (`5s → 15s → 30s → 60s → 300s`) when sign-in is rejected and prompts you to verify credentials after repeated rejections.
- **Friendly with Clash, Mihomo, and TUN proxies.** Probes and portal requests connect directly and ignore proxy environment variables, so proxy restarts are not mistaken for campus disconnects. Authentication automatically skips virtual tunnel IPs (`198.18.0.0/15` Fake-IP TUN and `100.64.0.0/10` Tailscale), and gateway latency measures full HTTP responses to avoid fake `1 ms` TCP handshake readings.
- **Latency trends and outage investigation.** View **1, 7, or 24 hours** of campus gateway latency alongside a synchronized uptime bar and an **outages and causes** list, then filter the live log by severity (`INFO` / `WARN` / `ERR`), time range, or keyword to tell expired sessions apart from gateway, DNS, or upstream faults.
- **Native tray UX and desktop alerts.** Includes a custom Windows 11 Fluent-style right-click tray menu (auto-synced with light/dark theme), a 5-state tray indicator dot, and native Windows Toast notifications reporting outage causes and total downtime upon recovery.
- **Encrypted credentials and shoulder-surfing protection.** Passwords are stored exclusively in **Windows Credential Manager** (DPAPI) for the current user—never written to config files or logs. Saved passwords are not prefilled and hide the reveal-password toggle; logs mask student IDs beyond the first 4 characters.

## Compatibility

| Item | Support |
| :--- | :--- |
| Campus portal | Guangxi Normal University Dr.COM portal (`https://yc.gxnu.edu.cn`) |
| Carrier exits | Campus (`校园用户`), China Telecom (`中国电信`), China Unicom (`中国联通`), China Mobile (`中国移动`) |
| Platform | Windows 10 / 11 x64 (portable single `.exe`, no installer or admin rights needed) |

## Quick start

1. **Download and extract.** Download the latest Windows x64 ZIP from [GitHub Releases](https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest), extract it to a **permanent folder** (so startup paths remain valid), and run `GXNU 校园网守护助手.exe`.
   > Because this standalone open-source executable is unsigned, Windows SmartScreen may show a warning on first launch. You can verify the `.sha256` checksum from the release first, then select **More info → Run anyway**.
2. **Enter your account and start monitoring.** On first launch, the left settings panel opens to **账号** (Account). Under **账号与网络** (Account and network), enter your student ID and password, choose your carrier, and click **保存并开始守护** (Save and start monitoring). The header immediately shows live status, uptime, and gateway latency.
3. **Enable startup at sign-in (optional).** To start silently in the tray with Windows, click **设置** (Settings) in the top bar if the left panel is collapsed, switch to **监控** (Monitoring), and enable **开机自动启动到托盘** (Launch to tray at sign-in). Enable **启动后立即开始监控** (Start monitoring on launch) as well if you want checks to begin automatically, then click **保存设置** (Save settings) in the upper-right corner to apply.

<details>
<summary>Upgrade from v0.1.0 or verify a download</summary>

When upgrading from `v0.1.0`, quit the old version from its tray menu before opening the new executable. If startup at sign-in was enabled previously, run this PowerShell command from the new executable’s folder to update its registered path:

```powershell
& ".\GXNU 校园网守护助手.exe" --autostart on
```

Each release includes a `.sha256` file. Run the command below (replacing `<version>` with the downloaded version) and compare the output hash with the checksum file:

```powershell
Get-FileHash .\gxnu-campus-net-helper-<version>-windows-x64.zip -Algorithm SHA256
```

</details>

## Everyday use and tray states

### Common actions

| Task | How to do it |
| :--- | :--- |
| Keep running in the background | Close the dashboard (`×`) to free UI memory while the tray monitor stays active. Click the app icon in the Windows notification area to reopen it. |
| Run a manual check anytime | Click **立即检测** (Check now) in the dashboard header or tray menu. |
| Run a single read-only check | Right-click the tray icon, choose **暂停监控** (Pause monitoring), then click **立即检测** (Check now). No sign-in is attempted and monitoring stays paused. |
| Resume monitoring or quit | Right-click the tray icon and choose **开始监控** (Start monitoring), or **退出 GXNU 校园网守护助手** (Exit) to stop completely. |
| Investigate the last outage | Switch between `1h / 7h / 24h`, check the timestamp under **断网记录与原因** (Outages and causes), then filter **运行日志** (Logs) by level, timeframe, or keyword. |
| Switch themes or widen charts | Click the sun / moon icon in the title bar to toggle themes, or click the sidebar button / **收起设置** (Hide settings) for a wider dashboard view. |

### Tray menu and status icons

Right-click the blue GXNU helper icon in the Windows notification area to inspect live status, gateway latency, and quick actions (menu appearance follows your light/dark theme):

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/tray-menu.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/images/tray-menu-light.png">
    <img src="docs/images/tray-menu-light.png" width="280" alt="GXNU Campus Network Helper Fluent tray menu showing campus network connected, 24 ms gateway latency, and quick controls">
  </picture>
</p>

The dot at the lower-right corner of the tray icon indicates the current state. Hover over the icon for the exact tooltip (`GXNU 校园网守护助手：<status>`), especially to distinguish the three amber recovery stages:

| Icon | Tooltip in the app | Meaning |
| :---: | :--- | :--- |
| <img src="docs/images/tray-online.png" width="28" alt="Green dot"> | 网络正常 · Online | Internet probe succeeded. Routine checks continue at the configured interval. |
| <img src="docs/images/tray-checking.png" width="28" alt="Sky-blue dot"> | 检测中 · Checking | A connectivity check is currently running. |
| <img src="docs/images/tray-recovery.png" width="28" alt="Amber dot"> | 正在复核 / 正在登录 / 正在验证 · Rechecking / Signing in / Verifying | Rechecking a failure, signing in to the portal, or verifying internet access after sign-in. A recheck can also return directly to online. |
| <img src="docs/images/tray-offline.png" width="28" alt="Red dot"> | 连接异常 · Offline | Confirmed offline. Open the dashboard to view the cause; if sign-in is repeatedly rejected, check your student ID, password, and carrier. |
| <img src="docs/images/tray-paused.png" width="28" alt="Gray dot"> | 已暂停 / 已停止 · Paused / Stopped | Automatic monitoring is inactive. You can still run manual checks or resume monitoring from the tray. |

## FAQ

**Q: Does closing the main window exit the app? How do I quit completely?**  
Clicking `×` closes only the GUI process (releasing OpenGL and font memory), while the ~3 MB tray daemon keeps monitoring. To exit completely, right-click the tray icon and choose **退出 GXNU 校园网守护助手**.

**Q: Does it work alongside Clash / Mihomo (including TUN mode)?**  
Yes. Probes and portal authentication bypass proxy environment variables and ignore virtual tunnel IPs such as Fake-IP TUN. Brief route changes when toggling TUN are treated as network switching and rechecked automatically; if the portal remains online, an upstream/DNS fault is reported instead of re-signing in.

**Q: Where are credentials and logs stored?**  
Passwords are saved in Windows Credential Manager (`GXNUCampusLogin/v4`) for the current user. Your student ID, settings, logs, and 48-hour telemetry live under `%LOCALAPPDATA%\GXNUCampusLogin\`. When filing an issue, include only relevant log lines and redact student IDs, device names, IPs, and personal paths.

**Q: How do I uninstall it cleanly?**  
1. If startup at sign-in is enabled, turn off **开机自动启动到托盘** under **监控** and save (or run `--autostart off`).
2. Right-click the tray icon and select **退出 GXNU 校园网守护助手**.
3. Delete the `.exe` file and `%LOCALAPPDATA%\GXNUCampusLogin\`. To remove saved credentials as well, delete `GXNUCampusLogin/v4` in **Control Panel → Credential Manager → Windows Credentials**.

For `config.json` fields, environment overrides, and more troubleshooting tips, see [Configuration and FAQ (Chinese)](docs/CONFIGURATION.md).

## Command line and building from source

### Command-line options

Run these from PowerShell in the executable’s folder:

| Option | Effect |
| :--- | :--- |
| `--once` | Run one read-only diagnostic check without taking the single-instance lock or submitting credentials. Exit codes: `0` online, `2` offline, `3` route changed between samples. |
| `--minimized` | Start silently in the tray without opening the dashboard. |
| `--start` | Start or resume monitoring immediately on launch; combinable as `--minimized --start`. |
| `--autostart on \| off \| status` | Enable, disable, or query startup at sign-in (`HKCU\...\Run` pointing to the current `.exe`). |
| `--test-toast` | Send a test Windows notification. |

> **Note:** Release binaries use the Windows GUI subsystem, so invoking them directly in PowerShell may not print stdout or wait for completion. Pipe through `Out-Host` to inspect output and exit code:
> ```powershell
> & ".\GXNU 校园网守护助手.exe" --once | Out-Host; $LASTEXITCODE
> ```

<details>
<summary>Build the Windows x64 release from source</summary>

Install Rust and 64-bit MinGW-w64. Put the directory containing `gcc`, `dlltool`, and `windres` on `PATH` (or set `MINGW_BIN`), then run from the repository root:

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu --component rustfmt --component clippy
rustup override set stable-x86_64-pc-windows-gnu
.\build.ps1 verify
.\build.ps1 release
```

`verify` runs `rustfmt`, Clippy, tests, and a release build. `release` embeds the icon and version metadata and produces `target\release\GXNU 校园网守护助手.exe`.

</details>

## Documentation and feedback

- [Architecture and diagnostics (Chinese)](docs/ARCHITECTURE.md) — Dual-mode architecture, local IPC, layered diagnostic state machine, and backoff policy
- [Configuration and FAQ (Chinese)](docs/CONFIGURATION.md) — Runtime files, `config.json` schema, debug environment variables, and FAQ
- [Changelog (Chinese)](CHANGELOG.md) · [Contributing (Chinese)](CONTRIBUTING.md) · [Issues](https://github.com/junkaih2019/gxnu-campus-net-helper/issues) · [Security reports (Chinese)](SECURITY.md) · [MIT License](LICENSE)
