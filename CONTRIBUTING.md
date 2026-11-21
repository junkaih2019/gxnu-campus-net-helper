# 参与开发

项目使用 Rust，面向 Windows x64。校园网账号只在使用应用时需要。开发和测试可在普通 Windows 网络环境下进行。

## 本地验证

1. 安装 Rust，通过 rustup 配置 Windows GNU 工具链及检查组件。
2. 安装 64 位 MinGW-w64，确保 `gcc`、`dlltool` 和 `windres` 位于 PATH 中。也可以将 `MINGW_BIN` 指向它们所在的目录。
3. 在仓库根目录用 PowerShell 执行以下命令。

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu --component rustfmt --component clippy
rustup override set stable-x86_64-pc-windows-gnu
.\build.ps1 verify
```

验证脚本依次执行格式检查、Clippy、测试和锁定依赖的 Release 构建。日常开发可以用 `.\build.ps1 test --locked` 单独运行测试。

运行 `.\build.ps1 release` 可生成带中文名称的发行 EXE。该命令会根据 Windows 文件资源中的原始文件名复制 Cargo 产物。

## 修改网络行为

网络诊断、门户认证和监控状态机分别位于 `src/diagnostics.rs`、`src/authenticator.rs` 和 `src/monitor.rs`。修改前请阅读[诊断顺序和状态机](docs/ARCHITECTURE.md)。在回环地址上模拟门户响应，测试账号使用 `demo-student` 等虚构值，示例网络地址使用 `192.0.2.0/24`。

提交涉及登录策略的改动时，请覆盖两类场景：应当触发认证的断线，以及应当跳过认证的网络切换或上游故障。

## 反馈与 Pull Request

报告问题时，请写明应用版本、Windows 版本、复现步骤和期望结果。日志只摘录相关行，并遮盖学号、IP 地址、计算机名和个人目录。配置文件、会话令牌和认证请求可能包含私人信息，请留在本机。

每个 Pull Request 聚焦一个问题，并列出已运行的检查。界面改动可以附使用虚构账号的截图。涉及密码、凭据或会话泄露的问题请按[安全报告说明](SECURITY.md)提交。
