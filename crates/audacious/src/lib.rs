//! 库入口：桌面 `main` 与 Android `android_main` 共用同一套界面/引擎代码。
//!
//! 桌面走 `run()`；Android 走 [`android_main`]（NativeActivity 的入口符号），
//! 两者除了入口和日志/数据目录之外没有任何差别。

pub mod app;
mod skin;
#[cfg(target_os = "android")]
pub mod android;
pub mod effects;
pub mod engine;
pub mod ipc_server;
pub mod liked;
pub mod login;
pub mod theme;

use audcore::Config;
use engine::{Cmd, Shared};

/// 桌面入口（原 `main.rs` 的逻辑）。
pub fn run() -> eframe::Result<()> {
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

    // 截图/占用测试用：AUDACIOUS_WINDOW_SIZE=380x820 覆盖默认窗口尺寸
    let win_size = std::env::var("AUDACIOUS_WINDOW_SIZE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some([w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?])
        })
        .unwrap_or([1180.0, 720.0]);
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size(win_size)
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

/// Android 入口：由 `NativeActivity` 通过 `android_main` 符号拉起。
///
/// 和桌面版的差别只有三处：
/// 1. 数据目录：`Context` 私有目录塞进 `AUDACIOUS_RS_DATA_DIR`，`data_dir()` 认得它；
/// 2. 日志：没有 stderr，走 logcat + 私有目录下的 `app.log`；
/// 3. 没有 `audtool` 控制端口，`port` 传 0。
#[cfg(target_os = "android")]
#[no_mangle]
#[allow(improper_ctypes_definitions)] // AndroidApp 是 #[repr(transparent)] 的指针包装
pub extern "C" fn android_main(app: android_activity::AndroidApp) {
    if let Some(dir) = app.internal_data_path() {
        std::env::set_var("AUDACIOUS_RS_DATA_DIR", dir);
    } else {
        // JNI 没拿到路径时退回 AOSP 的固定私有路径
        std::env::set_var(
            "AUDACIOUS_RS_DATA_DIR",
            "/data/data/io.github.hhym.audaciousrs/files",
        );
    }
    init_logging();
    // SAF 选择器要靠 AndroidApp 拿 JavaVM 与 Activity；导入线程同时起来
    android::install(&app);
    android::start_importer();

    let config = Config::load();
    let shared = Shared::spawn(config.clone());
    // 音频焦点回调要往引擎发命令
    android::install_shared(shared.clone());

    let options = eframe::NativeOptions {
        android_app: Some(app),
        // 手机上没有 1180×720，交给系统按屏幕给窗口
        viewport: egui::ViewportBuilder::default()
            .with_min_inner_size([320.0, 320.0])
            .with_title("Audacious RS"),
        ..Default::default()
    };

    let result = eframe::run_native(
        "Audacious RS",
        options,
        Box::new(move |cc| {
            let app = app::App::new(cc, shared, config, 0);
            Ok(Box::new(app))
        }),
    );

    if let Err(err) = result {
        log::error!("eframe 退出：{err}");
    }
}

/// 程序图标：用 `assets/icon.png`（橙色圆角「a」），编译时嵌进二进制。
pub struct AppIcon;

impl AppIcon {
    /// 嵌入的原始 PNG 字节。
    pub fn bytes() -> &'static [u8] {
        include_bytes!("../assets/icon.png")
    }

    pub fn build() -> egui::IconData {
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

/// 日志：桌面打 stderr（Windows 发布版写文件）；Android 同时写 logcat 和 `app.log`。
fn init_logging() {
    #[cfg(target_os = "android")]
    {
        // `adb logcat -s audacious` 能直接看到
        let _ = android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("audacious"),
        );
    }

    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));

    #[cfg(all(windows, not(debug_assertions)))]
    {
        let dir = audcore::config::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(file) = std::fs::File::create(dir.join("app.log")) {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }

    #[cfg(target_os = "android")]
    {
        // Android 上 stderr 不可见，日志落到私有目录，方便 adb pull 回来
        let dir = audcore::config::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("app.log"))
        {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }

    let _ = builder.try_init();
}
