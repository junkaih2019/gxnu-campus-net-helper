<p align="center">
  <img src="docs/images/app-icon-v2-qinglan.png" width="88" height="88" alt="GXNU 校园网守护助手图标">
</p>
<h1 align="center">GXNU 校园网守护助手</h1>
<p align="center">广西师范大学校园网（Dr.COM）Windows 轻量托盘工具 —— 育才/雁山/王城三校区自动重连、流量与在线设备管理、多账号一键切换</p>
<p align="center"><strong>简体中文</strong> · <a href="README.en.md">English</a></p>
<p align="center">
  <a href="https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest"><strong>⬇️ 下载 Windows x64 单文件版</strong></a>
  &nbsp;·&nbsp; <a href="CHANGELOG.md">更新记录</a>
  &nbsp;·&nbsp; <a href="docs/CONFIGURATION.md">配置说明</a>
  &nbsp;·&nbsp; <a href="docs/ARCHITECTURE.md">架构设计</a>
  &nbsp;·&nbsp; <a href="LICENSE">MIT 许可证</a>
</p>
<p align="center">
  <sub>Windows 10 / 11 x64 · 绿色单文件免安装 · 育才 / 雁山 / 王城三校区通用 · 支持校园网 / 电信 / 联通 / 移动 / 广电出口</sub>
</p>

---

## ✨ 界面总览

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/readme-overview-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/images/readme-overview-light.png">
    <img src="docs/images/readme-overview-light.png" alt="GXNU 校园网守护助手多界面全景预览（主窗口看板、托盘左键浮窗、多账号管理、三校区网关切换与五态托盘）">
  </picture>
</p>

<details>
<summary>🖼️ 展开查看各界面独立高清截图（浅色 / 深色主看板、托盘浮窗、右键菜单与设置中心）</summary>

| 浅色主窗口（运营商视角） | 深色主窗口（运营商视角） |
| :---: | :---: |
| [![浅色主窗口](docs/images/main-window-light.png)](docs/images/main-window-light.png) | [![深色主窗口](docs/images/main-window.png)](docs/images/main-window.png) |

| 校园网额度与多终端管理视图 | 设置中心（多账号与五出口管理） |
| :---: | :---: |
| [![校园网额度与多终端管理视图](docs/images/main-window-campus.png)](docs/images/main-window-campus.png) | [![设置中心多账号管理](docs/images/settings-dark.png)](docs/images/settings-dark.png) |

| 左键单击 · 竖版快捷浮窗 | 右键单击 · 快捷菜单 |
| :---: | :---: |
| [![左键托盘快捷浮窗](docs/images/tray-flyout-light.png)](docs/images/tray-flyout-light.png) | [![右键托盘菜单](docs/images/tray-menu-light.png)](docs/images/tray-menu-light.png) |

</details>

---

## 💡 核心功能与优势

- 🔄 **三校区断网无感重连**：支持**育才 / 雁山 / 王城**多校区认证网关自动探测与跨校区无感切换（亦可手动指定或自定义网关）；睡眠唤醒、插拔网线、切换 Wi-Fi 或认证过期时秒级自动重登，错误原因全中文直观提示。
- 📊 **流量与在线设备管理**：直连校园网自助服务，一键切换 **运营商实际吞吐 / 校园网套餐余量**；实时展示在线终端与近期上网记录浮层，支持在主界面或托盘浮窗**一键注销占线设备**。
- ⚡ **托盘左键快捷浮窗**：单击任务栏图标秒弹原生圆角浮窗，无需打开主窗口即可切账号、看流量、踢占线设备与查看连接稳定性。
- 👥 **多账号 / 双出口秒切 & 防封保护**：支持一键生成共享密码的「宽带 + 校园网」双出口、同号免输新增出口与改密自动同步（保存前自动无损验密）；切换到「校园网」出口前自动提醒关闭 VPN / 代理，防止账号被封禁。
- 📈 **连接稳定性时间轴**：以 `1h / 7h / 24h` 胶囊时间轴记录在线率，悬停黄格即可查看掉线时刻与原因。
- ⬆️ **应用内原位自动更新**：自动检查 GitHub Release 新版本，点击即可完成 SHA-256 校验下载并原位替换重启，无损保留全部账号与自启设置。
- 🛡️ **代理兼容 & 极轻常驻**：开启 Clash / Mihomo（含 TUN 模式）照常直连认证；密码加密存入 Windows 凭据管理器；后台托盘常驻私有内存仅约 **3 MB**，打开主窗口私有内存仅约 **41 MB**（零拷贝映射系统字库），关闭窗口即彻底释放全部 GUI 内存。

---

## 🚀 快速开始

1. **下载运行**：前往 [Releases](https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest) 下载最新单文件 **`gxnu-campus-net-helper-v0.2.6.exe`**，放入**固定文件夹**后直接双击运行（免安装、免解压）。
   > 💡 首次运行若弹出 Windows SmartScreen 提示，点击 **更多信息 → 仍要运行** 即可（个人开源单文件未附商业代码签名，可通过随附 `SHA256SUMS.txt` 校验完整性）。
2. **添加账号**：首次启动会自动打开 **设置 → 账号管理**，在左侧填入校园网学号与密码，右侧点选出口（`校园网 / 电信 / 联通 / 移动 / 广电`，选运营商时可勾选同时生成「校园网」快捷切换项），点击 **验证并添加账号** 即可开始守护；后续为同一学号补充其他出口无需重复输入账密。
3. **日常使用**：
   - **平时直接关闭主窗口（`×`）**，程序会在系统托盘极轻常驻；
   - **左键单击**托盘图标秒开快捷浮窗，**左键双击**打开完整主窗口，**右键单击**呼出快捷菜单；
   - 如需开机自启或手动指定校区网关（默认自动识别育才/雁山/王城），可在 **设置 → 监控与自启** 中调整并保存。

### 托盘五态指示

| <img src="docs/images/tray-v2-online.png" width="24"> 正常在线 | <img src="docs/images/tray-v2-checking.png" width="24"> 检测中 | <img src="docs/images/tray-v2-recovery.png" width="24"> 重连恢复中 | <img src="docs/images/tray-v2-offline.png" width="24"> 连接异常 | <img src="docs/images/tray-v2-paused.png" width="24"> 已暂停 |
| :---: | :---: | :---: | :---: | :---: |
| 公网连通正常 | 正在例行探测 | 复核 / 登录 / 验证中 | 离线或认证被拒 | 自动守护已暂停 |

---

## ❓ 常见问题

- **在育才、雁山、王城不同校区之间移动需要手动改设置吗？**  
  不需要。程序默认开启**「自动探测」**，会自动并行测速候选网关并嗅探当前校区的认证门户，合盖换校区联网也会自动无缝切换；同时内置师大网段与协议白名单校验，在外连接公共 Wi-Fi 或家用路由绝不会误发凭据。若有特殊需求，也可在「设置 → 监控与自启 → 校区与认证网关」手动锁定校区或填写自定义网关地址。
- **为什么账号流量有「运营商」和「校园网」两个切换项？**  
  师大电信/联通/移动/广电宽带出口通常不限流量，而自助服务套餐余量仅统计「校园网」出口额度。程序默认匹配当前账号出口视角，也可随时点击右上角一键切换。
- **开着 Clash / Mihomo 代理（或 TUN 模式）会影响自动登录吗？**  
  在电信/联通/移动/广电出口下不会，探测、认证与自助服务查询均绕过系统代理直连并穿透虚拟网卡识别真实校园网 IP。但使用**「校园网」出口**时请按弹窗提醒先关闭 VPN / 代理软件，避免触发学校安全监测被封号。
- **账号密码存在哪里？如何升级或卸载？**  
  密码由 Windows 凭据管理器加密保存，配置与日志位于 `%LOCALAPPDATA%\GXNUCampusLogin\`。**升级**直接点击主窗口顶栏的新版本提示（或在「设置 → 监控与自启 → 检查更新」）即可一键原位更新；**卸载**只需关闭开机自启、右键托盘退出程序，再删除 `.exe` 与上述目录即可。

---

## 🛠️ 进阶与文档

<details>
<summary>⌨️ 命令行参数与源码构建</summary>

### 常用命令行参数

| 参数 | 说明 |
| :--- | :--- |
| `--minimized --start` | 静默启动到托盘并立即开始守护 |
| `--once` | 执行一次只读网络诊断（退出码：`0` 在线、`2` 离线、`3` 路由变化） |
| `--autostart on \| off \| status` | 开启、关闭或查询开机自启 |
| `--test-toast` | 发送一条测试系统通知 |

### 从源码构建（Windows x64 + MinGW-w64）

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu --component rustfmt --component clippy
rustup override set stable-x86_64-pc-windows-gnu
.\build.ps1 verify
.\build.ps1 release
```

</details>

- 📖 [配置与使用说明](docs/CONFIGURATION.md) · [架构与诊断策略](docs/ARCHITECTURE.md) · [更新记录](CHANGELOG.md)
- 🤝 [参与贡献](CONTRIBUTING.md) · [问题反馈 (Issues)](https://github.com/junkaih2019/gxnu-campus-net-helper/issues) · [安全报告](SECURITY.md) · [MIT 许可证](LICENSE)
