//! audacious-rs 入口：加载配置 → 启动播放引擎 → 启动 audtool 控制服务 → 打开界面。

mod app;
mod effects;
mod engine;
mod ipc_server;
mod theme;

use audcore::Config;
use engine::{Cmd, Shared};

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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

    let port = ipc_server::serve(shared.clone()).unwrap_or(0);

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
