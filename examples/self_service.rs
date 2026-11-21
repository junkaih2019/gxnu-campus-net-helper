//! GXNU 校园网自助服务 HTTP 探针（直接复用主程序 `src/self_service.rs` 实现）。
//! 运行：`.\build.ps1 run --locked --example self_service`

#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/credentials.rs"]
mod credentials;
#[allow(dead_code)]
#[path = "../src/http.rs"]
mod http;
#[allow(dead_code)]
#[path = "../src/logging.rs"]
mod logging;
#[allow(dead_code)]
#[path = "../src/metrics_store.rs"]
mod metrics_store;
#[allow(dead_code)]
#[path = "../src/models.rs"]
mod models;
#[allow(dead_code)]
#[path = "../src/self_service.rs"]
mod self_service;

use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("自助服务查询失败：{err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let settings = config::SettingsStore::new()
        .load_strict()
        .map_err(|_| "无法读取守护助手的配置")?;
    let account_id = settings
        .active_account()
        .map_or(settings.active_account_id.as_str(), |profile| &profile.id);
    let credential = credentials::load_for_account(account_id, &settings.student_id)?
        .ok_or("当前账号没有已保存的 Windows 凭据")?;

    let snapshot = self_service::fetch_snapshot(&credential).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&snapshot).map_err(|_| "无法生成查询结果 JSON")?;
    println!("{json}");
    Ok(())
}
