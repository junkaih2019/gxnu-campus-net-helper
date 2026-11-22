<p align="center">
  <img src="docs/images/app-icon.png" width="88" height="88" alt="GXNU Campus Network Helper icon">
</p>
<h1 align="center">GXNU Campus Network Helper</h1>
<p align="center">A lightweight Windows tray app for Guangxi Normal University’s Dr.COM campus network — auto-reconnect, live traffic & device sync, and 1-click multi-account switching</p>
<p align="center"><a href="README.md">简体中文</a> · <strong>English</strong></p>
<p align="center">
  <a href="https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest"><strong>⬇️ Download Portable Windows x64</strong></a>
  &nbsp;·&nbsp; <a href="CHANGELOG.md">Changelog</a>
  &nbsp;·&nbsp; <a href="docs/CONFIGURATION.md">Configuration</a>
  &nbsp;·&nbsp; <a href="docs/ARCHITECTURE.md">Architecture</a>
  &nbsp;·&nbsp; <a href="LICENSE">MIT License</a>
</p>
<p align="center">
  <sub>Windows 10 / 11 x64 · Single portable executable · No installer or admin rights · Built with Rust</sub>
</p>

---

## ✨ Preview

### Main Dashboard

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/main-window.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/images/main-window-light.png">
    <img src="docs/images/main-window-light.png" alt="GXNU Campus Network Helper main dashboard">
  </picture>
</p>

### Tray Experience (Left-click Flyout × Right-click Menu)

<table>
  <tr>
    <th align="center">🖱️ Left-click · Quick Flyout</th>
    <th align="center">🖱️ Right-click · Context Menu</th>
  </tr>
  <tr>
    <td align="center" valign="middle">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/images/tray-flyout.png">
        <source media="(prefers-color-scheme: light)" srcset="docs/images/tray-flyout-light.png">
        <img src="docs/images/tray-flyout-light.png" width="300" alt="Left-click vertical tray flyout">
      </picture>
    </td>
    <td align="center" valign="middle">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/images/tray-menu.png">
        <source media="(prefers-color-scheme: light)" srcset="docs/images/tray-menu-light.png">
        <img src="docs/images/tray-menu-light.png" width="248" alt="Right-click context menu">
      </picture>
    </td>
  </tr>
</table>

<details>
<summary>🖼️ More views: Light / Dark comparison, Campus multi-device view & Settings Center</summary>

| Light Dashboard (Carrier view) | Dark Dashboard (Carrier view) |
| :---: | :---: |
| [![Light dashboard](docs/images/main-window-light.png)](docs/images/main-window-light.png) | [![Dark dashboard](docs/images/main-window.png)](docs/images/main-window.png) |

| Campus Quota & Multi-device View | Settings Center (Multi-account) |
| :---: | :---: |
| [![Campus quota & multi-device view](docs/images/main-window-campus.png)](docs/images/main-window-campus.png) | [![Settings Center multi-account](docs/images/settings-dark.png)](docs/images/settings-dark.png) |

</details>

---

## 💡 Key Features

- 🔄 **Seamless Auto-Reconnect**: Automatically detects drops and signs back in after sleep/wake, Wi-Fi switches, or session timeouts.
- 📊 **Live Traffic & Online Devices**: Syncs read-only with GXNU Self-Service to show **Carrier throughput / Campus quota**, active terminals, and your real campus IP.
- ⚡ **Instant Left-click Tray Flyout**: Single-click the tray icon for a native rounded flyout combining traffic, devices, and stability at a glance.
- 👥 **1-Click Multi-Account Switch**: Save multiple accounts with custom remarks and switch atomically from the top status bar.
- 📈 **Uptime Timeline**: Tracks `1h / 7h / 24h` connection stability in a high-density capsule strip with outage hover cards.
- 🛡️ **Proxy-Friendly & Secure**: Works cleanly alongside Clash / Mihomo (TUN mode); encrypts passwords in Windows Credential Manager.
- 🪶 **Ultra-Light Background Daemon**: Single native Rust `.exe`; closing the window frees all UI/GPU memory while the Win32 tray monitor stays active.

---

## 🚀 Quick Start

1. **Download & Run**: Grab the latest ZIP from [Releases](https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest), extract it to a **permanent folder**, and launch `GXNU 校园网守护助手.exe`.
   > 💡 If Windows SmartScreen appears on first launch, click **More info → Run anyway** (you can verify the `.sha256` checksum from the release page).
2. **Add Account**: Enter your student ID, password, and carrier exit in **设置 → 账号管理 (Settings → Accounts)**, then click **保存并开始守护 (Save & Start)**.
3. **Everyday Use**:
   - **Close the main window (`×`)** anytime to keep monitoring silently in the tray with minimal RAM;
   - **Left-click** the tray icon for the quick flyout, **double-click** for the main window, or **right-click** for the compact menu;
   - Enable **Start at sign-in** under **设置 → 监控与自启** if needed.

### Tray Status Indicators

| <img src="docs/images/tray-online.png" width="22"> Online | <img src="docs/images/tray-checking.png" width="22"> Checking | <img src="docs/images/tray-recovery.png" width="22"> Recovering | <img src="docs/images/tray-offline.png" width="22"> Offline | <img src="docs/images/tray-paused.png" width="22"> Paused |
| :---: | :---: | :---: | :---: | :---: |
| Internet OK | Routine probe | Recheck / Sign-in / Verify | Disconnected or rejected | Monitoring paused |

---

## ❓ FAQ

- **Why are there both "Carrier" (`运营商`) and "Campus" (`校园网`) traffic views?**  
  Carrier broadband exits (Telecom / Unicom / Mobile) are unmetered, whereas the self-service package quota only applies to the Campus exit. The app defaults to your active exit and lets you toggle anytime.
- **Does it conflict with Clash / Mihomo (TUN mode)?**  
  No. All probes, portal logins, and self-service queries bypass system proxies and pierce virtual TUN adapters to identify your real campus IP.
- **Where is my data stored, and how do I upgrade or uninstall?**  
  Passwords are encrypted in Windows Credential Manager; config and logs live in `%LOCALAPPDATA%\GXNUCampusLogin\`. To upgrade, quit the running tray app and replace the `.exe`. To uninstall, disable auto-start, quit the app, and delete the `.exe` and data folder.

---

## 🛠️ Advanced & Docs

<details>
<summary>⌨️ Command-Line Options & Building from Source</summary>

### Command-Line Options

| Option | Description |
| :--- | :--- |
| `--minimized --start` | Launch silently into the system tray and start monitoring immediately |
| `--once` | Run a single read-only diagnostic check (`0` online, `2` offline, `3` route changed) |
| `--autostart on \| off \| status` | Enable, disable, or query startup at Windows sign-in |
| `--test-toast` | Send a test Windows notification |

### Build from Source (Windows x64 + MinGW-w64)

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu --component rustfmt --component clippy
rustup override set stable-x86_64-pc-windows-gnu
.\build.ps1 verify
.\build.ps1 release
```

</details>

- 📖 [Configuration (Chinese)](docs/CONFIGURATION.md) · [Architecture (Chinese)](docs/ARCHITECTURE.md) · [Changelog (Chinese)](CHANGELOG.md)
- 🤝 [Contributing](CONTRIBUTING.md) · [Issues](https://github.com/junkaih2019/gxnu-campus-net-helper/issues) · [Security](SECURITY.md) · [MIT License](LICENSE)
