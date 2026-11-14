//! audacious-rs 入口：加载配置 → 启动播放引擎 → 启动 audtool 控制服务 → 打开界面。

// 发布版在 Windows 上以 GUI 子系统启动，避免弹出一个黑色控制台窗口。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod effects;
mod engine;
mod ipc_server;
mod theme;

use audcore::Config;
use engine::{Cmd, Shared};

fn main() -> eframe::Result<()> {
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

/// 程序图标：代码里现画一个 64×64 的琥珀色圆角方块加白色音符，
/// 免得打包时还要额外塞一个 .ico 资源。
struct AppIcon;

impl AppIcon {
    fn build() -> egui::IconData {
        let size = 64usize;
        let mut rgba = vec![0u8; size * size * 4];
        let radius = 12.0f32;

        for y in 0..size {
            for x in 0..size {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;

                // 圆角矩形遮罩
                let cx = fx.clamp(radius, size as f32 - radius);
                let cy = fy.clamp(radius, size as f32 - radius);
                let dist = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                let alpha = if dist > radius {
                    0.0
                } else if dist > radius - 1.5 {
                    (radius - dist) / 1.5
                } else {
                    1.0
                };

                // 竖向渐变：上亮下暗
                let t = fy / size as f32;
                let r = (214.0 - 28.0 * t) as u8;
                let g = (132.0 - 26.0 * t) as u8;
                let b = (36.0 - 12.0 * t) as u8;

                let idx = (y * size + x) * 4;
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = (alpha * 255.0) as u8;

                // 白色的四分音符：符头椭圆 + 符干 + 符尾
                let note = Self::note_alpha(fx, fy);
                if note > 0.0 {
                    let a = (note * 255.0) as u8;
                    let keep = (rgba[idx + 3] as f32 / 255.0).min(1.0);
                    let a = (a as f32 * keep) as u8;
                    rgba[idx] = (r as f32 * (1.0 - note) + 255.0 * note) as u8;
                    rgba[idx + 1] = (g as f32 * (1.0 - note) + 255.0 * note) as u8;
                    rgba[idx + 2] = (b as f32 * (1.0 - note) + 255.0 * note) as u8;
                    rgba[idx + 3] = a.max(rgba[idx + 3]);
                }
            }
        }

        egui::IconData {
            rgba,
            width: size as u32,
            height: size as u32,
        }
    }

    /// 音符形状的覆盖透明度（0..1），坐标基于 64×64
    fn note_alpha(x: f32, y: f32) -> f32 {
        // 符头：略倾斜的实心椭圆
        let (cx, cy, rx, ry) = (21.0f32, 45.0f32, 11.5f32, 8.5f32);
        let dx = (x - cx) / rx;
        let dy = (y - cy) / ry;
        let head = 1.0 - (dx * dx + dy * dy);
        if head > 0.0 {
            return head.sqrt().min(1.0) * 3.0_f32.min(1.0);
        }

        // 符干：一根 3px 竖条
        if (29.0..=32.0).contains(&x) && (17.0..=45.0).contains(&y) {
            return 1.0;
        }

        // 符尾：从符干顶端向右下的弧形
        let sx = (x - 33.0) / 9.0;
        let sy = (y - 21.0) / 7.0;
        if x >= 32.0 && sx * sx + sy * sy <= 1.0 && y >= 15.0 && y <= 28.0 {
            return 1.0;
        }

        0.0
    }
}

/// 日志：平时打到 stderr；Windows 发布版没有控制台，改写到配置目录下的 app.log。
fn init_logging() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));

    #[cfg(all(windows, not(debug_assertions)))]
    {
        let dir = dirs::config_dir()
            .map(|d| d.join("audacious-rs"))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(file) = std::fs::File::create(dir.join("app.log")) {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }

    let _ = builder.try_init();
}
