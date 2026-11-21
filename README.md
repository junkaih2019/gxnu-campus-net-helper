# GXNU 校园网守护助手

面向广西师范大学校园网的 Windows 托盘应用。断线后先检查本机网络和校园网会话，再决定是否重新登录。主窗口可查看门户延迟曲线、最近的断网记录和诊断日志。

[下载 Windows x64 版](https://github.com/junkaih2019/gxnu-campus-net-helper/releases/latest) · [配置与常见问题](docs/CONFIGURATION.md) · [更新记录](CHANGELOG.md)

## 界面预览

主窗口展示最近 24 小时的校园网延迟、断网记录和运行日志。图中的账号 `DEMO`、曲线和日志均为演示数据。

![v0.1.1 主窗口，左侧是账号设置，右侧是延迟曲线、两次断网记录和运行日志](docs/images/main-window.png)

右键点击托盘图标可查看守护状态、立即检测或切换监控开关。图中监控已暂停。

<img src="docs/images/tray-menu.png" alt="v0.1.1 托盘菜单，监控已暂停，可选择立即检测或开始监控" width="256">

## 开始使用

1. 在 Releases 下载 `gxnu-campus-net-helper-0.1.1-windows-x64.zip`，解压到固定目录，运行 `GXNU 校园网守护助手.exe`。
2. 在账号页填写自己的校园网学号、运营商和密码，点击 **保存并开始守护**。
3. 关闭主窗口后，守护进程继续在托盘运行。单击托盘图标可以重新打开界面，右键选择 **退出 GXNU 校园网守护助手** 可完全退出。需要随 Windows 启动时，在监控设置中开启 **开机自动启动到托盘**。

从 v0.1.0 升级时，请先从托盘退出旧版，再运行新 EXE。如果原先开启了开机自启，在新程序所在目录运行 `& ".\GXNU 校园网守护助手.exe" --autostart on`，将自启路径更新到新文件。

每个二进制发行包附 SHA-256 校验文件。可以在下载目录运行以下命令，对照校验文件中的值。

```powershell
Get-FileHash .\gxnu-campus-net-helper-0.1.1-windows-x64.zip -Algorithm SHA256
```

## 掉线时会发生什么

| 检测结果 | 守护行为 |
| --- | --- |
| 从 Wi-Fi 切到有线，或电脑刚从睡眠恢复 | 等出口稳定后复查，短暂切换不会触发登录。 |
| 校园网会话仍在线，但公网探测失败 | 显示出口或上游故障，等待网络恢复。 |
| 门户报告离线，或检测到认证重定向 | 复核后尝试登录，再次探测公网以确认恢复。 |
| 门户连续拒绝登录 | 提示核对账号和运营商，逐步延长重试间隔，最长 5 分钟。 |
| 监控已暂停时点击「立即检测」 | 执行一次只读诊断。 |

界面提供最近 1、7 或 24 小时的门户延迟和断网记录。探测和认证使用直连请求。完整判定条件见[网络诊断与状态机](docs/ARCHITECTURE.md)。

## 常用命令

| PowerShell 命令 | 用途 |
| --- | --- |
| `& ".\GXNU 校园网守护助手.exe" --once` | 只读检测。退出码 0 表示在线，2 表示离线，3 表示连续采样时出口发生变化。 |
| `& ".\GXNU 校园网守护助手.exe" --minimized` | 启动后驻留托盘。 |
| `& ".\GXNU 校园网守护助手.exe" --minimized --start` | 在后台开始或恢复监控。 |
| `& ".\GXNU 校园网守护助手.exe" --test-toast` | 发送测试系统通知。 |
| `& ".\GXNU 校园网守护助手.exe" --autostart status` | 查询开机自启状态。 |

## 数据与隐私

密码保存在当前用户的 Windows 凭据管理器中。学号和偏好写入 `%LOCALAPPDATA%\GXNUCampusLogin\config.json`，同目录还保存日志、界面偏好和最近的网络记录。提交问题时请只摘录相关日志，并遮盖设备和网络信息。[文件与配置字段](docs/CONFIGURATION.md)

## 从源码构建

在 Windows x64 上安装 Rust 和 MinGW-w64。将 `gcc`、`dlltool`、`windres` 所在目录加入 PATH，在仓库根目录运行以下命令。

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu --component rustfmt --component clippy
rustup override set stable-x86_64-pc-windows-gnu
.\build.ps1 verify
.\build.ps1 release
```

`verify` 执行格式检查、Clippy、全量测试和 Release 构建，`release` 输出发行文件 `target\release\GXNU 校园网守护助手.exe`。参与开发可参考[贡献指南](CONTRIBUTING.md)，安全问题请参阅[私下报告方式](SECURITY.md)。

本项目使用 [MIT 许可证](LICENSE)。
