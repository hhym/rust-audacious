//! audacious-rs 入口：加载配置 → 启动播放引擎 → 启动 audtool 控制服务 → 打开界面。

// 发布版在 Windows 上以 GUI 子系统启动，避免弹出一个黑色控制台窗口。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod effects;
mod engine;
mod ipc_server;
mod liked;
mod login;
mod theme;

use audcore::Config;
use engine::{Cmd, Shared};

fn main() -> eframe::Result<()> {
    #[cfg(target_os = "android")]
    init_android_env();
    init_logging();

    let config = Config::load();
    let shared = Shared::spawn(config.clone());

    // 命令行带上文件/目录/播放列表就直接入列
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = app::paths_from_args(&args);
    if !paths.is_empty() {
        if let Ok(mut pl) = shared.playlist.lock() {
            let added = ipc_server::add_paths(&mut pl, &paths);
            log::info!("命令行添加 {added} 首");
        }
        shared.send(Cmd::Refresh);
    }

    // audtool 的 TCP 控制在手机上没意义，直接不启动
    #[cfg(not(target_os = "android"))]
    let port = ipc_server::serve(shared.clone()).unwrap_or(0);
    #[cfg(target_os = "android")]
    let port = 0u16;

    // 无界面模式：只跑引擎 + 控制服务，方便脚本/CI 验证 audtool 协议
    if args.iter().any(|a| a == "--headless") {
        log::info!("headless 模式，控制端口 {port}，用 audtool 操作或 quit 退出");
        while !shared.is_shutdown() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        shared.send(Cmd::Shutdown);
        return Ok(());
    }

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 720.0])
        .with_min_inner_size([520.0, 130.0])
        .with_icon(AppIcon::build())
        .with_title(format!("Audacious RS {}", audcore::VERSION));

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Audacious RS",
        options,
        Box::new(move |cc| {
            let app = app::App::new(cc, shared, config, port);
            Ok(Box::new(app))
        }),
    )
}

/// 程序图标：用 `assets/icon.png`（橙色圆角「a」），编译时嵌进二进制。
struct AppIcon;

impl AppIcon {
    /// 嵌入的原始 PNG 字节。
    fn bytes() -> &'static [u8] {
        include_bytes!("../assets/icon.png")
    }

    fn build() -> egui::IconData {
        match image::load_from_memory(Self::bytes()) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (width, height) = rgba.dimensions();
                egui::IconData {
                    rgba: rgba.into_raw(),
                    width,
                    height,
                }
            }
            Err(err) => {
                log::warn!("图标解码失败：{err}，退回纯色方块");
                let size = 64usize;
                egui::IconData {
                    rgba: vec![210, 120, 34, 255].repeat(size * size),
                    width: size as u32,
                    height: size as u32,
                }
            }
        }
    }
}

/// 日志：平时打到 stderr；Windows 发布版没有控制台，改写到配置目录下的 app.log。
#[cfg(target_os = "android")]
fn init_android_env() {
    // android-activity 起来时拿不到 `dirs::config_dir()`，这里把 App 私有目录塞进环境变量，
    // `audcore::config::data_dir()` 会优先用它。真机上由 NativeActivity 的 Context 决定，
    // 当前按包名拼默认路径，JNI 版本接上后替换。
    if std::env::var("AUDACIOUS_RS_DATA_DIR").is_err() {
        let dir = "/data/data/io.github.hhym.audaciousrs/files".to_string();
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("AUDACIOUS_RS_DATA_DIR", dir);
    }
}

fn init_logging() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));

    #[cfg(any(all(windows, not(debug_assertions)), target_os = "android"))]
    {
        let dir = audcore::config::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(file) = std::fs::File::create(dir.join("app.log")) {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }

    let _ = builder.try_init();
}
