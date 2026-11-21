//! Windows 构建脚本：使用 MinGW `windres` 将 `assets/app.ico` 嵌入最终的 `gxnu-guard.exe`，
//! 使 Windows 资源管理器、快捷方式与文件属性页都能显示高清应用图标。

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let rc_path = manifest_dir.join("assets").join("app.rc");
    let ico_path = manifest_dir.join("assets").join("app.ico");
    if !rc_path.exists() || !ico_path.exists() {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let obj_path = out_dir.join("app_icon.o");

    let status = Command::new("windres")
        .arg("-I")
        .arg(manifest_dir.join("assets"))
        .arg(&rc_path)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&obj_path)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bin=gxnu-guard={}", obj_path.display());
        }
        Ok(s) => {
            println!(
                "cargo:warning=windres 编译图标资源失败（退出码 {:?}）",
                s.code()
            );
        }
        Err(err) => {
            println!("cargo:warning=未找到 windres，跳过嵌入 exe 图标：{err}");
        }
    }
}
