//! 给 Windows 可执行文件嵌入图标和版本信息。
//!
//! 只在交叉/原生编译 Windows 目标时生效；其它平台直接跳过。
//! 需要 PATH 里有 windres（llvm-mingw 提供 `llvm-windres`，
//! GNU mingw 提供 `x86_64-w64-mingw32-windres`）。

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let name = std::env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME");
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let (file, product) = match name.as_str() {
        "audacious-rs" => ("audacious-rs.exe", "Audacious RS"),
        other => (other, other),
    };

    // 4 段版本号，资源编译器要求 FILEVERSION/PRODUCTVERSION 是数字
    let mut parts: Vec<u32> = version
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    parts.resize(4, 0);
    let numeric = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);

    let icon = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("assets/app.ico");
    let icon = icon.to_string_lossy().replace('\\', "/");

    let rc = format!(
        r#"#include <windows.h>
1 ICON "{icon}"

VS_VERSION_INFO VERSIONINFO
 FILEVERSION {numeric}
 PRODUCTVERSION {numeric}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "CompanyName", "hhym"
            VALUE "FileDescription", "{product} — Audacious 的 Rust 重写"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "{file}"
            VALUE "OriginalFilename", "{file}"
            VALUE "ProductName", "{product}"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    );
    let rc_path = std::path::Path::new(&out_dir).join("audacious-res.rc");
    std::fs::write(&rc_path, rc).expect("写 rc 失败");

    let obj = std::path::Path::new(&out_dir).join("audacious-res.o");
    let mut done = false;
    for tool in [
        "llvm-windres",
        "x86_64-w64-mingw32-windres",
        "windres",
        "llvm-rc",
    ] {
        if tool == "llvm-rc" {
            continue;
        }
        let status = Command::new(tool)
            .arg(rc_path.as_os_str())
            .arg("-O")
            .arg("coff")
            .arg("-o")
            .arg(obj.as_os_str())
            .status();
        if let Ok(st) = status {
            if st.success() {
                done = true;
                break;
            }
        }
    }
    if !done {
        println!("cargo:warning=没有可用的 windres，exe 图标和版本信息将缺失");
        return;
    }
    println!("cargo:rustc-link-arg={}", obj.display());
}
