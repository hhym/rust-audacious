//! 桌面入口：真正的逻辑都在 `lib.rs`（Android 的 `android_main` 复用同一套代码）。

// 发布版在 Windows 上以 GUI 子系统启动，避免弹出一个黑色控制台窗口。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> eframe::Result<()> {
    audacious_rs::run()
}
