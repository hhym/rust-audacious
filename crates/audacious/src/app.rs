//! 主界面：仿 Audacious GTK 界面的 egui 实现。
//!
//! 布局自上而下：菜单栏 / 工具栏（播放控制 + 进度 + 音量）/ 播放列表表格 /
//! Now Playing 信息带（封面 + 频谱）/ 状态栏；迷你模式下只保留信息带。

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use audcore::{plugin, Config, Playlist, RepeatMode};
use egui::{
    Align2, Color32, CornerRadius, FontId, RichText, Sense, Stroke, StrokeKind, vec2,
};

use crate::engine::{Cmd, PlaybackState, Shared, Status};
use crate::ipc_server;
use crate::liked::{Liked, LikedItem};
use crate::theme::{self, Palette};

pub struct Browser {
    dir: PathBuf,
    entries: Vec<(PathBuf, bool)>,
    selected: Option<usize>,
}

/// 「选 zip / 选文件夹」用的小浏览窗口。
pub struct PluginPick {
    dir: PathBuf,
    entries: Vec<(PathBuf, bool)>,
    selected: Option<usize>,
    /// true：只列目录和 .zip；false：只列目录（装文件夹用）
    want_zip: bool,
}

impl PluginPick {
    fn new(dir: PathBuf, want_zip: bool) -> Self {
        let mut pick = PluginPick {
            dir,
            entries: Vec::new(),
            selected: None,
            want_zip,
        };
        pick.refresh();
        pick
    }

    fn refresh(&mut self) {
        self.entries.clear();
        self.selected = None;
        let Ok(read_dir) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if !self.want_zip || path.join("plugin.json").is_file() || true {
                    dirs.push((path, true));
                }
            } else if self.want_zip
                && path.extension().map(|e| e.eq_ignore_ascii_case("zip")).unwrap_or(false)
            {
                files.push((path, false));
            }
        }
        dirs.sort();
        files.sort();
        self.entries.extend(dirs);
        self.entries.extend(files);
    }

    fn go_up(&mut self) {
        if let Some(parent) = self.dir.parent() {
            self.dir = parent.to_path_buf();
            self.refresh();
        }
    }
}

impl Browser {
    fn new(dir: PathBuf) -> Self {
        let mut b = Browser {
            dir,
            entries: Vec::new(),
            selected: None,
        };
        b.refresh();
        b
    }

    fn refresh(&mut self) {
        self.entries.clear();
        self.selected = None;
        let Ok(read_dir) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push((path, true));
            } else if audcore::tags::is_audio_file(&path) || plugin::playlist_plugin_for(&path).is_some()
            {
                files.push((path, false));
            }
        }
        dirs.sort();
        files.sort();
        self.entries.extend(dirs);
        self.entries.extend(files);
    }

    fn go_up(&mut self) {
        if let Some(parent) = self.dir.parent() {
            self.dir = parent.to_path_buf();
            self.refresh();
        }
    }

    fn enter(&mut self, index: usize) {
        if let Some((path, is_dir)) = self.entries.get(index) {
            if *is_dir {
                self.dir = path.clone();
                self.refresh();
            }
        }
    }
}

pub struct App {
    shared: Shared,
    config: Config,
    state: PlaybackState,
    playlist: Playlist,
    spectrum: Vec<f32>,
    last_spectrum: Instant,
    has_cjk: bool,
    browser: Option<Browser>,
    show_settings: bool,
    show_eq: bool,
    show_plugins: bool,
    show_about: bool,
    toast: Option<(String, Instant)>,
    port: u16,
    style_key: (audcore::Theme, [u8; 3]),
    mini_applied: bool,
    /// 主题切换菜单是否展开
    theme_menu_open: bool,
    /// 主题按钮的位置（菜单贴着它弹出）
    theme_button_rect: Option<egui::Rect>,
    plugins: Vec<audcore::plugin::PluginInfo>,
    /// 在线插件（MusicFree 标准）
    media_plugins: Vec<audplugins::OnlinePlugin>,
    /// 后台扫描插件用的接收端（扫描不阻塞界面）
    scan_rx: Option<std::sync::mpsc::Receiver<Vec<audplugins::OnlinePlugin>>>,
    show_search: bool,
    search_plugin: usize,
    /// 插件选择弹层是否打开
    plugin_picker_open: bool,
    /// 弹层是从哪个选择器打开的（nav / search），避免两处同帧都画
    plugin_picker_source: &'static str,
    /// 插件选择弹层的页码（每页 16 个）
    plugin_page: usize,
    /// 安装插件的下载地址输入框
    plugin_url_input: String,
    /// 「选 zip / 选文件夹」的浏览窗口
    plugin_pick: Option<PluginPick>,
    /// 安装结果提示：成功与否 + 文案
    plugin_install_msg: Option<(bool, String)>,
    /// 后台下载安装
    install_rx: Option<Receiver<Result<audplugins::install::InstalledPlugin, String>>>,
    /// MyGo 插件设置项的编辑缓冲：`插件id/key` → 当前文本
    mygo_setting_inputs: std::collections::HashMap<String, String>,
    search_query: String,
    search_results: Vec<serde_json::Value>,
    search_status: SearchStatus,
    search_rx: Option<Receiver<Result<Vec<serde_json::Value>, String>>>,
    /// 在线歌曲解析结果：item + 播放地址
    resolve_rx: Option<
        Receiver<Result<(serde_json::Value, audplugins::MediaSource, bool, usize), String>>,
    >,
    /// 主面板当前标签页
    tab: MainTab,
    /// 「喜欢」列表
    liked: Liked,
    /// 扫码登录窗口
    login: crate::login::LoginState,
    /// 当前插件的主页内容
    home: Option<audplugins::HomePage>,
    /// 这份主页属于哪个插件
    home_for: Option<String>,
    /// 后台加载主页
    home_rx: Option<std::sync::mpsc::Receiver<Result<audplugins::HomePage, String>>>,
    home_error: Option<String>,
    /// 歌曲页多选
    song_select: std::collections::BTreeSet<usize>,
    /// Shift 连选的起点
    song_anchor: Option<usize>,
    /// 按专辑视图里正在看哪张专辑
    album_filter: String,
    /// 当前在线曲目（插件 + 原始 item），歌词从这里取
    current_online: Option<(usize, serde_json::Value)>,
    /// 歌词加载结果对应的曲目 key
    lyric_key: String,
    lyric_state: LyricState,
    lyric_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    lyric_active: usize,
    /// 侧栏 / 关于页用的应用图标纹理
    logo_texture: Option<egui::TextureHandle>,
    /// 按专辑名缓存的封面缩略图（`None` 表示这轮已经试过、没读到封面）
    album_art: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    /// 迷你播放器用的圆形封面
    album_art_round: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    /// 按专辑缓存「封面主色」，`None` 表示这张封面挑不出合适的颜色
    album_accent: std::collections::HashMap<String, Option<[u8; 3]>>,
    /// 当前生效的封面色（`config.cover_accent` 打开时才有）
    cover_accent_rgb: Option<[u8; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
enum LyricState {
    Idle,
    Loading,
    Lines(Vec<(f64, String)>),
    Plain(String),
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MainTab {
    Home,
    Search,
    Liked,
    Playlists,
    Songs,
    Lyrics,
    Album,
}

#[derive(Clone, Debug, PartialEq)]
enum SearchStatus {
    Idle,
    Busy,
    Error(String),
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        shared: Shared,
        config: Config,
        port: u16,
    ) -> Self {
        let has_cjk = theme::install_fonts(&cc.egui_ctx);
        theme::apply_style(&cc.egui_ctx, &config);

        let mut app = App {
            state: shared.state(),
            playlist: shared.playlist(),
            shared,
            config: config.clone(),
            spectrum: vec![0.0; 16],
            last_spectrum: Instant::now(),
            has_cjk,
            browser: None,
            show_settings: false,
            show_eq: false,
            show_plugins: false,
            show_about: false,
            toast: None,
            port,
            style_key: (config.theme, config.accent),
            mini_applied: !config.mini,
            theme_menu_open: false,
            theme_button_rect: None,
            plugins: Vec::new(),
            media_plugins: Vec::new(),
            scan_rx: None,
            show_search: false,
            search_plugin: 0,
            plugin_picker_open: false,
            plugin_picker_source: "nav",
            plugin_page: 0,
            plugin_url_input: String::new(),
            plugin_pick: None,
            plugin_install_msg: None,
            install_rx: None,
            mygo_setting_inputs: std::collections::HashMap::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            search_status: SearchStatus::Idle,
            search_rx: None,
            resolve_rx: None,
            tab: MainTab::Songs,
            liked: Liked::load(),
            login: crate::login::LoginState::default(),
            home: None,
            home_for: None,
            home_rx: None,
            home_error: None,
            song_select: std::collections::BTreeSet::new(),
            song_anchor: None,
            album_filter: String::new(),
            current_online: None,
            lyric_key: String::new(),
            lyric_state: LyricState::Idle,
            lyric_rx: None,
            lyric_active: 0,
            logo_texture: None,
            album_art: std::collections::HashMap::new(),
            album_art_round: std::collections::HashMap::new(),
            album_accent: std::collections::HashMap::new(),
            cover_accent_rgb: None,
        };
        app.plugins = app.collect_plugins();
        app.reload_media_plugins();

        // 调试用：AUDACIOUS_DEBUG_WINDOWS=eq,settings,plugins,browser,about
        // 启动时打开指定窗口，方便截图与自动化验收。
        if let Ok(list) = std::env::var("AUDACIOUS_DEBUG_WINDOWS") {
            for name in list.split(',') {
                match name.trim() {
                    "eq" => app.show_eq = true,
                    "settings" => app.show_settings = true,
                    "plugins" => app.show_plugins = true,
                    "search" => app.show_search = true,
                    "about" => app.show_about = true,
                    "browser" => {
                        let dir = app
                            .config
                            .last_dir
                            .clone()
                            .unwrap_or_else(|| PathBuf::from("."));
                        app.browser = Some(Browser::new(dir));
                    }
                    _ => {}
                }
            }
        }

        app
    }

    fn collect_plugins(&self) -> Vec<audcore::plugin::PluginInfo> {
        use audcore::plugin::PluginKind;
        let mut list = vec![
            audcore::plugin::PluginInfo::new(
                "Symphonia 解码器",
                PluginKind::Input,
                "MP3 / FLAC / Ogg Vorbis / Opus / WAV / AAC / Matroska",
            ),
            audcore::plugin::PluginInfo::new(
                "Rodio 音频输出",
                PluginKind::Output,
                "cpal 后端：WASAPI / ALSA / CoreAudio，支持 seek 与音量",
            ),
            audcore::plugin::PluginInfo::new(
                "十段均衡器",
                PluginKind::Effect,
                "每段 ±24dB 的 peaking biquad，播放中实时生效",
            ),
            audcore::plugin::PluginInfo::new(
                "频谱分析",
                PluginKind::Visualization,
                "采样旁路 + Goertzel 算法，16 段 log 频谱",
            ),
            audcore::plugin::PluginInfo::new(
                "标签读取",
                PluginKind::Input,
                "lofty：标题 / 艺人 / 专辑 / 时长",
            ),
        ];
        for p in plugin::playlist_plugins() {
            list.push(audcore::plugin::PluginInfo::new(
                p.name(),
                PluginKind::Playlist,
                &format!("支持 {}", p.extensions().join(" / ")),
            ));
        }
        list
    }

    fn t(&self, zh: &'static str, en: &'static str) -> &'static str {
        if self.has_cjk {
            zh
        } else {
            en
        }
    }

    /// 后台加载当前插件的主页，加载完更新 `home`。
    fn poll_home(&mut self) {
        let Some(rx) = &self.home_rx else { return };
        match rx.try_recv() {
            Ok(result) => {
                self.home_rx = None;
                match result {
                    Ok(page) => {
                        self.home = Some(page);
                        self.home_error = None;
                    }
                    Err(err) => {
                        self.home = None;
                        self.home_error = Some(err);
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.home_rx = None;
            }
        }
    }

    /// 真正去跑插件 `home()`（可能在网络里卡住，所以放后台）。
    fn load_home(&mut self) {
        let Some(plugin) = self.media_plugins.get(self.search_plugin).cloned() else {
            self.home = None;
            self.home_for = None;
            return;
        };
        let name = plugin.name().to_string();
        self.home_for = Some(name.clone());
        self.home = None;
        self.home_error = None;
        if !plugin.supports_home() {
            self.home_rx = None;
            self.home_error = Some(format!("{} 没有首页接口", name));
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = plugin.home().map_err(|e| audplugins::friendly_error(&e.to_string()));
            let _ = tx.send(result);
        });
        self.home_rx = Some(rx);
    }

    /// 按插件 id 找 MyGo 插件并打开扫码登录窗口。
    fn open_login(&mut self, id: &str) {
        let found = self
            .media_plugins
            .iter()
            .filter_map(|pl| pl.as_mygo())
            .find(|mygo| mygo.manifest.id == id);
        match found {
            Some(mygo) => {
                if mygo.supports_login() {
                    self.login.open(mygo.clone());
                } else if mygo.supports_sign_in() {
                    self.login.open_sign_in(mygo.clone());
                } else {
                    self.toast("这个插件没有实现扫码登录（loginQr / loginPoll / signIn）");
                }
            }
            None => self.toast("找不到该插件，先点「重新扫描」"),
        }
    }

    fn toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), Instant::now()));
    }

    fn sync(&mut self) {
        self.state = self.shared.state();
        self.playlist = self.shared.playlist();
        if self.last_spectrum.elapsed() >= Duration::from_millis(50) {
            self.last_spectrum = Instant::now();
            let fresh = self
                .shared
                .spectrum
                .lock()
                .map(|s| s.bands(16, self.state.sample_rate))
                .unwrap_or_else(|_| vec![0.0; 16]);
            for (i, v) in fresh.iter().enumerate() {
                let old = self.spectrum.get(i).copied().unwrap_or(0.0);
                let decayed = (old - 0.08).max(0.0);
                let value = if *v > decayed { *v } else { decayed };
                if i < self.spectrum.len() {
                    self.spectrum[i] = value;
                } else {
                    self.spectrum.push(value);
                }
            }
        }
    }

    fn add_paths(&mut self, paths: &[PathBuf]) {
        let added = {
            let Ok(mut pl) = self.shared.playlist.lock() else {
                return;
            };
            ipc_server::add_paths(&mut pl, paths)
        };
        self.shared.send(Cmd::Refresh);
        if added > 0 {
            self.toast(format!("已添加 {added} 首"));
        } else {
            self.toast("没有可添加的音频文件");
        }
    }

    fn save_playlist(&mut self) {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(audcore::APP_NAME)
            .join("playlists")
            .join(format!("playlist-{stamp}.m3u"));
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let entries = self.playlist.entries.clone();
        match plugin::save_playlist(&path, &entries) {
            Ok(()) => self.toast(format!("已保存到 {}", path.display())),
            Err(err) => self.toast(format!("保存失败：{err}")),
        }
    }

}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync();
        self.refresh_cover_accent();
        self.poll_media_channels();
        self.poll_home();
        self.login.tick(ctx);

        if self.config.mini != self.mini_applied {
            self.mini_applied = self.config.mini;
            let size = if self.config.mini {
                vec2(420.0, 140.0)
            } else {
                vec2(1180.0, 720.0)
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }

        let p = self.pal();
        if self.style_key != (self.config.theme, self.config.accent) {
            self.style_key = (self.config.theme, self.config.accent);
            theme::apply_style(ctx, &self.config);
        }

        // 拖拽文件到窗口
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            let mut audio_paths: Vec<PathBuf> = Vec::new();
            for path in dropped {
                let installable = path
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("zip"))
                    .unwrap_or(false)
                    || (path.is_dir() && path.join("plugin.json").is_file());
                if installable {
                    match audplugins::install::install_from_path(&path) {
                        Ok(plugin) => {
                            self.plugin_install_msg = Some((
                                true,
                                format!("已安装《{}》", plugin.name),
                            ));
                            self.reload_media_plugins();
                        }
                        Err(err) => {
                            self.plugin_install_msg =
                                Some((false, format!("安装失败：{err}")));
                        }
                    }
                } else {
                    audio_paths.push(path);
                }
            }
            if !audio_paths.is_empty() {
                self.add_paths(&audio_paths);
            }
        }

        // 快捷键：空格播放/暂停，左右切歌
        if !ctx.wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                self.shared.send(Cmd::Toggle);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                self.shared.send(Cmd::Next);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                self.shared.send(Cmd::Prev);
            }
        }

        if self.config.mini {
            self.mini_ui(ctx, &p);
        } else {
            self.menubar_ui(ctx, &p);
            self.toolbar_ui(ctx, &p);
            self.statusbar_ui(ctx, &p);
            self.nowband_ui(ctx, &p);
            self.main_panel(ctx, &p);
        }

        self.theme_menu_ui(ctx, &p);
        self.settings_ui(ctx, &p);
        self.eq_ui(ctx, &p);
        self.plugins_ui(ctx, &p);
        self.search_ui(ctx, &p);
        self.about_ui(ctx, &p);
        if let Some(msg) = self.login.ui(ctx, &p, self.has_cjk) {
            self.toast(msg);
        }

        self.plugin_pick_ui(ctx, &p);
        let pending = self.browser_ui(ctx, &p);
        if !pending.is_empty() {
            self.add_paths(&pending);
        }

        if self.shared.is_shutdown() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // 50ms 一帧，频谱与进度足够顺滑，又不至于空转
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

impl App {
    fn menubar_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        egui::TopBottomPanel::top("menubar")
            .exact_height(34.0)
            .frame(egui::Frame::NONE.fill(p.panel_alt))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("AUDACIOUS · RUST")
                            .color(p.accent)
                            .strong()
                            .size(13.5),
                    );
                    ui.add_space(6.0);
                    ui.separator();
                    if icon_button(ui, self.t("添加文件", "Add files"), p, icon_file_add) {
                        self.browser = Some(Browser::new(
                            self.config
                                .last_dir
                                .clone()
                                .unwrap_or_else(|| PathBuf::from(".")),
                        ));
                    }
                    if icon_button(ui, self.t("添加目录", "Add folder"), p, icon_folder_add) {
                        let dir = self
                            .config
                            .last_dir
                            .clone()
                            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
                        self.add_paths(&[dir]);
                    }
                    if icon_button(ui, self.t("保存列表", "Save list"), p, icon_save_list) {
                        self.save_playlist();
                    }
                    ui.separator();
                    if icon_button(ui, self.t("均衡器", "Equalizer"), p, icon_eq) {
                        self.show_eq = true;
                    }
                    if icon_button(ui, self.t("插件", "Plugins"), p, icon_plugins) {
                        self.show_plugins = true;
                    }
                    if icon_button(ui, self.t("设置", "Settings"), p, icon_settings) {
                        self.show_settings = true;
                    }
                    ui.separator();
                    if icon_button(ui, self.t("迷你", "Mini"), p, icon_mini) {
                        self.config.mini = true;
                        let _ = self.config.save();
                    }
                    if icon_button(ui, self.t("关于", "About"), p, icon_info) {
                        self.show_about = true;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if let Some((msg, at)) = &self.toast {
                            if at.elapsed() < Duration::from_secs(4) {
                                ui.label(RichText::new(msg).color(p.accent));
                            }
                        }
                    });
                });
            });
    }

    fn toolbar_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        egui::TopBottomPanel::top("toolbar")
            .exact_height(52.0)
            .frame(egui::Frame::NONE.fill(p.panel))
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let playing = self.state.status == Status::Playing;
                    if icon_button(ui, self.t("上一首", "Prev"), p, mini_prev_icon) {
                        self.shared.send(Cmd::Prev);
                    }
                    if transport_button(
                        ui,
                        p,
                        if playing {
                            self.t("暂停", "Pause")
                        } else {
                            self.t("播放", "Play")
                        },
                        playing,
                    ) {
                        self.shared.send(Cmd::Toggle);
                    }
                    if icon_button(ui, self.t("停止", "Stop"), p, icon_stop) {
                        self.shared.send(Cmd::Stop);
                    }
                    if icon_button(ui, self.t("下一首", "Next"), p, mini_next_icon) {
                        self.shared.send(Cmd::Next);
                    }

                    ui.add_space(8.0);
                    let shuffle_label = self.t("随机", "Shuffle");
                    let mut shuffle = self.playlist.shuffle;
                    if ui
                        .add(
                            egui::Button::new(RichText::new(shuffle_label).color(if shuffle {
                                p.accent
                            } else {
                                p.dim
                            }))
                            .fill(if shuffle { p.accent_soft } else { p.panel_alt }),
                        )
                        .clicked()
                    {
                        shuffle = !shuffle;
                        if let Ok(mut pl) = self.shared.playlist.lock() {
                            pl.shuffle = shuffle;
                        }
                        self.config.shuffle = shuffle;
                        let _ = self.config.save();
                    }
                    let repeat = self.playlist.repeat;
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(repeat.label()).color(if repeat
                                    != RepeatMode::Off
                                {
                                    p.accent
                                } else {
                                    p.dim
                                }),
                            )
                            .fill(if repeat != RepeatMode::Off {
                                p.accent_soft
                            } else {
                                p.panel_alt
                            }),
                        )
                        .clicked()
                    {
                        let next = repeat.cycle();
                        if let Ok(mut pl) = self.shared.playlist.lock() {
                            pl.repeat = next;
                        }
                        self.config.repeat = next;
                        let _ = self.config.save();
                    }

                    ui.add_space(12.0);
                    let len = self.state.length;
                    let mut pos = self.state.position.min(len.max(0.0));
                    let slider_w = (ui.available_width() - 250.0).max(160.0);
                    let resp = ui.add_sized(
                        [slider_w, 18.0],
                        egui::Slider::new(&mut pos, 0.0..=len.max(1.0)).show_value(false),
                    );
                    if resp.changed() {
                        self.shared.send(Cmd::Seek(pos));
                    }
                    ui.label(
                        RichText::new(format!(
                            "{} / {}",
                            theme::time_text(self.state.position),
                            theme::time_text(len)
                        ))
                        .monospace()
                        .color(p.dim),
                    );

                    ui.add_space(8.0);
                    let mut volume = self.config.volume;
                    if ui
                        .add_sized(
                            [90.0, 18.0],
                            egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false),
                        )
                        .changed()
                    {
                        self.config.volume = volume;
                        self.shared.send(Cmd::SetVolume(volume));
                    }
                    if ui.button(self.t("音量", "Vol")).clicked() {
                        let muted = if self.config.volume > 0.0 {
                            0.0
                        } else {
                            self.state.volume.max(0.5)
                        };
                        self.config.volume = muted;
                        self.shared.send(Cmd::SetVolume(muted));
                        let _ = self.config.save();
                    }

                    // 右下角：主题切换按钮（衣服图标，点一下换一套皮肤）
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.add_space(10.0);
                            let (rect, resp) =
                                ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::click());
                            if resp.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    egui::CornerRadius::same(6),
                                    p.accent_soft,
                                );
                            }
                            paint_shirt(ui.painter(), rect, p.accent, p.panel);
                            let resp = resp.on_hover_text(self.t(
                                "切换主题（橘黄 / 浅色 / 深色）",
                                "Switch theme (amber / light / dark)",
                            ));
                            if resp.clicked() {
                                self.theme_menu_open = !self.theme_menu_open;
                            }
                            self.theme_button_rect = Some(rect);

                            let current = if self.has_cjk {
                                self.config.theme.label()
                            } else {
                                self.config.theme.label_en()
                            };
                            ui.label(RichText::new(current).small().color(p.dim));
                        },
                    );
                });
            });
    }

    fn statusbar_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        egui::TopBottomPanel::bottom("statusbar")
            .exact_height(24.0)
            .frame(egui::Frame::NONE.fill(p.panel_alt))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(8.0);
                    let format = if self.state.format.is_empty() {
                        "—".to_string()
                    } else {
                        self.state.format.clone()
                    };
                    ui.label(
                        RichText::new(format!(
                            "{} · {} {} · {}",
                            format,
                            self.t("队列", "Queue"),
                            self.playlist.len(),
                            theme::time_text(self.playlist.total_duration())
                        ))
                        .small()
                        .color(p.dim),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        let port = if self.port > 0 {
                            self.port.to_string()
                        } else {
                            "—".into()
                        };
                        ui.label(
                            RichText::new(format!(
                                "v{} · audtool :{} · {}",
                                audcore::VERSION,
                                port,
                                self.state.status.label()
                            ))
                            .small()
                            .color(p.dim),
                        );
                    });
                });
            });
    }

    #[allow(dead_code)]
    fn playlist_panel(&mut self, ctx: &egui::Context, p: &Palette) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.bg))
            .show(ctx, |ui| {
                self.playlist_table(ui, p);
            });
    }

    /// 歌曲页顶部：全选 / 取消选择 / 删除所选。
    fn songs_toolbar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let total = self.playlist.len();
        let selected = self.song_select.len();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                RichText::new(format!("{} {}", total, self.t("首", "songs")))
                    .small()
                    .color(p.dim),
            );
            ui.separator();
            if ui
                .add_enabled(total > 0, egui::Button::new(self.t("全选", "Select all")))
                .clicked()
            {
                self.song_select = (0..total).collect();
                self.song_anchor = Some(0);
            }
            if ui
                .add_enabled(selected > 0, egui::Button::new(self.t("取消选择", "Clear")))
                .clicked()
            {
                self.song_select.clear();
            }
            let delete_text = if selected == 0 {
                self.t("删除所选", "Delete selected").to_string()
            } else {
                format!("{} ({selected})", self.t("删除所选", "Delete selected"))
            };
            if ui
                .add_enabled(selected > 0, egui::Button::new(delete_text))
                .clicked()
            {
                let indexes: Vec<usize> = self.song_select.iter().rev().copied().collect();
                if let Ok(mut pl) = self.shared.playlist.lock() {
                    for index in indexes {
                        pl.remove(index);
                    }
                }
                self.song_select.clear();
                self.shared.send(Cmd::Refresh);
            }
            if selected > 0 {
                ui.label(
                    RichText::new(self.t(
                        "Ctrl / Shift 点行可多选，Shift 连选",
                        "Ctrl/Shift-click rows to multi-select",
                    ))
                    .small()
                    .color(p.dim),
                );
            }
        });
        ui.add_space(4.0);
    }

    fn playlist_table(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let entries = self.playlist.entries.clone();
        let total = ui.available_width();
        let widths = column_widths(total);
        let header_h = 26.0;
        let row_h = 23.0;

        // 表头
        let (rect, _) = ui.allocate_exact_size(vec2(total, header_h), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::ZERO, p.panel_alt);
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0_f32, p.accent),
        );
        let labels = [
            "#".to_string(),
            self.t("标题", "Title").to_string(),
            self.t("艺人", "Artist").to_string(),
            self.t("专辑", "Album").to_string(),
            self.t("时长", "Time").to_string(),
            self.t("格式", "Format").to_string(),
        ];
        let painter = ui.painter().with_clip_rect(rect);
        let mut x = rect.left() + 8.0;
        for (i, label) in labels.iter().enumerate() {
            painter.text(
                egui::pos2(x, rect.center().y),
                Align2::LEFT_CENTER,
                theme::fit_text(label, widths[i], 12.0),
                FontId::proportional(12.0),
                p.dim,
            );
            x += widths[i];
        }

        if entries.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(self.t(
                        "列表是空的 —— 拖文件进窗口，或点左上角「添加文件」",
                        "Playlist is empty — drop files here or use Add files",
                    ))
                    .color(p.dim),
                );
            });
            return;
        }

        let selected = self.playlist.selected;
        let playing = self.playlist.playing;
        let multi = self.song_select.clone();
        let anchor = self.song_anchor;
        let mut play_request = None;
        let mut selection = None;
        let mut extend_select: Option<(usize, bool, bool)> = None;
        let mut menu_action: Option<(usize, MenuAction)> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, entry) in entries.iter().enumerate() {
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(total, row_h), Sense::click());
                    let painter = ui.painter();

                    let zebra = if index % 2 == 1 {
                        p.panel_alt
                    } else {
                        p.bg
                    };
                    painter.rect_filled(rect, CornerRadius::ZERO, zebra);
                    if selected == Some(index) || multi.contains(&index) {
                        painter.rect_filled(rect, CornerRadius::ZERO, p.accent_soft);
                    }
                    if resp.hovered() {
                        let fill = if p.dark {
                            Color32::from_rgba_unmultiplied(255, 255, 255, 12)
                        } else {
                            Color32::from_rgba_unmultiplied(0, 0, 0, 10)
                        };
                        painter.rect_filled(rect, CornerRadius::ZERO, fill);
                    }
                    if playing == Some(index) {
                        painter.rect_filled(
                            egui::Rect::from_min_size(rect.left_top(), vec2(3.0, rect.height())),
                            CornerRadius::ZERO,
                            p.accent,
                        );
                    }

                    let title_color = if playing == Some(index) { p.accent } else { p.text };
                    let painter = painter.with_clip_rect(rect);
                    let mut x = rect.left() + 8.0;
                    painter.text(
                        egui::pos2(x, rect.center().y),
                        Align2::LEFT_CENTER,
                        format!("{:02}", index + 1),
                        FontId::monospace(11.5),
                        p.dim,
                    );
                    x += widths[0];
                    painter.text(
                        egui::pos2(x, rect.center().y),
                        Align2::LEFT_CENTER,
                        theme::fit_text(&entry.display_title(), widths[1] - 8.0, 13.0),
                        FontId::proportional(13.0),
                        title_color,
                    );
                    x += widths[1];
                    painter.text(
                        egui::pos2(x, rect.center().y),
                        Align2::LEFT_CENTER,
                        theme::fit_text(&entry.display_artist(), widths[2] - 8.0, 12.5),
                        FontId::proportional(12.5),
                        p.dim,
                    );
                    x += widths[2];
                    painter.text(
                        egui::pos2(x, rect.center().y),
                        Align2::LEFT_CENTER,
                        theme::fit_text(&entry.display_album(), widths[3] - 8.0, 12.5),
                        FontId::proportional(12.5),
                        p.dim,
                    );
                    x += widths[3];
                    painter.text(
                        egui::pos2(x, rect.center().y),
                        Align2::LEFT_CENTER,
                        entry.length_text(),
                        FontId::monospace(12.0),
                        p.dim,
                    );
                    x += widths[4];
                    painter.text(
                        egui::pos2(x, rect.center().y),
                        Align2::LEFT_CENTER,
                        theme::fit_text(&entry.format, widths[5], 12.0),
                        FontId::proportional(12.0),
                        p.dim,
                    );

                    if resp.clicked() {
                        let mods = ui.input(|i| i.modifiers);
                        if mods.shift || mods.ctrl || mods.command {
                            extend_select = Some((index, mods.shift, mods.ctrl || mods.command));
                        } else {
                            selection = Some(index);
                        }
                    }
                    if resp.double_clicked() {
                        play_request = Some(index);
                    }
                    resp.context_menu(|ui| {
                        if ui.button(self.t("播放这一首", "Play")).clicked() {
                            menu_action = Some((index, MenuAction::Play));
                            ui.close_menu();
                        }
                        if ui.button(self.t("从列表删除", "Remove")).clicked() {
                            menu_action = Some((index, MenuAction::Remove));
                            ui.close_menu();
                        }
                        if ui.button(self.t("加入喜欢", "Like")).clicked() {
                            menu_action = Some((index, MenuAction::Like));
                            ui.close_menu();
                        }
                        if ui.button(self.t("上移", "Move up")).clicked() {
                            menu_action = Some((index, MenuAction::MoveUp));
                            ui.close_menu();
                        }
                        if ui.button(self.t("下移", "Move down")).clicked() {
                            menu_action = Some((index, MenuAction::MoveDown));
                            ui.close_menu();
                        }
                    });
                }
            });

        if let Some((index, range, toggle)) = extend_select {
            if range {
                let from = anchor.unwrap_or(index);
                let (lo, hi) = if from <= index { (from, index) } else { (index, from) };
                for i in lo..=hi {
                    self.song_select.insert(i);
                }
                self.song_anchor = Some(index);
            } else if toggle {
                if !self.song_select.remove(&index) {
                    self.song_select.insert(index);
                }
                self.song_anchor = Some(index);
            }
        }
        if let Some(i) = selection {
            self.song_select.clear();
            self.song_anchor = Some(i);
            if let Ok(mut pl) = self.shared.playlist.lock() {
                pl.selected = Some(i);
            }
        }
        if let Some(i) = play_request {
            self.shared.send(Cmd::PlayIndex(i));
        }
        if let Some((index, action)) = menu_action {
            match action {
                MenuAction::Play => self.shared.send(Cmd::PlayIndex(index)),
                MenuAction::Remove => {
                    if let Ok(mut pl) = self.shared.playlist.lock() {
                        pl.remove(index);
                    }
                    self.shared.send(Cmd::Refresh);
                }
                MenuAction::MoveUp => {
                    if index > 0 {
                        if let Ok(mut pl) = self.shared.playlist.lock() {
                            pl.move_entry(index, index - 1);
                        }
                    }
                }
                MenuAction::Like => {
                    if let Some(entry) = self.playlist.entries.get(index).cloned() {
                        let item = LikedItem {
                            title: entry.display_title(),
                            artist: entry.display_artist(),
                            album: entry.display_album(),
                            duration: entry.length.unwrap_or(0.0),
                            path: Some(entry.path.clone()),
                            plugin: None,
                            item: None,
                            added_at: now_secs(),
                        };
                        if self.liked.add(item) {
                            self.toast("已加入喜欢");
                        } else {
                            self.toast("这首歌已经在喜欢里了");
                        }
                    }
                }
                MenuAction::MoveDown => {
                    let len = self.playlist.len();
                    if index + 1 < len {
                        if let Ok(mut pl) = self.shared.playlist.lock() {
                            pl.move_entry(index, index + 1);
                        }
                    }
                }
            }
        }
    }

    /// 底部播放条：细一条，封面 + 曲目信息 + 播放控制 + 音量，下方通栏进度。
    fn nowband_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        egui::TopBottomPanel::bottom("nowband")
            .exact_height(100.0)
            .frame(egui::Frame::NONE.fill(if p.dark {
                Color32::from_rgb(17, 18, 21)
            } else {
                p.panel_alt
            }))
            .show(ctx, |ui| {
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                    if self.state.title.is_empty() {
                        // 没在播放时用应用图标当占位封面
                        if self.logo_texture.is_none() {
                            self.logo_texture = Some(load_logo_texture(ui.ctx()));
                        }
                        if let Some(tex) = &self.logo_texture {
                            ui.painter().image(
                                tex.id(),
                                rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::WHITE,
                            );
                        }
                        let painter = ui.painter();
                        painter.rect_stroke(
                            rect,
                            CornerRadius::same(6),
                            Stroke::new(1.0_f32, p.accent),
                            StrokeKind::Inside,
                        );
                    }
                    let painter = ui.painter();
                    painter.rect_filled(rect, CornerRadius::same(6), p.accent_soft);
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(6),
                        Stroke::new(1.0_f32, p.accent),
                        StrokeKind::Inside,
                    );
                    match self.current_cover(ui.ctx()) {
                        Some(tex) if !self.state.title.is_empty() => {
                            ui.painter().image(
                                tex.id(),
                                rect.shrink(1.0),
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::WHITE,
                            );
                        }
                        _ => {
                            if let Some(ch) = self.state.title.chars().next() {
                                painter.text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    ch.to_string(),
                                    FontId::proportional(18.0),
                                    p.accent,
                                );
                            }
                        }
                    }

                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        let title = if self.state.title.is_empty() {
                            self.t("未在播放", "Nothing playing").to_string()
                        } else {
                            self.state.title.clone()
                        };
                        ui.label(
                            RichText::new(elide(&title, 30))
                                .size(14.0)
                                .strong()
                                .color(p.text),
                        );
                        let sub = if self.state.title.is_empty() {
                            self.t("选一首开始", "Pick a track").to_string()
                        } else if self.state.artist.is_empty() {
                            self.state.album.clone()
                        } else {
                            format!("{} · {}", self.state.artist, self.state.album)
                        };
                        ui.label(RichText::new(elide(&sub, 34)).size(11.0).color(p.dim));
                    });

                    ui.add_space(14.0);
                    // 按迷你卡片那套手绘图标重画的播放条按钮
                    let liked = self.current_liked();
                    let (heart_rect, heart) =
                        ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
                    if liked {
                        tabler(
                            ui.painter(),
                            heart_rect,
                            "filled-heart",
                            Color32::from_rgb(240, 96, 108),
                        );
                    } else {
                        tabler(
                            ui.painter(),
                            heart_rect,
                            "heart",
                            if heart.hovered() { p.text } else { p.dim },
                        );
                    }
                    if heart
                        .on_hover_text(self.t("喜欢", "Like"))
                        .clicked()
                    {
                        self.toggle_current_liked();
                    }
                    ui.add_space(2.0);

                    let (prev_rect, prev) =
                        ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
                    mini_prev_icon(ui.painter(), prev_rect, mini_icon_color(&prev));
                    if prev
                        .on_hover_text(self.t("上一首", "Prev"))
                        .clicked()
                    {
                        self.shared.send(Cmd::Prev);
                    }
                    ui.add_space(4.0);

                    let (play_rect, play) =
                        ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
                    ui.painter().circle_filled(play_rect.center(), 20.0, p.accent);
                    if self.state.status == Status::Playing {
                        mini_pause_icon(ui.painter(), play_rect);
                    } else {
                        mini_play_icon(ui.painter(), play_rect, Color32::WHITE);
                    }
                    if play
                        .on_hover_text(if self.state.status == Status::Playing {
                            self.t("暂停", "Pause")
                        } else {
                            self.t("播放", "Play")
                        })
                        .clicked()
                    {
                        self.shared.send(Cmd::Toggle);
                    }
                    ui.add_space(4.0);

                    let (next_rect, next) =
                        ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
                    mini_next_icon(ui.painter(), next_rect, mini_icon_color(&next));
                    if next
                        .on_hover_text(self.t("下一首", "Next"))
                        .clicked()
                    {
                        self.shared.send(Cmd::Next);
                    }
                    ui.add_space(2.0);

                    let (stop_rect, stop) =
                        ui.allocate_exact_size(vec2(26.0, 30.0), Sense::click());
                    let stop_color = if stop.hovered() { p.text } else { p.dim };
                    ui.painter().rect_filled(
                        egui::Rect::from_center_size(stop_rect.center(), vec2(11.0, 11.0)),
                        CornerRadius::same(2),
                        stop_color,
                    );
                    if stop
                        .on_hover_text(self.t("停止", "Stop"))
                        .clicked()
                    {
                        self.shared.send(Cmd::Stop);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(12.0);
                        let (list_rect, list_resp) =
                            ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
                        mini_list_icon(ui.painter(), list_rect, mini_icon_color(&list_resp));
                        if list_resp
                            .on_hover_text(self.t("待播清单", "Playlists"))
                            .clicked()
                        {
                            self.tab = MainTab::Playlists;
                        }
                        if ui
                            .add(
                                egui::Button::new(RichText::new("词").size(14.0).color(
                                    if self.tab == MainTab::Lyrics { p.accent } else { p.dim },
                                ))
                                .min_size(vec2(30.0, 28.0)),
                            )
                            .on_hover_text(self.t("歌词", "Lyrics"))
                            .clicked()
                        {
                            self.tab = MainTab::Lyrics;
                        }
                        ui.add_space(4.0);
                        let mut vol = self.config.volume;
                        if ui
                            .add_sized(
                                vec2(110.0, 20.0),
                                egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false),
                            )
                            .changed()
                        {
                            self.config.volume = vol;
                            self.shared.send(Cmd::SetVolume(vol));
                        }
                        ui.label(RichText::new("音量").small().color(p.dim));
                        ui.add_space(14.0);
                        let (spec_rect, _) =
                            ui.allocate_exact_size(vec2(96.0, 26.0), Sense::hover());
                        paint_spectrum(ui.painter(), spec_rect, &self.spectrum, p);
                    });
                });

                // 通栏进度条（自绘，保证细条也看得见）
                let len = self.state.length.max(1.0);
                let pos = self.state.position.clamp(0.0, len);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(theme::time_text(self.state.position))
                            .monospace()
                            .size(10.5)
                            .color(p.dim),
                    );
                    ui.add_space(6.0);
                    let width = (ui.available_width() - 90.0).max(120.0);
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(width, 10.0), Sense::click_and_drag());
                    let painter = ui.painter();
                    painter.rect_filled(rect, CornerRadius::same(5), p.panel);
                    let frac = (pos / len) as f32;
                    let fill = egui::Rect::from_min_size(
                        rect.min,
                        vec2(rect.width() * frac, rect.height()),
                    );
                    painter.rect_filled(fill, CornerRadius::same(5), p.accent);
                    if resp.hovered() {
                        painter.circle_filled(
                            egui::pos2(rect.left() + rect.width() * frac, rect.center().y),
                            5.0,
                            p.accent,
                        );
                    }
                    if resp.clicked() || resp.dragged() {
                        if let Some(p) = resp.interact_pointer_pos() {
                            let frac = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                            self.shared.send(Cmd::Seek(frac as f64 * len));
                        }
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(theme::time_text(self.state.length))
                            .monospace()
                            .size(10.5)
                            .color(p.dim),
                    );
                });
            });
    }

    /// 迷你播放器：深色圆角卡片，圆形封面 + 喜欢 / 上一首 / 播放 / 下一首 / 待播 / 关闭。
    fn mini_ui(&mut self, ctx: &egui::Context, _p: &Palette) {
        let card_bg = Color32::from_rgb(24, 26, 30);
        let card_edge = Color32::from_rgb(46, 50, 56);
        let accent = self.config_accent();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let full = ui.available_rect_before_wrap();
                // 左边留出一条，让圆形封面能压出卡片边缘
                let card = egui::Rect::from_min_max(
                    egui::pos2(full.left() + 64.0, full.top() + 16.0),
                    egui::pos2(full.right() - 10.0, full.bottom() - 16.0),
                );
                let painter = ui.painter();
                painter.rect(
                    card,
                    CornerRadius::same(18),
                    card_bg,
                    Stroke::new(1.0_f32, card_edge),
                    egui::StrokeKind::Inside,
                );

                // 圆形封面：左边压在卡片边缘上
                // 封面比卡片高一点，上下都压出去，跟设计稿一样
                let cover_size = card.height() + 10.0;
                let cover = egui::Rect::from_center_size(
                    egui::pos2(card.left() + 2.0, card.center().y),
                    vec2(cover_size, cover_size),
                );
                let cover_tex = self
                    .current_round_cover(ui.ctx())
                    .or_else(|| self.current_cover(ui.ctx()));
                match cover_tex {
                    Some(tex) => {
                        ui.painter().image(
                            tex.id(),
                            cover,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        ui.painter()
                            .circle_filled(cover.center(), cover_size * 0.5, Color32::from_rgb(40, 43, 48));
                        ui.painter().text(
                            cover.center(),
                            Align2::CENTER_CENTER,
                            self.state.title.chars().next().unwrap_or('♪').to_string(),
                            FontId::proportional(cover_size * 0.42),
                            accent,
                        );
                    }
                }
                // 封面外圈细边
                ui.painter().circle_stroke(
                    cover.center(),
                    cover_size * 0.5,
                    Stroke::new(1.0_f32, Color32::from_rgb(60, 64, 70)),
                );

                // 控制按钮：从封面右侧开始，整组居中
                let bsize = 26.0;
                let gap = 22.0;
                let group_w = bsize * 4.0 + gap * 3.0 + 46.0 + gap; // 心/上一/播放/下一/清单
                let start_x = cover.right() + 12.0;
                let right_edge = card.right() - 44.0;
                let x0 = ((start_x + right_edge) * 0.5 - group_w * 0.5)
                    .max(start_x)
                    .min(right_edge - group_w);
                let cy = card.center().y;
                let liked = self.current_liked();
                let mut rect_at = |i: usize, size: f32| {
                    egui::Rect::from_center_size(
                        egui::pos2(x0 + size * 0.5 + i as f32 * (bsize + gap) * 0.92, cy),
                        vec2(size, size),
                    )
                };

                // 喜欢
                let heart_rect = rect_at(0, 24.0);
                let heart = ui.interact(heart_rect, ui.id().with("mini-heart"), Sense::click());
                mini_heart_icon(
                    ui.painter(),
                    heart_rect,
                    if liked {
                        Color32::from_rgb(206, 62, 78)
                    } else if heart.hovered() {
                        Color32::from_rgb(176, 182, 190)
                    } else {
                        Color32::from_rgb(132, 138, 146)
                    },
                );
                if heart.clicked() {
                    self.toggle_current_liked();
                }

                // 上一首
                let prev_rect = rect_at(1, bsize);
                let prev = ui.interact(prev_rect, ui.id().with("mini-prev"), Sense::click());
                mini_prev_icon(ui.painter(), prev_rect, mini_icon_color(&prev));
                if prev.clicked() {
                    self.shared.send(Cmd::Prev);
                }

                // 播放 / 暂停（实心强调色圆）
                let play_size = 46.0;
                let play_rect = rect_at(2, play_size);
                let play = ui.interact(play_rect, ui.id().with("mini-play"), Sense::click());
                ui.painter().circle_filled(play_rect.center(), play_size * 0.5, accent);
                if self.state.status == Status::Playing {
                    mini_pause_icon(ui.painter(), play_rect);
                } else {
                    mini_play_icon(ui.painter(), play_rect, Color32::WHITE);
                }
                if play.clicked() {
                    self.shared.send(Cmd::Toggle);
                }

                // 下一首
                let next_rect = rect_at(3, bsize);
                let next = ui.interact(next_rect, ui.id().with("mini-next"), Sense::click());
                mini_next_icon(ui.painter(), next_rect, mini_icon_color(&next));
                if next.clicked() {
                    self.shared.send(Cmd::Next);
                }

                // 待播清单
                let list_rect = rect_at(4, bsize);
                let list = ui.interact(list_rect, ui.id().with("mini-list"), Sense::click());
                mini_list_icon(ui.painter(), list_rect, mini_icon_color(&list));
                if list.clicked() {
                    self.tab = MainTab::Playlists;
                    self.config.mini = false;
                    let _ = self.config.save();
                }

                // 关闭：回主界面
                let close_rect = egui::Rect::from_center_size(
                    egui::pos2(card.right() - 20.0, card.top() + 20.0),
                    vec2(28.0, 28.0),
                );
                let close = ui.interact(close_rect, ui.id().with("mini-close"), Sense::click());
                mini_close_icon(
                    ui.painter(),
                    close_rect,
                    if close.hovered() {
                        Color32::from_rgb(200, 205, 212)
                    } else {
                        Color32::from_rgb(140, 145, 152)
                    },
                );
                if close.clicked() {
                    self.config.mini = false;
                    let _ = self.config.save();
                }

                // 底边一条细进度
                let len = self.state.length.max(1.0);
                let ratio = (self.state.position / len).clamp(0.0, 1.0) as f32;
                let bar = egui::Rect::from_min_max(
                    egui::pos2(card.left() + 10.0, card.bottom() - 4.0),
                    egui::pos2(card.right() - 10.0, card.bottom() - 2.0),
                );
                ui.painter().rect_filled(
                    bar,
                    CornerRadius::same(2),
                    Color32::from_rgb(52, 56, 62),
                );
                let mut done = bar;
                done.set_width(bar.width() * ratio);
                ui.painter().rect_filled(done, CornerRadius::same(2), accent);
            });
    }

    /// 贴着主题按钮弹出的小菜单，三选一，点完立即换肤并存盘。
    fn theme_menu_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.theme_menu_open {
            return;
        }
        let Some(anchor) = self.theme_button_rect else {
            self.theme_menu_open = false;
            return;
        };

        // 跟随封面时，小方块里的强调色点也用封面色
        let live_accent = if self.config.cover_accent {
            self.cover_accent_rgb.unwrap_or(self.config.accent)
        } else {
            self.config.accent
        };
        // 每个主题的底色小方块，先算好避免在闭包里反复借用 self
        let items: Vec<(audcore::Theme, &'static str, egui::Color32, bool)> =
            audcore::Theme::all()
                .into_iter()
                .map(|t| {
                    let bg = theme::palette_for(t, live_accent).bg;
                    let label = if self.has_cjk { t.label() } else { t.label_en() };
                    (t, label, bg, self.config.theme == t)
                })
                .collect();

        let pos = egui::pos2(anchor.right() - 160.0, anchor.bottom() + 6.0);
        let mut chosen = None;
        let area = egui::Area::new(egui::Id::new("theme_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_width(152.0);
                        for (t, label, bg, selected) in &items {
                            ui.horizontal(|ui| {
                                let (rect, _) = ui
                                    .allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                                ui.painter().rect_filled(
                                    rect,
                                    egui::CornerRadius::same(3),
                                    *bg,
                                );
                                // 底色块里再点一颗强调色，深色主题的小方块才不至于糊成一片
                                let dot = egui::Rect::from_center_size(
                                    rect.center(),
                                    vec2(6.0, 6.0),
                                );
                                ui.painter().rect_filled(
                                    dot,
                                    egui::CornerRadius::same(3),
                                    theme::palette_for(*t, live_accent).accent,
                                );
                                ui.painter().rect_stroke(
                                    rect,
                                    egui::CornerRadius::same(3),
                                    Stroke::new(1.0_f32, if *selected { p.accent } else { p.dim }),
                                    egui::StrokeKind::Inside,
                                );
                                let text_color = if *selected { p.accent } else { p.text };
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(*label).color(text_color),
                                        )
                                        .fill(egui::Color32::TRANSPARENT)
                                        .stroke(Stroke::NONE),
                                    )
                                    .clicked()
                                {
                                    chosen = Some(*t);
                                }
                            });
                        }
                        ui.separator();
                        let mut follow = self.config.cover_accent;
                        let label = self.t("跟随封面取色", "Accent from cover");
                        if ui.checkbox(&mut follow, label).changed() {
                            self.config.cover_accent = follow;
                            self.album_accent.clear();
                            self.cover_accent_rgb = None;
                            let _ = self.config.save();
                            self.theme_menu_open = false;
                        }
                    });
            });

        if let Some(t) = chosen {
            self.theme_menu_open = false;
            self.set_theme(t);
        }

        // 点菜单外面或按 Esc 收起
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        let pressed = ctx.input(|i| i.pointer.any_pressed());
        let escaped = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if escaped
            || (pressed
                && pointer
                    .map(|pos| !area.response.rect.contains(pos) && !anchor.contains(pos))
                    .unwrap_or(true))
        {
            self.theme_menu_open = false;
        }
    }

    fn set_theme(&mut self, t: audcore::Theme) {
        if self.config.theme == t {
            return;
        }
        self.config.theme = t;
        // 让下一帧的 style_key 检查发现变化并重刷 style
        let _ = self.config.save();
    }

    fn settings_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        let mut open = self.show_settings;
        let mut changed = false;
        egui::Window::new(self.t("设置", "Settings"))
            .open(&mut open)
            .default_width(380.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(self.t("播放", "Playback")).strong().color(p.accent));
                ui.horizontal(|ui| {
                    ui.label(self.t("音量", "Volume"));
                    if ui
                        .add(egui::Slider::new(&mut self.config.volume, 0.0..=1.0))
                        .changed()
                    {
                        self.shared.send(Cmd::SetVolume(self.config.volume));
                        changed = true;
                    }
                });
                let mut shuffle = self.config.shuffle;
                if ui
                    .checkbox(&mut shuffle, self.t("随机播放", "Shuffle"))
                    .changed()
                {
                    self.config.shuffle = shuffle;
                    if let Ok(mut pl) = self.shared.playlist.lock() {
                        pl.shuffle = shuffle;
                    }
                    changed = true;
                }
                ui.horizontal(|ui| {
                    ui.label(self.t("循环", "Repeat"));
                    for (mode, zh, en) in [
                        (RepeatMode::Off, "关", "Off"),
                        (RepeatMode::All, "列表", "All"),
                        (RepeatMode::Track, "单曲", "Track"),
                    ] {
                        let selected = self.config.repeat == mode;
                        if ui
                            .selectable_label(selected, self.t(zh, en))
                            .clicked()
                        {
                            self.config.repeat = mode;
                            if let Ok(mut pl) = self.shared.playlist.lock() {
                                pl.repeat = mode;
                            }
                            changed = true;
                        }
                    }
                });
                let mut eq = self.config.eq_enabled;
                if ui
                    .checkbox(&mut eq, self.t("启用均衡器", "Equalizer"))
                    .changed()
                {
                    self.config.eq_enabled = eq;
                    self.shared.eq.set_enabled(eq);
                    changed = true;
                }

                ui.separator();
                ui.label(RichText::new(self.t("外观", "Appearance")).strong().color(p.accent));
                ui.horizontal_wrapped(|ui| {
                    for t in audcore::Theme::all() {
                        let label = if self.has_cjk { t.label() } else { t.label_en() };
                        if ui
                            .selectable_label(self.config.theme == t, label)
                            .clicked()
                            && self.config.theme != t
                        {
                            self.config.theme = t;
                            changed = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(self.t("强调色", "Accent"));
                    if ui
                        .color_edit_button_srgb(&mut self.config.accent)
                        .changed()
                    {
                        changed = true;
                    }
                    let mut follow = self.config.cover_accent;
                    if ui
                        .checkbox(&mut follow, self.t("跟随封面", "From cover"))
                        .changed()
                    {
                        self.config.cover_accent = follow;
                        self.album_accent.clear();
                        self.cover_accent_rgb = None;
                        changed = true;
                    }
                });

                ui.separator();
                ui.label(RichText::new(self.t("迷你模式", "Mini mode")).strong().color(p.accent));
                let mut mini = self.config.mini;
                if ui.checkbox(&mut mini, self.t("只显示信息带", "Now playing only")).changed() {
                    self.config.mini = mini;
                    changed = true;
                }

                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        self.t("配置文件", "Config file"),
                        Config::path().display()
                    ))
                    .small()
                    .color(p.dim),
                );
                if ui.button(self.t("保存配置", "Save config")).clicked() {
                    changed = true;
                }
            });
        self.show_settings = open;
        if changed {
            self.config.clamp();
            self.shared.eq.set_gains(&self.config.eq_gains);
            let _ = self.config.save();
        }
    }

    fn eq_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        let mut open = self.show_eq;
        let mut changed = false;
        egui::Window::new(self.t("十段均衡器", "Equalizer"))
            .open(&mut open)
            .default_width(560.0)
            .resizable(false)
            .show(ctx, |ui| {
                let mut enabled = self.config.eq_enabled;
                if ui
                    .checkbox(&mut enabled, self.t("启用", "Enable"))
                    .changed()
                {
                    self.config.eq_enabled = enabled;
                    self.shared.eq.set_enabled(enabled);
                    changed = true;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    for band in 0..audcore::EQ_BANDS.len() {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(format!("{}", audcore::EQ_BANDS[band]))
                                    .small()
                                    .color(p.dim),
                            );
                            let mut gain = self.config.eq_gains[band];
                            ui.spacing_mut().slider_width = 110.0;
                            if ui
                                .add(
                                    egui::Slider::new(&mut gain, -24.0..=24.0)
                                        .vertical()
                                        .show_value(false),
                                )
                                .changed()
                            {
                                self.config.eq_gains[band] = gain;
                                changed = true;
                            }
                            ui.label(
                                RichText::new(format!("{:.0}", self.config.eq_gains[band]))
                                    .small()
                                    .monospace()
                                    .color(p.text),
                            );
                        });
                    }
                });
                ui.separator();
                if ui.button(self.t("全部归零", "Reset flat")).clicked() {
                    self.config.eq_gains = vec![0.0; audcore::EQ_BANDS.len()];
                    changed = true;
                }
            });
        self.show_eq = open;
        if changed {
            self.shared.eq.set_gains(&self.config.eq_gains);
            let _ = self.config.save();
        }
    }

    // ---------------------------------------------------------- 在线插件

    /// 重新扫描插件目录：放到后台线程，避免插件卡死时界面「无响应」。
    fn reload_media_plugins(&mut self) {
        let dir = audplugins::plugins_dir();
        let _ = std::fs::create_dir_all(&dir);
        if self.scan_rx.is_some() {
            return;
        }
        self.scan_rx = Some(audplugins::scan_async(dir));
    }

    fn poll_media_channels(&mut self) {
        if let Some(rx) = &self.scan_rx {
            match rx.try_recv() {
                Ok(plugins) => {
                    let count = plugins.len();
                    self.media_plugins = plugins;
                    if self.search_plugin >= self.media_plugins.len() {
                        self.search_plugin = 0;
                    }
                    self.scan_rx = None;
                    self.toast(format!("已加载 {count} 个在线插件"));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.scan_rx = None;
                    self.toast("插件扫描线程异常结束");
                }
            }
        }

        if let Some(rx) = &self.search_rx {
            match rx.try_recv() {
                Ok(Ok(items)) => {
                    self.search_results = items;
                    self.search_status = SearchStatus::Idle;
                    self.search_rx = None;
                }
                Ok(Err(err)) => {
                    self.search_results.clear();
                    self.search_status = SearchStatus::Error(err);
                    self.search_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.search_status = SearchStatus::Error("搜索线程异常结束".into());
                    self.search_rx = None;
                }
            }
        }

        if let Some(rx) = &self.lyric_rx {
            match rx.try_recv() {
                Ok(Ok(raw)) => {
                    self.lyric_rx = None;
                    self.lyric_state = if raw.trim().is_empty() {
                        LyricState::Error("插件没有返回歌词".into())
                    } else {
                        let lines = parse_lrc(&raw);
                        if lines.is_empty() {
                            LyricState::Plain(raw)
                        } else {
                            LyricState::Lines(lines)
                        }
                    };
                }
                Ok(Err(err)) => {
                    self.lyric_rx = None;
                    self.lyric_state = LyricState::Error(err.to_string());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.lyric_rx = None;
                    self.lyric_state = LyricState::Error("歌词线程异常结束".into());
                }
            }
        }

        if let Some(rx) = &self.resolve_rx {
            match rx.try_recv() {
                Ok(Ok((item, source, play_now, plugin_index))) => {
                    self.resolve_rx = None;
                    audplugins::remember_headers(&source.url, &source.headers);
                    let entry = remote_entry(&item, &source.url);
                    let title = entry.display_title();
                    let index = {
                        let Ok(mut pl) = self.shared.playlist.lock() else {
                            return;
                        };
                        pl.add(entry)
                    };
                    self.shared.send(Cmd::Refresh);
                    if play_now {
                        self.current_online = Some((plugin_index, item.clone()));
                        self.lyric_key.clear();
                        self.lyric_state = LyricState::Idle;
                        self.lyric_rx = None;
                        self.shared.send(Cmd::PlayIndex(index));
                        self.toast(format!("正在播放 {title}"));
                    } else {
                        self.toast(format!("已加入列表：{title}"));
                    }
                }
                Ok(Err(err)) => {
                    self.resolve_rx = None;
                    self.toast(format!("解析失败：{err}"));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.resolve_rx = None;
                    self.toast("解析线程异常结束");
                }
            }
        }
    }

    fn start_search(&mut self) {
        let Some(plugin) = self.media_plugins.get(self.search_plugin).cloned() else {
            self.search_status = SearchStatus::Error("没有可用的在线插件".into());
            return;
        };
        if !plugin.supports("search") {
            self.search_status = SearchStatus::Error("该插件不支持搜索".into());
            return;
        }
        if self.search_rx.is_some() {
            return;
        }

        let query = self.search_query.trim().to_string();
        if query.is_empty() {
            self.search_status = SearchStatus::Error("请输入搜索关键词".into());
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.search_rx = Some(rx);
        self.search_status = SearchStatus::Busy;
        std::thread::spawn(move || {
            let result = plugin
                .search_tracks(&query, 1)
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
    }

    /// 解析播放地址：`play_now = true` 直接播放，否则只加入列表。
    fn resolve_media(&mut self, item: &serde_json::Value, play_now: bool) {
        log::info!("resolve_media play_now={play_now} idx={}", self.search_plugin);
        let Some(plugin) = self.media_plugins.get(self.search_plugin).cloned() else {
            log::warn!("没有插件可解析");
            return;
        };
        if !plugin.supports("getMediaSource") {
            self.toast("该插件不支持取播放地址");
            return;
        }
        if self.resolve_rx.is_some() {
            return;
        }

        let item = item.clone();
        let index = self.search_plugin;
        let (tx, rx) = std::sync::mpsc::channel();
        self.resolve_rx = Some(rx);
        std::thread::spawn(move || {
            log::info!("resolving media for {:?}", item["title"]);
            let result = plugin
                .media(&item, "standard")
                .map_err(|e| e.to_string());
            log::info!("resolve result: {:?}", result);
            let _ = tx.send(result.map(|url| (item, url, play_now, index)));
        });
    }

    fn search_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        let mut open = self.show_search;
        egui::Window::new(self.t("在线搜索", "Online Search"))
            .open(&mut open)
            .default_width(700.0)
            .max_width(700.0)
            .default_height(420.0)
            .show(ctx, |ui| {
                if self.media_plugins.is_empty() {
                    ui.label(
                        RichText::new(self.t(
                            "还没有在线插件。把 MusicFree 插件 .js 放进插件目录，然后点“重新扫描”。",
                            "No online plugins yet. Drop MusicFree .js plugins into the plugin folder and rescan.",
                        ))
                        .color(p.dim),
                    );
                    ui.label(
                        RichText::new(audplugins::plugins_dir().display().to_string())
                            .monospace()
                            .small()
                            .color(p.dim),
                    );
                    if ui.button(self.t("重新扫描", "Rescan")).clicked() {
                        self.reload_media_plugins();
                    }
                    return;
                }

                ui.horizontal(|ui| {
                    ui.label(self.t("插件", "Plugin"));
                    let current = self
                        .media_plugins
                        .get(self.search_plugin)
                        .map(|pl| pl.name())
                        .unwrap_or_default();
                    egui::ComboBox::from_id_salt("search-plugin")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            let names: Vec<String> = self
                                .media_plugins
                                .iter()
                                .map(|pl| pl.name())
                                .collect();
                            for (i, name) in names.into_iter().enumerate() {
                                ui.selectable_value(&mut self.search_plugin, i, name);
                            }
                        });
                });

                let hint = self.t("歌名 / 歌手", "Song / artist");
                ui.horizontal(|ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .desired_width(320.0)
                            .hint_text(hint),
                    );
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let busy = matches!(self.search_status, SearchStatus::Busy);
                    if ui
                        .add_enabled(!busy, egui::Button::new(self.t("搜索", "Search")))
                        .clicked()
                        || enter
                    {
                        self.start_search();
                    }
                    if busy {
                        ui.spinner();
                    }
                });

                if let SearchStatus::Error(err) = &self.search_status {
                    ui.label(RichText::new(err).color(egui::Color32::from_rgb(200, 80, 80)));
                }

                ui.separator();
                if self.search_results.is_empty() {
                    ui.label(
                        RichText::new(self.t("输入关键词后回车搜索", "Type a keyword and press Enter"))
                            .color(p.dim),
                    );
                    return;
                }

                let items = self.search_results.clone();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(300.0)
                    .show(ui, |ui| {
                        egui::Grid::new("search-results")
                            .num_columns(5)
                            .striped(true)
                            .min_col_width(40.0)
                            .show(ui, |ui| {
                                ui.label(RichText::new(self.t("标题", "Title")).strong().color(p.accent));
                                ui.label(RichText::new(self.t("艺人", "Artist")).strong().color(p.accent));
                                ui.label(RichText::new(self.t("专辑", "Album")).strong().color(p.accent));
                                ui.label(RichText::new(self.t("时长", "Length")).strong().color(p.accent));
                                ui.label("");
                                ui.end_row();

                                for (i, item) in items.iter().enumerate() {
                                    let title = item["title"].as_str().unwrap_or("未命名").to_string();
                                    let artist = item["artist"].as_str().unwrap_or("—").to_string();
                                    let album = item["album"].as_str().unwrap_or("—").to_string();
                                    let length = item["duration"]
                                        .as_f64()
                                        .map(audcore::config::format_time)
                                        .unwrap_or_else(|| "--:--".into());
                                    ui.label(format!("{}. {}", i + 1, elide(&title, 26)));
                                    ui.label(elide(&artist, 18));
                                    ui.label(elide(&album, 18));
                                    ui.label(length);
                                    ui.horizontal(|ui| {
                                        if ui.small_button("▶").on_hover_text(self.t("播放", "Play")).clicked() {
                                            self.resolve_media(item, true);
                                        }
                                        if ui.small_button("＋").on_hover_text(self.t("加入列表", "Add")).clicked() {
                                            self.resolve_media(item, false);
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                    });
            });
        self.show_search = open;
    }

    fn plugins_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        let mut open = self.show_plugins;
        egui::Window::new(self.t("插件", "Plugins"))
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                // 后台下载安装的结果
                let installed = self.install_rx.as_ref().and_then(|rx| rx.try_recv().ok());
                if let Some(result) = installed {
                    self.install_rx = None;
                    match result {
                        Ok(plugin) => {
                            self.plugin_install_msg = Some((
                                true,
                                format!(
                                    "已安装《{}》，插件目录 {}",
                                    plugin.name,
                                    plugin.dir.display()
                                ),
                            ));
                            self.reload_media_plugins();
                        }
                        Err(err) => {
                            self.plugin_install_msg = Some((false, format!("安装失败：{err}")));
                        }
                    }
                }

                let title = self.t("安装插件", "Install plugin");
                let desc = self.t(
                    "只支持 MyGo Music 规范：一个插件目录，里面有 plugin.json 和 index.js（ESM 导出 search / stream / lyrics，可用全局 host 和 fetch）。「选 zip」直接挑打包好的插件压缩包，「选文件夹」直接挑插件目录；也可以在下面填 .zip 的下载地址，或者干脆把 .zip / 插件文件夹直接拖进窗口，拖进来就装好了。",
                    "MyGo Music layout only: one folder containing plugin.json and index.js (ESM exporting search / stream / lyrics, with global host and fetch). You can choose a zip, choose a folder, paste a download URL, or simply drag a zip / plugin folder into the window.",
                );
                let install_label = self.t("安装", "Install");
                let zip_label = self.t("选 zip", "Choose zip");
                let folder_label = self.t("选文件夹", "Choose folder");

                let mut url_buf = self.plugin_url_input.clone();
                let mut do_install = false;
                let mut open_zip = false;
                let mut open_folder = false;
                let mut open_dir = false;
                let mut rescan = false;
                let message = self.plugin_install_msg.clone();

                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(title.clone()).strong().color(p.accent));
                    ui.label(RichText::new(desc.clone()).small().color(p.dim));
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let button_width = 210.0;
                        let width = (ui.available_width() - button_width).max(140.0);
                        ui.add_sized(
                            vec2(width, 26.0),
                            egui::TextEdit::singleline(&mut url_buf)
                                .hint_text("https://example.com/plugin.zip"),
                        );
                        if ui
                            .add(egui::Button::new(RichText::new(install_label.clone()).color(
                                egui::Color32::WHITE,
                            ))
                            .fill(p.accent))
                            .clicked()
                        {
                            do_install = true;
                        }
                        if ui.button(zip_label.clone()).clicked() {
                            open_zip = true;
                        }
                        if ui.button(folder_label.clone()).clicked() {
                            open_folder = true;
                        }
                    });
                    if let Some((ok, text)) = &message {
                        ui.label(
                            RichText::new(text)
                                .small()
                                .color(if *ok {
                                    p.accent
                                } else {
                                    egui::Color32::from_rgb(200, 80, 80)
                                }),
                        );
                    }
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        if ui
                            .small_button(self.t("打开插件目录", "Open plugins folder"))
                            .clicked()
                        {
                            open_dir = true;
                        }
                        if ui.small_button(self.t("重新扫描", "Rescan")).clicked() {
                            rescan = true;
                        }
                        if self.install_rx.is_some() {
                            ui.spinner();
                            ui.label(
                                RichText::new(self.t("正在下载安装…", "Downloading…"))
                                    .small()
                                    .color(p.dim),
                            );
                        }
                    });
                });
                self.plugin_url_input = url_buf;
                ui.add_space(6.0);

                if open_dir {
                    open_in_file_manager(&audplugins::plugins_dir());
                }
                if rescan {
                    self.reload_media_plugins();
                }
                if open_zip || open_folder {
                    let base = self
                        .config
                        .last_dir
                        .clone()
                        .or_else(dirs::home_dir)
                        .unwrap_or_else(|| PathBuf::from("."));
                    self.plugin_pick = Some(PluginPick::new(base, open_zip));
                }
                if do_install {
                    let url = self.plugin_url_input.trim().to_string();
                    if url.is_empty() {
                        self.plugin_install_msg = Some((
                            false,
                            self.t("先填一个插件下载地址", "Enter a download URL first").to_string(),
                        ));
                    } else {
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let result = audplugins::install::install_from_url(&url)
                                .map_err(|e| e.to_string());
                            let _ = tx.send(result);
                        });
                        self.install_rx = Some(rx);
                    }
                }

                ui.separator();
                let mut last_kind: Option<audcore::plugin::PluginKind> = None;
                for info in &self.plugins {
                    if last_kind != Some(info.kind) {
                        last_kind = Some(info.kind);
                        ui.add_space(6.0);
                        ui.label(RichText::new(info.kind.label()).strong().color(p.accent));
                    }
                    ui.horizontal(|ui| {
                        ui.label("●");
                        ui.label(RichText::new(&info.name).color(p.text));
                    });
                    ui.label(RichText::new(&info.description).small().color(p.dim));
                }
                ui.separator();
                ui.label(
                    RichText::new(self.t(
                        "解码与输出插件在编译期注册，运行期常开。",
                        "Plugins are registered at compile time and always on.",
                    ))
                    .small()
                    .color(p.dim),
                );

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.t("在线插件（MusicFree / MyGo）", "Online plugins (MusicFree / MyGo)"))
                            .strong()
                            .color(p.accent),
                    );
                    let scanning = self.scan_rx.is_some();
                    if ui
                        .add_enabled(
                            !scanning,
                            egui::Button::new(self.t("重新扫描", "Rescan")).small(),
                        )
                        .clicked()
                    {
                        self.reload_media_plugins();
                    }
                    if scanning {
                        ui.spinner();
                        ui.label(RichText::new(self.t("正在扫描…", "Scanning…")).small().color(p.dim));
                    }
                    if ui.small_button(self.t("打开目录", "Open folder")).clicked() {
                        open_in_file_manager(&audplugins::plugins_dir());
                    }
                    if ui
                        .add_enabled(!self.media_plugins.is_empty(), egui::Button::new(self.t("在线搜索…", "Search…")))
                        .clicked()
                    {
                        self.show_search = true;
                    }
                });

                if self.media_plugins.is_empty() {
                    ui.label(
                        RichText::new(self.t(
                            "把 MusicFree 的 .js 或 MyGo 插件目录（含 plugin.json）放进下面目录，点“重新扫描”即可加载：",
                            "Put MusicFree .js files or MyGo plugin folders (with plugin.json) here, then rescan:",
                        ))
                        .small()
                        .color(p.dim),
                    );
                    ui.label(
                        RichText::new(audplugins::plugins_dir().display().to_string())
                            .monospace()
                            .small()
                            .color(p.dim),
                    );
                } else {
                    let plugins = self.media_plugins.clone();
                    for pl in &plugins {
                        ui.horizontal(|ui| {
                            ui.label("●");
                            ui.label(
                                RichText::new(format!(
                                    "{}  v{}",
                                    pl.name(),
                                    if pl.version().is_empty() { "?" } else { pl.version() }
                                ))
                                .color(p.text),
                            );
                            if pl.supports("search") {
                                ui.label(RichText::new("搜索").small().color(p.dim));
                            }
                            if pl.supports("getMediaSource") {
                                ui.label(RichText::new("取流").small().color(p.dim));
                            }
                            if pl.supports("getLyric") || pl.supports("lyrics") {
                                ui.label(RichText::new("歌词").small().color(p.dim));
                            }
                        });
                        if let Some(err) = pl.error() {
                            ui.label(
                                RichText::new(format!("  加载失败：{err}"))
                                    .small()
                                    .color(egui::Color32::from_rgb(200, 80, 80)),
                            );
                        }

                        // MyGo 插件声明的设置项，直接在这里填
                        if let Some(mygo) = pl.as_mygo() {
                            if mygo.supports_login() || mygo.supports_sign_in() {
                                let id = mygo.manifest.id.clone();
                                if ui
                                    .button(self.t("扫码登录", "Scan to sign in"))
                                    .on_hover_text(self.t(
                                        "插件支持扫码登录（导出了 loginQr / loginPoll 或 signIn）",
                                        "This plugin supports QR sign-in (loginQr / loginPoll or signIn)",
                                    ))
                                    .clicked()
                                {
                                    self.open_login(&id);
                                }
                            }
                            if !mygo.settings().is_empty() {
                                for def in mygo.settings() {
                                    if def.key.is_empty() {
                                        continue;
                                    }
                                    let buf_key = format!("{}/{}", mygo.manifest.id, def.key);
                                    let entry = self
                                        .mygo_setting_inputs
                                        .entry(buf_key)
                                        .or_insert_with(|| {
                                            let raw = mygo.setting(&def.key);
                                            if raw.trim() == "null" || raw.trim() == "undefined" {
                                                String::new()
                                            } else {
                                                raw
                                            }
                                        });
                                    let mut save = false;
                                    ui.horizontal(|ui| {
                                        let label = if def.label.is_empty() {
                                            def.key.clone()
                                        } else {
                                            zh_plugin_text(&def.label)
                                        };
                                        ui.label(RichText::new(label).small().color(p.dim));
                                        let width = (ui.available_width() - 60.0).max(120.0);
                                        let resp = ui.add_sized(
                                            [width, 20.0],
                                            egui::TextEdit::singleline(entry)
                                                .hint_text("未设置")
                                                .password(def.kind.as_deref() == Some("password")),
                                        );
                                        save = resp.lost_focus() && !resp.changed();
                                    });
                                    if !def.help.is_empty() {
                                        ui.label(
                                            RichText::new(zh_plugin_text(&def.help))
                                                .small()
                                                .color(p.dim),
                                        );
                                    }
                                    if save {
                                        mygo.set_setting(&def.key, entry.trim());
                                        self.toast(format!("已保存 {}", zh_plugin_text(&def.label)));
                                    }
                                }
                            }
                        }
                    }
                }
            });
        self.show_plugins = open;
    }

    fn about_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        let mut open = self.show_about;
        egui::Window::new(self.t("关于", "About"))
            .open(&mut open)
            .default_width(400.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("AUDACIOUS · RUST")
                        .size(20.0)
                        .strong()
                        .color(p.accent),
                );
                ui.label(
                    RichText::new(format!("{} {}", self.t("版本", "Version"), audcore::VERSION))
                        .monospace(),
                );
                ui.add_space(6.0);
                ui.label(self.t(
                    "Audacious 内核的 Rust 重写：播放列表、插件体系、均衡器、频谱与 audtool 控制协议。",
                    "A Rust rewrite of the Audacious core: playlist, plugin traits, EQ, spectrum and audtool control.",
                ));
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "{} egui {} · rodio/symphonia · lofty",
                        self.t("界面", "UI"),
                        "0.31"
                    ))
                    .small()
                    .color(p.dim),
                );
                ui.hyperlink_to(
                    "github.com/hhym/audacious",
                    "https://github.com/hhym/audacious",
                );
                ui.label(
                    RichText::new(self.t(
                        "原项目以 BSD-2-Clause 发布，本项目同样以 BSD-2-Clause 发布。",
                        "Original project is BSD-2-Clause; this rewrite is too.",
                    ))
                    .small()
                    .color(p.dim),
                );
            });
        self.show_about = open;
    }

    /// 「选 zip / 选文件夹」的小浏览窗口：选中后直接安装。
    fn plugin_pick_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(want_zip) = self.plugin_pick.as_ref().map(|p| p.want_zip) else {
            return;
        };
        let title = if want_zip {
            self.t("选择插件压缩包", "Choose plugin zip").to_string()
        } else {
            self.t("选择插件目录", "Choose plugin folder").to_string()
        };
        let up_label = self.t("上级", "Up").to_string();
        let install_label = self.t("导入这个", "Install this").to_string();
        let close_label = self.t("关闭", "Close").to_string();
        let hint = if want_zip {
            self.t(
                "选一个 .zip 压缩包（里面要有 plugin.json 或 index.js）",
                "Pick a .zip containing plugin.json or index.js",
            )
            .to_string()
        } else {
            self.t(
                "选一个里面有 plugin.json 的插件目录",
                "Pick a folder containing plugin.json",
            )
            .to_string()
        };
        let Some(pick) = self.plugin_pick.as_mut() else {
            return;
        };

        let mut open = true;
        let mut enter: Option<usize> = None;
        let mut chosen: Option<PathBuf> = None;
        let mut close = false;
        let dir_text = pick.dir.to_string_lossy().to_string();
        let entries = pick.entries.clone();
        let selected = pick.selected;

        egui::Window::new(title)
            .open(&mut open)
            .default_width(560.0)
            .default_height(430.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button(up_label.clone()).clicked() {
                        if let Some(parent) = pick.dir.parent() {
                            pick.dir = parent.to_path_buf();
                            pick.refresh();
                        }
                    }
                    ui.label(RichText::new(dir_text.clone()).monospace().small().color(p.dim));
                });
                ui.label(RichText::new(hint.clone()).small().color(p.dim));
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for (i, (path, is_dir)) in entries.iter().enumerate() {
                            let name = path
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            let label = if *is_dir {
                                format!("/ {name}")
                            } else {
                                format!("  {name}")
                            };
                            let is_selected = selected == Some(i);
                            let resp = ui.selectable_label(is_selected, label);
                            if resp.clicked() {
                                pick.selected = Some(i);
                                if *is_dir {
                                    enter = Some(i);
                                }
                            }
                            if resp.double_clicked() && !*is_dir {
                                chosen = Some(path.clone());
                            }
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    let can_install = pick
                        .selected
                        .and_then(|i| pick.entries.get(i))
                        .map(|(path, is_dir)| {
                            if *is_dir {
                                path.join("plugin.json").is_file()
                            } else {
                                true
                            }
                        })
                        .unwrap_or(false);
                    if ui
                        .add_enabled(can_install, egui::Button::new(install_label.clone()).fill(p.accent))
                        .clicked()
                    {
                        if let Some((path, _)) = pick.selected.and_then(|i| pick.entries.get(i)) {
                            chosen = Some(path.clone());
                        }
                    }
                    if ui.button(close_label.clone()).clicked() {
                        close = true;
                    }
                });
            });

        if let Some(index) = enter {
            if let Some((path, _)) = pick.entries.get(index).cloned() {
                pick.dir = path;
                pick.refresh();
            }
        }

        if let Some(path) = chosen {
            match audplugins::install::install_from_path(&path) {
                Ok(plugin) => {
                    self.plugin_install_msg =
                        Some((true, format!("已安装《{}》到 {}", plugin.name, plugin.dir.display())));
                    self.reload_media_plugins();
                }
                Err(err) => {
                    self.plugin_install_msg = Some((false, format!("安装失败：{err}")));
                }
            }
            close = true;
        }

        if close || !open {
            self.plugin_pick = None;
        }
    }

    fn browser_ui(&mut self, ctx: &egui::Context, p: &Palette) -> Vec<PathBuf> {
        let title = self.t("添加文件", "Add files");
        let label_up = self.t("上级", "Up");
        let label_add_selected = self.t("添加选中", "Add selected");
        let label_add_folder = self.t("添加整个目录", "Add whole folder");

        let Some(browser) = self.browser.as_mut() else {
            return Vec::new();
        };
        let mut open = true;
        let mut pending: Vec<PathBuf> = Vec::new();
        let mut enter_index: Option<usize> = None;
        let mut add_selected: Option<PathBuf> = None;

        egui::Window::new(title)
            .open(&mut open)
            .default_width(560.0)
            .default_height(420.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button(label_up).clicked() {
                        browser.go_up();
                    }
                    ui.label(
                        RichText::new(browser.dir.to_string_lossy().to_string())
                            .monospace()
                            .small()
                            .color(p.dim),
                    );
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        let entries = browser.entries.clone();
                        for (i, (path, is_dir)) in entries.iter().enumerate() {
                            let name = path
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            let label = if *is_dir {
                                format!("{name}/")
                            } else {
                                format!("    {name}")
                            };
                            let selected = browser.selected == Some(i);
                            let resp = ui.selectable_label(selected, label);
                            if resp.clicked() {
                                browser.selected = Some(i);
                                if *is_dir {
                                    enter_index = Some(i);
                                }
                            }
                            if resp.double_clicked() && !*is_dir {
                                add_selected = Some(path.clone());
                            }
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(label_add_selected).clicked() {
                        if let Some(i) = browser.selected {
                            if let Some((path, is_dir)) = browser.entries.get(i) {
                                if !*is_dir {
                                    add_selected = Some(path.clone());
                                }
                            }
                        }
                    }
                    if ui.button(label_add_folder).clicked() {
                        pending.push(browser.dir.clone());
                    }
                });
            });

        if let Some(i) = enter_index {
            browser.enter(i);
        }
        if let Some(path) = add_selected {
            if let Some(dir) = path.parent() {
                self.config.last_dir = Some(dir.to_path_buf());
                let _ = self.config.save();
            }
            pending.push(path);
        }
        if !open {
            self.browser = None;
        }
        pending
    }
}

enum MenuAction {
    Play,
    Remove,
    MoveUp,
    MoveDown,
    Like,
}

/// 列宽：标题占一半弹性宽度，艺人/专辑各占四分之一，其余固定。
fn column_widths(total: f32) -> [f32; 6] {
    let fixed = 48.0 + 66.0 + 84.0 + 16.0;
    let flexible = (total - fixed).max(300.0);
    [
        48.0,
        (flexible * 0.50).max(160.0),
        (flexible * 0.25).max(90.0),
        (flexible * 0.25).max(90.0),
        66.0,
        84.0,
    ]
}

fn paint_spectrum(painter: &egui::Painter, rect: egui::Rect, spectrum: &[f32], p: &Palette) {
    let n = spectrum.len().max(1);
    let gap = 3.0;
    let bar_w = ((rect.width() - gap * (n as f32 - 1.0)) / n as f32).max(2.0);
    let baseline = rect.bottom();
    for (i, value) in spectrum.iter().enumerate() {
        let h = (value * rect.height()).max(1.5);
        let x = rect.left() + i as f32 * (bar_w + gap);
        let bar = egui::Rect::from_min_max(
            egui::pos2(x, baseline - h),
            egui::pos2(x + bar_w, baseline),
        );
        let color = if p.accent.r() as u32 + p.accent.g() as u32 + p.accent.b() as u32 > 400 {
            p.accent
        } else {
            p.accent
        };
        painter.rect_filled(bar, CornerRadius::same(1), color);
    }
}

/// 供 main 复用：把命令行参数转成路径列表。
pub fn paths_from_args(args: &[String]) -> Vec<PathBuf> {
    args.iter()
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .collect()
}

/// 在给定矩形里画一件 T 恤，用作「换肤」按钮的图标。
fn paint_shirt(
    painter: &egui::Painter,
    rect: egui::Rect,
    color: egui::Color32,
    collar: egui::Color32,
) {
    tabler(painter, rect, "palette", color);
}

/// 由搜索到的歌曲 JSON 和播放地址构造播放列表条目。
fn remote_entry(item: &serde_json::Value, url: &str) -> audcore::Entry {
    audcore::Entry {
        path: PathBuf::from(url),
        title: item["title"].as_str().unwrap_or("在线歌曲").to_string(),
        artist: item["artist"].as_str().unwrap_or_default().to_string(),
        album: item["album"].as_str().unwrap_or("在线".into()).to_string(),
        length: item["duration"].as_f64(),
        format: "在线".into(),
    }
}

impl App {
    /// 主面板：左侧导航栏 + 顶部搜索条 + 内容区。
    fn main_panel(&mut self, ctx: &egui::Context, p: &Palette) {
        self.sidebar_ui(ctx, p);
        self.top_search_bar(ctx, p);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.bg))
            .show(ctx, |ui| {
                self.page_header(ui, p);
                match self.tab {
                    MainTab::Home => self.home_tab(ui, p),
                    MainTab::Search => self.search_tab(ui, p),
                    MainTab::Songs => {
                        self.songs_toolbar(ui, p);
                        self.playlist_table(ui, p);
                    }
                    MainTab::Liked => self.liked_tab(ui, p),
                    MainTab::Playlists => self.playlists_tab(ui, p),
                    MainTab::Lyrics => self.lyrics_tab(ui, p),
                    MainTab::Album => self.album_tab(ui, p),
                }
            });
    }

    /// 左侧栏：应用名、在线音源、导航、我的歌单、按专辑。
    fn sidebar_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        // 弹层要画在侧栏面板外面，否则会被 236px 宽的侧栏裁掉
        let mut picker_pos: Option<egui::Pos2> = None;
        // 刚点开的那一帧别走「点外面收起」，否则同一次点击立刻关掉
        let mut sidebar_just_opened = false;
        egui::SidePanel::left("sidebar")
            .exact_width(236.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(p.bg)
                    .stroke(egui::Stroke::new(1.0, p.panel))
                    .inner_margin(egui::Margin::symmetric(10, 10)),
            )
            .show(ctx, |ui| {
                // 音源标识：圆形 logo + 当前音源名 + 「共 N 个音源 ⌄」
                let source_name = self
                    .media_plugins
                    .get(self.search_plugin)
                    .map(|pl| pl.name())
                    .unwrap_or_else(|| self.t("本地音频", "Local audio").to_string());
                let source_count = self.media_plugins.len();
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), 15.0, p.accent_soft);
                    if self.logo_texture.is_none() {
                        self.logo_texture = Some(load_logo_texture(ui.ctx()));
                    }
                    if let Some(tex) = &self.logo_texture {
                        let inner = rect.shrink(4.0);
                        ui.painter().image(
                            tex.id(),
                            inner,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(elide(&source_name, 16))
                                .size(14.0)
                                .strong()
                                .color(p.text),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} {} {}",
                                self.t("共", "of"),
                                source_count,
                                self.t("个音源", "sources")
                            ))
                            .small()
                            .color(p.dim),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // 点这个箭头（或下面的大封面）都能换音源
                        let arrow = ui
                            .add(egui::Button::new(RichText::new(self.t("切换", "Switch")).size(11.0).color(p.dim)).frame(false))
                            .on_hover_text(self.t("切换音源", "Switch source"));
                        picker_pos = Some(arrow.rect.left_bottom() + vec2(0.0, 4.0));
                        if arrow.clicked() {
                            self.plugin_picker_open = !self.plugin_picker_open;
                            self.plugin_picker_source = "sidebar";
                            if self.plugin_picker_open {
                                self.plugin_page = self.search_plugin / Self::PLUGIN_PAGE_SIZE;
                            }
                            sidebar_just_opened = self.plugin_picker_open;
                        }
                    });
                });
                // 播放封面 + 「＋」添加文件
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(8), p.accent_soft);
                    let painter = ui.painter();
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(8),
                        Stroke::new(1.0_f32, p.panel),
                        StrokeKind::Inside,
                    );
                    if !self.state.title.is_empty() {
                        let cover_tex = self.current_cover(ui.ctx());
                        match cover_tex {
                            Some(tex) => {
                                ui.painter().image(
                                    tex.id(),
                                    rect.shrink(1.0),
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    Color32::WHITE,
                                );
                            }
                            None => {
                                painter.text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    self.state
                                        .title
                                        .chars()
                                        .next()
                                        .unwrap_or('♪')
                                        .to_string(),
                                    FontId::proportional(22.0),
                                    p.accent,
                                );
                            }
                        }
                    } else {
                        painter.text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            "♪",
                            FontId::proportional(22.0),
                            p.accent,
                        );
                    }
                    ui.add_space(4.0);
                    let (add_rect, resp) =
                        ui.allocate_exact_size(vec2(52.0, 52.0), Sense::click());
                    let painter = ui.painter();
                    painter.rect_filled(add_rect, CornerRadius::same(8), p.bg);
                    painter.rect_stroke(
                        add_rect,
                        CornerRadius::same(8),
                        Stroke::new(1.0_f32, if resp.hovered() { p.accent } else { p.panel }),
                        StrokeKind::Inside,
                    );
                    painter.text(
                        add_rect.center(),
                        Align2::CENTER_CENTER,
                        "＋",
                        FontId::proportional(22.0),
                        if resp.hovered() { p.accent } else { p.dim },
                    );
                    if resp.clicked() {
                        self.browser = Some(Browser::new(
                            self.config
                                .last_dir
                                .clone()
                                .unwrap_or_else(|| PathBuf::from(".")),
                        ));
                    }
                });

                ui.add_space(12.0);
                self.sidebar_tab(ui, p, MainTab::Home, self.t("首页", "Home"), None);
                self.sidebar_tab(ui, p, MainTab::Search, self.t("搜索", "Search"), None);
                self.sidebar_tab(
                    ui,
                    p,
                    MainTab::Liked,
                    self.t("喜欢", "Liked"),
                    Some(self.liked.len()),
                );
                self.sidebar_tab(
                    ui,
                    p,
                    MainTab::Playlists,
                    self.t("待播清单", "Playlists"),
                    Some(Self::playlist_files().len()),
                );
                self.sidebar_tab(
                    ui,
                    p,
                    MainTab::Songs,
                    self.t("歌曲", "Songs"),
                    Some(self.playlist.len()),
                );
                self.sidebar_tab(ui, p, MainTab::Lyrics, self.t("歌词", "Lyrics"), None);

                ui.add_space(10.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.t("我的歌单", "My playlists"))
                            .small()
                            .color(p.dim),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("＋").on_hover_text(self.t("保存当前列表", "Save list")).clicked()
                        {
                            self.save_playlist();
                        }
                    });
                });
                let files = Self::playlist_files();
                if files.is_empty() {
                    ui.label(
                        RichText::new(self.t("还没有保存的歌单", "No saved playlists"))
                            .small()
                            .color(p.dim),
                    );
                }
                for file in files.iter().take(6) {
                    let name = file
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("歌单")
                        .to_string();
                    if ui
                        .add_sized(
                            vec2(ui.available_width(), 22.0),
                            egui::Button::new(RichText::new(elide(&name, 18)).small().color(p.text))
                                .frame(false),
                        )
                        .clicked()
                    {
                        self.load_playlist_file(file, true);
                    }
                }

                ui.add_space(8.0);
                ui.label(
                    RichText::new(self.t("按专辑", "By album"))
                        .small()
                        .color(p.dim),
                );
                let albums = self.album_counts();
                if albums.is_empty() {
                    ui.label(
                        RichText::new(self.t("列表里有歌后这里会分组", "Albums appear once you add songs"))
                            .small()
                            .color(p.dim),
                    );
                }
                for (album, count) in albums.iter().take(8) {
                    // 第一次用到这张专辑时去读文件里的内嵌封面，之后走缓存
                    if !self.album_art.contains_key(album) {
                        let bytes = self
                            .playlist
                            .entries
                            .iter()
                            .find(|e| &e.display_album() == album)
                            .and_then(|e| audcore::tags::read_art(&e.path));
                        let tex = bytes.and_then(|b| cover_texture(ui.ctx(), &b, album));
                        self.album_art.insert(album.clone(), tex);
                    }
                    let text = format!("{}  ·  {}", elide(album, 14), count);
                    let width = ui.available_width();
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
                    if self.tab == MainTab::Album && self.album_filter == *album {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(5), p.accent_soft);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(5), p.panel);
                    }
                    let cover = egui::Rect::from_center_size(
                        rect.left_center() + vec2(18.0, 0.0),
                        vec2(22.0, 22.0),
                    );
                    match self.album_art.get(album) {
                        Some(Some(tex)) => {
                            ui.painter().image(
                                tex.id(),
                                cover,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::WHITE,
                            );
                        }
                        _ => {
                            ui.painter()
                                .rect_filled(cover, CornerRadius::same(4), p.accent_soft);
                            ui.painter().text(
                                cover.center(),
                                Align2::CENTER_CENTER,
                                album.chars().next().unwrap_or('♪').to_string(),
                                FontId::proportional(10.5),
                                p.accent,
                            );
                        }
                    }
                    ui.painter().text(
                        rect.left_center() + vec2(36.0, 0.0),
                        Align2::LEFT_CENTER,
                        text,
                        FontId::proportional(12.0),
                        p.dim,
                    );
                    if resp.clicked() {
                        self.album_filter = album.clone();
                        self.tab = MainTab::Album;
                    }
                }
            });
        if self.plugin_picker_open && self.plugin_picker_source == "sidebar" {
            let names: Vec<String> = self.media_plugins.iter().map(|pl| pl.name()).collect();
            let pos = picker_pos.unwrap_or_else(|| egui::pos2(16.0, 120.0));
            let area_id = egui::Id::new(("main-plugin-picker", "sidebar"));
            self.plugin_picker_ui(ctx, pos, &names, p, !sidebar_just_opened, area_id);
        }
    }

    /// 侧栏里的一行导航（左对齐，选中时铺底色）。
    fn sidebar_tab(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        tab: MainTab,
        label: &str,
        count: Option<usize>,
    ) {
        let selected = self.tab == tab;
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
        if selected {
            ui.painter().rect_filled(rect, CornerRadius::same(6), p.accent_soft);
        } else if resp.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), p.panel);
        }
        let color = if selected { p.accent } else { p.text };
        ui.painter().text(
            rect.left_center() + vec2(12.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(13.5),
            color,
        );
        if let Some(count) = count {
            ui.painter().text(
                rect.right_center() - vec2(12.0, 0.0),
                Align2::RIGHT_CENTER,
                count.to_string(),
                FontId::proportional(11.0),
                p.dim,
            );
        }
        if resp.clicked() {
            self.tab = tab;
        }
    }

    /// 当前主题的强调色（迷你播放器先把 palette 借不走，这里单独取一份）。
    fn config_accent(&self) -> Color32 {
        self.pal().accent
    }

    /// 当前这套配色：跟随封面的强调色会盖掉配置里的 `accent`。
    fn pal(&self) -> theme::Palette {
        match self.cover_accent_rgb {
            Some(a) if self.config.cover_accent => {
                theme::palette_for(self.config.theme, a)
            }
            _ => theme::palette(&self.config),
        }
    }

    /// 换曲时重新从封面挑主色，同一张专辑走缓存。
    fn refresh_cover_accent(&mut self) {
        if !self.config.cover_accent {
            self.cover_accent_rgb = None;
            return;
        }
        let album = self.state.album.clone();
        if album.is_empty() {
            self.cover_accent_rgb = None;
            return;
        }
        if let Some(cached) = self.album_accent.get(&album) {
            self.cover_accent_rgb = *cached;
            return;
        }
        let title = self.state.title.clone();
        let dark = self.config.theme.is_dark();
        let bytes = self
            .playlist
            .entries
            .iter()
            .find(|e| e.album == album && (title.is_empty() || e.title == title))
            .and_then(|e| audcore::tags::read_art(&e.path));
        let accent = bytes
            .as_deref()
            .and_then(|b| theme::accent_from_image(b, dark));
        self.album_accent.insert(album, accent);
        self.cover_accent_rgb = accent;
    }

    /// 当前曲目在「喜欢」里的样子（本地文件按路径，在线按插件 + id）。
    fn current_liked_entry(&self) -> Option<LikedItem> {
        if let Some((idx, item)) = &self.current_online {
            return Some(LikedItem {
                title: item["title"].as_str().unwrap_or("未命名").to_string(),
                artist: item["artist"].as_str().unwrap_or_default().to_string(),
                album: item["album"].as_str().unwrap_or_default().to_string(),
                duration: item["duration"].as_f64().unwrap_or(0.0),
                path: None,
                plugin: Some(
                    self.media_plugins
                        .get(*idx)
                        .map(|p| p.name())
                        .unwrap_or_default(),
                ),
                item: Some(item.clone()),
                added_at: now_secs(),
            });
        }
        let i = self.state.current?;
        let entry = self.playlist.entries.get(i)?;
        Some(LikedItem {
            title: entry.display_title(),
            artist: entry.display_artist(),
            album: entry.display_album(),
            duration: entry.length.unwrap_or(0.0),
            path: Some(entry.path.clone()),
            plugin: None,
            item: None,
            added_at: now_secs(),
        })
    }

    fn current_liked(&self) -> bool {
        self.current_liked_entry()
            .map(|it| self.liked.contains(&it))
            .unwrap_or(false)
    }

    /// 迷你播放器那颗心：已收藏就从「喜欢」里移除，没收藏就加进去。
    fn toggle_current_liked(&mut self) {
        let Some(item) = self.current_liked_entry() else {
            return;
        };
        let key = item.key();
        if let Some(pos) = self.liked.items().iter().position(|i| i.key() == key) {
            self.liked.remove(pos);
            self.toast("已取消喜欢");
        } else {
            let title = item.title.clone();
            if self.liked.add(item) {
                self.toast(&format!("已加入喜欢：{title}"));
            }
        }
    }

    /// 圆形封面（迷你播放器用，四角抠成透明）。
    fn current_round_cover(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let album = self.state.album.clone();
        if album.is_empty() {
            return None;
        }
        if !self.album_art_round.contains_key(&album) {
            let title = self.state.title.clone();
            let bytes = self
                .playlist
                .entries
                .iter()
                .find(|e| e.album == album && (title.is_empty() || e.title == title))
                .and_then(|e| audcore::tags::read_art(&e.path));
            let tex = bytes.and_then(|b| round_texture(ctx, &b, &album, 88));
            self.album_art_round.insert(album.clone(), tex);
        }
        self.album_art_round.get(&album).cloned().flatten()
    }

    /// 当前播放曲目的内嵌封面（在线曲目没有文件，返回 None）。
    fn current_cover(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let album = self.state.album.clone();
        if album.is_empty() {
            return None;
        }
        if !self.album_art.contains_key(&album) {
            let title = self.state.title.clone();
            let bytes = self
                .playlist
                .entries
                .iter()
                .find(|e| e.album == album && (title.is_empty() || e.title == title))
                .and_then(|e| audcore::tags::read_art(&e.path));
            let tex = bytes.and_then(|b| cover_texture(ctx, &b, &album));
            self.album_art.insert(album.clone(), tex);
        }
        self.album_art.get(&album).cloned().flatten()
    }

    fn album_counts_btn_count(&self) -> usize {
        self.album_counts()
            .iter()
            .find(|(name, _)| name == &self.album_filter)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }

    fn album_counts(&self) -> Vec<(String, usize)> {
        let mut albums: Vec<(String, usize)> = Vec::new();
        for entry in &self.playlist.entries {
            let name = entry.display_album();
            if name.is_empty() || name == "—" {
                continue;
            }
            match albums.iter_mut().find(|(album, _)| *album == name) {
                Some((_, count)) => *count += 1,
                None => albums.push((name, 1)),
            }
        }
        albums.sort_by(|a, b| b.1.cmp(&a.1));
        albums
    }

    /// 顶部搜索条：常驻，回车或点「搜索」跳到搜索页。
    fn top_search_bar(&mut self, ctx: &egui::Context, p: &Palette) {
        let mut top_picker_pos: Option<egui::Pos2> = None;
        // 本帧刚点开时不要走「点外面就收起」的判断，否则同一次点击会立刻关掉
        let mut top_just_opened = false;
        egui::TopBottomPanel::top("topsearch")
            .exact_height(50.0)
            .frame(egui::Frame::NONE.fill(p.bg))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(12.0);
                    let hint = self.t("歌名 / 歌手", "Song / artist");
                    let edit = ui.add_sized(
                        vec2((ui.available_width() - 220.0).max(180.0), 30.0),
                        egui::TextEdit::singleline(&mut self.search_query).hint_text(hint),
                    );
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let busy = matches!(self.search_status, SearchStatus::Busy);
                    if ui
                        .add_enabled(!busy, egui::Button::new(self.t("搜索", "Search")))
                        .clicked()
                        || enter
                    {
                        self.tab = MainTab::Search;
                        self.start_search();
                    }
                    // 右侧三个小方按钮：音源 / 待播清单 / 随机
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        let mut shuffle = self.playlist.shuffle;
                        if top_icon_button(ui, p, "随", shuffle)
                            .on_hover_text(self.t("随机播放", "Shuffle"))
                            .clicked()
                        {
                            shuffle = !shuffle;
                            if let Ok(mut pl) = self.shared.playlist.lock() {
                                pl.shuffle = shuffle;
                            }
                            self.config.shuffle = shuffle;
                            let _ = self.config.save();
                        }
                        if top_icon_button(ui, p, "≡", self.tab == MainTab::Playlists)
                            .on_hover_text(self.t("待播清单", "Playlists"))
                            .clicked()
                        {
                            self.tab = MainTab::Playlists;
                        }
                        let src_btn = top_icon_button(
                            ui,
                            p,
                            "音",
                            self.plugin_picker_open && self.plugin_picker_source == "topsearch",
                        )
                        .on_hover_text(self.t("切换音源", "Switch source"));
                        top_picker_pos = Some(src_btn.rect.left_bottom() + vec2(0.0, 4.0));
                        if src_btn.clicked() {
                            self.plugin_picker_open = !self.plugin_picker_open;
                            self.plugin_picker_source = "topsearch";
                            top_just_opened = self.plugin_picker_open;
                            if self.plugin_picker_open {
                                self.plugin_page = self.search_plugin / Self::PLUGIN_PAGE_SIZE;
                            }
                        }
                    });
                });
            });
        // 弹层画在面板外面，否则会被 50px 高的面板裁掉
        if self.plugin_picker_open && self.plugin_picker_source == "topsearch" {
            let names: Vec<String> = self.media_plugins.iter().map(|pl| pl.name()).collect();
            let pos = top_picker_pos.unwrap_or_else(|| egui::pos2(880.0, 56.0));
            let area_id = egui::Id::new(("main-plugin-picker", "topsearch"));
            self.plugin_picker_ui(ctx, pos, &names, p, !top_just_opened, area_id);
        }
    }

    /// 内容区标题：大字标题 + 一行说明，右侧是音源选择。
    fn page_header(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let (title, subtitle) = match self.tab {
            MainTab::Home => (self.t("首页", "Home"), self.t("当前音源的主页", "Home of the current source")),
            MainTab::Search => (
                self.t("搜索结果", "Search results"),
                self.t("输入关键词后回车搜索", "Type a keyword and press Enter"),
            ),
            MainTab::Liked => (
                self.t("我喜欢", "Liked"),
                self.t("搜索结果里的 ☆、右键「加入喜欢」都会到这里", "Stars from search land here"),
            ),
            MainTab::Playlists => (
                self.t("待播清单", "Playlists"),
                self.t("本地保存的 m3u / m3u8", "Saved local playlists"),
            ),
            MainTab::Songs => (
                self.t("歌曲", "Songs"),
                self.t("当前播放队列", "Current queue"),
            ),
            MainTab::Lyrics => (
                self.t("歌词", "Lyrics"),
                self.t(
                    "跟着唱，可切卡拉OK / 整段滚动",
                    "Sing along — karaoke or whole-page scroll",
                ),
            ),
            MainTab::Album => (
                self.t("专辑", "Album"),
                self.t("列表里这张专辑的曲目", "Tracks of this album in the queue"),
            ),
        };
        let title = if self.tab == MainTab::Album && !self.album_filter.is_empty() {
            format!("{} · {}", self.t("专辑", "Album"), self.album_filter)
        } else {
            title.to_string()
        };
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(&title).size(20.0).strong().color(p.text));
                ui.label(RichText::new(subtitle).small().color(p.dim));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                let hint = match self.tab {
                    MainTab::Search if !self.search_results.is_empty() => {
                        format!("{} 条结果", self.search_results.len())
                    }
                    MainTab::Songs => format!("队列 {} 首", self.playlist.len()),
                    MainTab::Liked => format!("{} 首", self.liked.len()),
                    MainTab::Album => format!("{} 首", self.album_counts_btn_count()),
                    _ => String::new(),
                };
                ui.label(RichText::new(hint).small().color(p.dim));
            });
        });
        ui.add_space(8.0);
        ui.separator();
    }

    /// 歌词页：可切对齐方式（居左/居中/居右）与「滚动歌词 / 卡拉OK」两种模式。
    fn lyrics_tab(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.add_space(8.0);
        let title = self.state.title.clone();
        if title.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(
                    RichText::new(self.t("还没有正在播放的歌", "Nothing playing")).color(p.dim),
                );
            });
            return;
        }
        let sub = if self.state.artist.is_empty() {
            self.state.album.clone()
        } else {
            format!("{} · {}", self.state.artist, self.state.album)
        };
        let mut align = self.config.lyric_align;
        let mut karaoke = self.config.lyric_karaoke;
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(&title).size(19.0).strong().color(p.text));
                ui.label(RichText::new(sub).small().color(p.dim));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(18.0);
                // 右侧模式胶囊：卡拉OK 选中时实心强调，整段滚动是描边
                if lyric_pill(ui, p, self.t("整段滚动", "Scroll"), !karaoke).clicked() {
                    karaoke = false;
                }
                if lyric_pill(ui, p, self.t("卡拉OK", "Karaoke"), karaoke).clicked() {
                    karaoke = true;
                }
            });
        });

        // 对齐方式：小字一行，不抢视觉
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.label(RichText::new(self.t("对齐", "Align")).small().color(p.dim));
            for (value, label) in [
                (0u8, self.t("居左", "Left")),
                (1u8, self.t("居中", "Center")),
                (2u8, self.t("居右", "Right")),
            ] {
                let small = RichText::new(label).size(11.5);
                let color = if align == value { p.accent } else { p.dim };
                if ui.add(egui::Button::new(small.color(color)).frame(false)).clicked() {
                    align = value;
                }
            }
        });
        if align != self.config.lyric_align || karaoke != self.config.lyric_karaoke {
            self.config.lyric_align = align;
            self.config.lyric_karaoke = karaoke;
            let _ = self.config.save();
        }

        ui.add_space(6.0);

        if self.state.current.is_none() {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(
                    RichText::new(self.t(
                        "还没有在放的曲目，先点一首吧。",
                        "Nothing playing yet.",
                    ))
                    .color(p.dim),
                );
            });
            return;
        }

        let key = self.current_lyric_key();
        if self.lyric_key != key {
            self.lyric_key = key;
            self.start_lyric_load();
        }

        match self.lyric_state.clone() {
            LyricState::Loading => {
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.spinner();
                    ui.label(RichText::new(self.t("正在取歌词…", "Loading lyrics…")).color(p.dim));
                });
            }
            LyricState::Error(err) => {
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.label(
                        RichText::new(format!("歌词不可用：{err}"))
                            .small()
                            .color(egui::Color32::from_rgb(190, 90, 90)),
                    );
                });
            }
            LyricState::Plain(text) => {
                let align = self.config.lyric_align;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(10.0);
                        let rich = RichText::new(text.clone()).size(15.0).color(p.text);
                        match align {
                            0 => {
                                ui.horizontal_wrapped(|ui| {
                                    ui.add_space(20.0);
                                    ui.label(rich);
                                });
                            }
                            2 => {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Min),
                                    |ui| {
                                        ui.add_space(20.0);
                                        ui.label(rich);
                                    },
                                );
                            }
                            _ => {
                                ui.vertical_centered(|ui| {
                                    ui.label(rich);
                                });
                            }
                        }
                    });
            }
            LyricState::Lines(lines) => {
                let pos = self.state.position;
                let len = self.state.length.max(1.0);
                let mut active = 0usize;
                for (i, (t, _)) in lines.iter().enumerate() {
                    if *t <= pos + 0.05 {
                        active = i;
                    }
                }
                if self.config.lyric_karaoke {
                    self.karaoke_view(ui, p, &lines, active, pos, len);
                } else {
                    let changed = self.lyric_active != active;
                    self.lyric_active = active;
                    let align = self.config.lyric_align;
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(ui.available_height() * 0.25);
                            let width = ui.available_width();
                            for (i, (_, text)) in lines.iter().enumerate() {
                                let is_active = i == active;
                                let height = if is_active { 36.0 } else { 26.0 };
                                let (rect, _) = ui.allocate_exact_size(
                                    vec2(width, height),
                                    Sense::hover(),
                                );
                                let color = if is_active { p.accent } else { p.dim };
                                let font = egui::FontId::proportional(if is_active {
                                    18.0
                                } else {
                                    14.5
                                });
                                let x = match align {
                                    0 => rect.left() + 24.0,
                                    2 => rect.right() - 24.0,
                                    _ => rect.center().x,
                                };
                                let anchor = match align {
                                    0 => Align2::LEFT_CENTER,
                                    2 => Align2::RIGHT_CENTER,
                                    _ => Align2::CENTER_CENTER,
                                };
                                ui.painter().text(
                                    egui::pos2(x, rect.center().y),
                                    anchor,
                                    text,
                                    font,
                                    color,
                                );
                                if is_active && changed {
                                    ui.scroll_to_rect(rect, Some(egui::Align::Center));
                                }
                            }
                            ui.add_space(ui.available_height() * 0.5);
                        });
                }
            }
            LyricState::Idle => {}
        }
    }

    /// 卡拉OK 模式：上一行 / 当前行（按行时长做扫色）/ 下一行。
    fn karaoke_view(
        &self,
        ui: &mut egui::Ui,
        p: &Palette,
        lines: &[(f64, String)],
        active: usize,
        pos: f64,
        len: f64,
    ) {
        let align = self.config.lyric_align;
        let width = ui.available_width();
        let height = ui.available_height().max(120.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let center_y = rect.center().y;
        let x = match align {
            0 => rect.left() + 28.0,
            2 => rect.right() - 28.0,
            _ => rect.center().x,
        };
        let anchor = match align {
            0 => Align2::LEFT_CENTER,
            2 => Align2::RIGHT_CENTER,
            _ => Align2::CENTER_CENTER,
        };
        let painter = ui.painter();

        if active > 0 {
            if let Some((_, prev)) = lines.get(active - 1) {
                painter.text(
                    egui::pos2(x, center_y - 58.0),
                    anchor,
                    prev,
                    egui::FontId::proportional(14.0),
                    p.dim,
                );
            }
        }
        if let Some((_, next)) = lines.get(active + 1) {
            painter.text(
                egui::pos2(x, center_y + 58.0),
                anchor,
                next,
                egui::FontId::proportional(14.0),
                p.dim,
            );
        }

        let Some((start, cur)) = lines.get(active) else { return };
        let end = lines
            .get(active + 1)
            .map(|(t, _)| *t)
            .unwrap_or(len.max(*start + 3.0));
        let frac = ((pos - start) / (end - start).max(0.1)).clamp(0.0, 1.0) as f32;

        let font = egui::FontId::proportional(28.0);
        let text_rect =
            painter.text(egui::pos2(x, center_y), anchor, cur, font.clone(), p.dim);
        // 已唱过的部分用强调色扫上去
        let mut clip = text_rect;
        clip.set_width(text_rect.width() * frac);
        clip = clip.intersect(rect);
        let clipped = painter.with_clip_rect(clip);
        clipped.text(egui::pos2(x, center_y), anchor, cur, font, p.accent);
    }

    /// 当前在线曲目的歌词缓存键。
    fn current_lyric_key(&self) -> String {
        match &self.current_online {
            Some((idx, item)) => format!(
                "{idx}|{}|{}",
                item["title"].as_str().unwrap_or(""),
                item["artist"].as_str().unwrap_or("")
            ),
            None => match self.state.current {
                Some(i) if i < self.playlist.entries.len() => {
                    format!("local|{}", self.playlist.entries[i].path.display())
                }
                _ => String::new(),
            },
        }
    }

    fn start_lyric_load(&mut self) {
        let Some((index, item)) = self.current_online.clone() else {
            // 本地曲目：找同名 .lrc，其次内嵌歌词标签
            let Some(i) = self.state.current else { return };
            let Some(entry) = self.playlist.entries.get(i).cloned() else {
                return;
            };
            self.lyric_state = LyricState::Loading;
            let (tx, rx) = std::sync::mpsc::channel();
            self.lyric_rx = Some(rx);
            std::thread::spawn(move || {
                let text = audcore::tags::read_lyric(&entry.path);
                let _ = tx.send(text.ok_or_else(|| {
                    "本地文件没有歌词：放一个同名 .lrc，或给音频写入歌词标签".to_string()
                }));
            });
            return;
        };
        let Some(plugin) = self.media_plugins.get(index).cloned() else {
            self.lyric_state = LyricState::Error(self.t("曲目来自插件，但插件已被卸载", "Plugin missing").into());
            return;
        };
        if !plugin.supports("lyric") && !plugin.supports("lyrics") && !plugin.supports("getLyric") {
            self.lyric_state = LyricState::Error(self.t("该插件没有歌词接口", "No lyric API").into());
            return;
        }
        self.lyric_state = LyricState::Loading;
        let (tx, rx) = std::sync::mpsc::channel();
        self.lyric_rx = Some(rx);
        std::thread::spawn(move || {
            let out = plugin.lyric(&item).map_err(|e| e.to_string());
            let text = out.and_then(|v| extract_lyric_text(&v).ok_or_else(|| "插件没有返回歌词文本".to_string()));
            let _ = tx.send(text);
        });
    }

    /// 按专辑视图：列出这张专辑里的曲目，点一行直接播。
    fn album_tab(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.add_space(10.0);
        if self.album_filter.is_empty() {
            return;
        }
        let album = self.album_filter.clone();
        let rows: Vec<(usize, String, String, f64)> = self
            .shared
            .playlist
            .lock()
            .map(|pl| {
                pl.entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.display_album() == album)
                    .map(|(i, e)| {
                        (i, e.display_title(), e.display_artist(), e.length.unwrap_or(0.0))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if rows.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(RichText::new(self.t("这张专辑的歌已经不在列表里了", "No tracks in this album")).color(p.dim));
            });
            return;
        }
        let mut play: Option<usize> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("album-grid")
                    .num_columns(4)
                    .striped(true)
                    .min_col_width(60.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new(self.t("标题", "Title")).small().color(p.accent));
                        ui.label(RichText::new(self.t("艺人", "Artist")).small().color(p.accent));
                        ui.label(RichText::new(self.t("时长", "Length")).small().color(p.accent));
                        ui.label("");
                        ui.end_row();
                        for (index, title, artist, len) in &rows {
                            if ui
                                .add(egui::Button::new(RichText::new(title).color(p.text)).frame(false))
                                .clicked()
                            {
                                play = Some(*index);
                            }
                            ui.label(RichText::new(artist).small().color(p.dim));
                            ui.label(
                                RichText::new(theme::time_text(*len))
                                    .monospace()
                                    .small()
                                    .color(p.dim),
                            );
                            if ui.small_button("▶").clicked() {
                                play = Some(*index);
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some(index) = play {
            self.shared.send(Cmd::PlayIndex(index));
        }
    }

    /// 在线插件选择器：显示插件名，点开可以换插件。
    fn plugin_selector(&mut self, ui: &mut egui::Ui, p: &Palette, key: &'static str) {
        if self.media_plugins.is_empty() {
            ui.label(
                RichText::new(self.t("未安装在线插件", "No online plugin"))
                    .small()
                    .color(p.dim),
            );
            return;
        }
        let names: Vec<String> = self.media_plugins.iter().map(|pl| pl.name()).collect();
        let current = names
            .get(self.search_plugin)
            .cloned()
            .unwrap_or_else(|| "—".into());
        let resp = ui.add(
            egui::Button::new(RichText::new(elide(&current, 24)).color(p.accent)),
        );
        let was_open = self.plugin_picker_open && self.plugin_picker_source == key;
        if resp.clicked() {
            self.plugin_picker_open = !was_open;
            self.plugin_picker_source = key;
            if self.plugin_picker_open {
                self.plugin_page = self.search_plugin / Self::PLUGIN_PAGE_SIZE;
            }
        }
        if self.plugin_picker_open && self.plugin_picker_source == key {
            let pos = resp.rect.left_bottom() + vec2(0.0, 4.0);
            let area_id = egui::Id::new(("main-plugin-picker", key));
            self.plugin_picker_ui(ui.ctx(), pos, &names, p, was_open, area_id);
        }
    }

    const PLUGIN_PAGE_SIZE: usize = 16;

    /// 插件选择弹层：每页 8 个，底部翻页。
    fn plugin_picker_ui(
        &mut self,
        ctx: &egui::Context,
        pos: egui::Pos2,
        names: &[String],
        p: &Palette,
        was_open: bool,
        area_id: egui::Id,
    ) {
        let total = names.len();
        let pages = total.div_ceil(Self::PLUGIN_PAGE_SIZE).max(1);
        self.plugin_page = self.plugin_page.min(pages - 1);
        let start = self.plugin_page * Self::PLUGIN_PAGE_SIZE;
        let end = (start + Self::PLUGIN_PAGE_SIZE).min(total);

        let mut choice: Option<usize> = None;
        let mut page_delta: i32 = 0;
        let mut close = false;
        // 弹层贴右边缘时往左挪，别被窗口裁掉
        let screen = ctx.screen_rect();
        let pos = egui::pos2(pos.x.min(screen.right() - 350.0).max(screen.left() + 4.0), pos.y);
        let area = egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(p.panel)
                    .stroke(egui::Stroke::new(1.0, p.accent))
                    .inner_margin(egui::Margin::same(8))
                    .corner_radius(egui::CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.set_min_width(330.0);
                        let current_name = names
                            .get(self.search_plugin)
                            .cloned()
                            .unwrap_or_else(|| "—".into());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} {} · {} {}/{}",
                                    self.t("插件", "Plugins"),
                                    elide(&current_name, 12),
                                    self.t("第", "page"),
                                    self.plugin_page + 1,
                                    pages
                                ))
                                .small()
                                .color(p.dim),
                            );
                        });
                        ui.add_space(4.0);
                        egui::Grid::new(area_id.with("grid"))
                            .num_columns(2)
                            .spacing(vec2(6.0, 4.0))
                            .show(ui, |ui| {
                                for i in start..end {
                                    let selected = self.search_plugin == i;
                                    let name = names.get(i).map(|s| elide(s, 26)).unwrap_or_default();
                                    if ui
                                        .add_sized(
                                            vec2(155.0, 24.0),
                                            egui::Button::new(
                                                RichText::new(name)
                                                    .color(if selected { p.accent } else { p.text }),
                                            )
                                            .selected(selected),
                                        )
                                        .clicked()
                                    {
                                        choice = Some(i);
                                    }
                                    if (i - start) % 2 == 1 {
                                        ui.end_row();
                                    }
                                }
                                if (end - start) % 2 == 1 {
                                    ui.end_row();
                                }
                            });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    self.plugin_page > 0,
                                    egui::Button::new(self.t("« 第一页", "« First")),
                                )
                                .clicked()
                            {
                                self.plugin_page = 0;
                            }
                            if ui
                                .add_enabled(
                                    self.plugin_page > 0,
                                    egui::Button::new(self.t("‹ 上一页", "‹ Prev")),
                                )
                                .clicked()
                            {
                                page_delta = -1;
                            }
                            if ui
                                .add_enabled(
                                    self.plugin_page + 1 < pages,
                                    egui::Button::new(self.t("下一页 ›", "Next ›")),
                                )
                                .clicked()
                            {
                                page_delta = 1;
                            }
                            ui.add_space(6.0);
                            if ui.button(self.t("关闭", "Close")).clicked() {
                                close = true;
                            }
                        });
                    });
            });

        if let Some(i) = choice {
            self.search_plugin = i;
            self.plugin_picker_open = false;
        }
        if page_delta != 0 {
            self.plugin_page = (self.plugin_page as i32 + page_delta).clamp(0, pages as i32 - 1) as usize;
        }
        if close {
            self.plugin_picker_open = false;
        }
        // 点到弹层外面就收起
        if was_open
            && self.plugin_picker_open
            && ctx.input(|i| i.pointer.any_click())
            && !area.response.contains_pointer()
            && choice.is_none()
        {
            self.plugin_picker_open = false;
        }
    }


    fn home_tab(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::ScrollArea::vertical()
            .id_salt("home-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("AUDACIOUS · RUST")
                                .size(22.0)
                                .strong()
                                .color(p.accent),
                        );
                        ui.label(
                            RichText::new(self.t(
                                "本地播放 + 插件音源，一套界面搜歌、喜欢、攒歌单",
                                "Local playback with plugin sources: search, like and build playlists in one place",
                            ))
                            .small()
                            .color(p.dim),
                        );
                    });
                });

                // 首页主体：当前插件提供的内容
                self.plugin_home_ui(ui, p);

                ui.add_space(10.0);
                ui.separator();

                // 本地状态与快捷入口放下面，不挡插件内容
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                egui::Frame::NONE
                    .fill(p.panel)
                    .inner_margin(egui::Margin::same(12))
                    .corner_radius(egui::CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.set_min_width(360.0);
                        ui.label(
                            RichText::new(self.t("当前在线音源", "Current online source"))
                                .small()
                                .color(p.dim),
                        );
                        match self.media_plugins.get(self.search_plugin) {
                            Some(plugin) => {
                                ui.label(RichText::new(plugin.name()).size(16.0).color(p.text));
                                ui.label(
                                    RichText::new(format!(
                                        "{}  ·  {}",
                                        plugin.version(),
                                        plugin.methods_brief().join(" / ")
                                    ))
                                    .small()
                                    .color(p.dim),
                                );
                                if let Some(err) = plugin.error() {
                                    ui.label(
                                        RichText::new(err)
                                            .small()
                                            .color(egui::Color32::from_rgb(200, 80, 80)),
                                    );
                                }
                            }
                            None => {
                                ui.label(
                                    RichText::new(self.t("还没有可用的在线插件", "No plugin yet"))
                                        .color(p.dim),
                                );
                            }
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button(self.t("在线搜索", "Search")).clicked() {
                                self.tab = MainTab::Search;
                            }
                            if ui.button(self.t("管理插件", "Plugins")).clicked() {
                                self.show_plugins = true;
                            }
                        });
                    });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    self.stat_card(ui, p, &format!("{}", self.playlist.len()), self.t("歌曲", "Songs"));
                    self.stat_card(ui, p, &format!("{}", self.liked.len()), self.t("喜欢", "Liked"));
                    self.stat_card(
                        ui,
                        p,
                        &format!("{}", App::playlist_files().len()),
                        self.t("播放列表", "Playlists"),
                    );
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(self.t("添加文件", "Add files")).clicked() {
                        self.browser = Some(Browser::new(
                            self.config
                                .last_dir
                                .clone()
                                .unwrap_or_else(|| PathBuf::from(".")),
                        ));
                    }
                    if ui.button(self.t("添加目录", "Add folder")).clicked() {
                        let dir = self
                            .config
                            .last_dir
                            .clone()
                            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
                        self.add_paths(&[dir]);
                    }
                    if ui.button(self.t("保存当前列表", "Save list")).clicked() {
                        self.save_playlist();
                    }
                    if ui.button(self.t("重新扫描插件", "Rescan plugins")).clicked() {
                        self.reload_media_plugins();
                    }
                });
            });
        });

        });
    }

    /// 首页下半部分：当前插件的 `home()`（MyGo）或 `getTopLists()`（MusicFree）。
    fn plugin_home_ui(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let current = self
            .media_plugins
            .get(self.search_plugin)
            .map(|plugin| plugin.name().to_string());
        let Some(current) = current else {
            return;
        };
        // 换插件、或第一次进首页时自动拉一次
        if self.home_for.as_deref() != Some(current.as_str()) && self.home_rx.is_none() {
            self.load_home();
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        // 当前插件支持扫码登录时，在「刷新」后面给一个入口
        let login_target: Option<(String, String)> = self
            .media_plugins
            .get(self.search_plugin)
            .and_then(|pl| pl.as_mygo())
            .filter(|mygo| mygo.supports_login() || mygo.supports_sign_in())
            .map(|mygo| (mygo.manifest.id.clone(), mygo.manifest.name.clone()));
        let mut open_login_for: Option<String> = None;
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(
                RichText::new(self.t("插件主页", "Plugin home"))
                    .strong()
                    .color(p.accent),
            );
            ui.label(RichText::new(current.clone()).small().color(p.dim));
            if ui.button(self.t("刷新", "Refresh")).clicked() {
                self.load_home();
            }
            if let Some((id, name)) = &login_target {
                if ui
                    .button(self.t("扫码登录", "Scan to sign in"))
                    .on_hover_text(format!(
                        "{}{} 支持扫码登录（loginQr / loginPoll / signIn）",
                        self.t("这个插件", "This plugin"),
                        self.t("：", ": ")
                    ))
                    .clicked()
                {
                    log::info!("home 页扫码登录：{name}");
                    open_login_for = Some(id.clone());
                }
            }
            if self.home_rx.is_some() {
                ui.spinner();
                ui.label(
                    RichText::new(self.t("正在加载…", "Loading…"))
                        .small()
                        .color(p.dim),
                );
            }
        });
        if let Some(id) = open_login_for {
            self.open_login(&id);
        }
        ui.add_space(4.0);

        if let Some(err) = self.home_error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(16.0);
                ui.label(
                    RichText::new(err)
                        .small()
                        .color(egui::Color32::from_rgb(200, 80, 80)),
                );
            });
            return;
        }
        let Some(page) = self.home.clone() else {
            return;
        };

        {
            let ui = &mut *ui;
            {
                if !page.title.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(RichText::new(page.title).size(16.0).color(p.text));
                        if !page.subtitle.is_empty() {
                            ui.label(RichText::new(page.subtitle).small().color(p.dim));
                        }
                    });
                }
                if page.sections.is_empty() {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(
                            RichText::new(self.t(
                                "这个插件的首页没有内容",
                                "This plugin returned an empty home page",
                            ))
                            .small()
                            .color(p.dim),
                        );
                    });
                    return;
                }
                for (si, section) in page.sections.iter().enumerate() {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(RichText::new(&section.title).strong().color(p.accent));
                    });
                    for (ti, track) in section.tracks.iter().enumerate() {
                        let title = track["title"].as_str().unwrap_or("未命名").to_string();
                        let artist = track["artist"].as_str().unwrap_or("—").to_string();
                        let mut play = false;
                        let mut add = false;
                        ui.horizontal(|ui| {
                            ui.add_space(16.0);
                            ui.label(
                                RichText::new(format!("{}. {}", ti + 1, elide(&title, 40)))
                                    .color(p.text),
                            );
                            ui.label(RichText::new(elide(&artist, 24)).small().color(p.dim));
                            play = ui.small_button("▶").on_hover_text("播放").clicked();
                            add = ui.small_button("＋").on_hover_text("加入列表").clicked();
                        });
                        if play {
                            self.resolve_media(track, true);
                        } else if add {
                            self.resolve_media(track, false);
                        }
                    }
                    if !section.cards.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(16.0);
                            for card in &section.cards {
                                let title = card["title"].as_str().unwrap_or("歌单");
                                egui::Frame::NONE
                                    .fill(p.panel)
                                    .inner_margin(egui::Margin::symmetric(10, 6))
                                    .corner_radius(egui::CornerRadius::same(6))
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(elide(title, 22)).small().color(p.text),
                                        );
                                    });
                            }
                        });
                    }
                    let _ = si;
                }
            }
        }
    }

    fn stat_card(&self, ui: &mut egui::Ui, p: &Palette, value: &str, label: &str) {
        egui::Frame::NONE
            .fill(p.panel)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .corner_radius(egui::CornerRadius::same(6))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(value).size(20.0).strong().color(p.accent));
                    ui.label(RichText::new(label).small().color(p.dim));
                });
            });
    }

    fn search_tab(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.add_space(4.0);
        if matches!(self.search_status, SearchStatus::Busy) {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.spinner();
                ui.label(RichText::new(self.t("正在搜索…", "Searching…")).small().color(p.dim));
            });
        }

        if let SearchStatus::Error(err) = &self.search_status {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new(err)
                        .small()
                        .color(egui::Color32::from_rgb(200, 80, 80)),
                );
            });
        }

        ui.add_space(6.0);
        if self.search_results.is_empty() {
            if matches!(self.search_status, SearchStatus::Idle) {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(self.t(
                            "输入关键词，回车搜索当前音源",
                            "Type a keyword and press Enter to search the current source",
                        ))
                        .color(p.dim),
                    );
                });
            }
            return;
        }

        self.search_results_grid(ui, p);
    }

    /// 搜索结果表格：主面板「搜索」和弹出窗口共用一套列。
    fn search_results_grid(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let items = self.search_results.clone();
        let mut like_request: Option<usize> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("main-search-results")
                    .num_columns(6)
                    .striped(true)
                    .min_col_width(36.0)
                    .show(ui, |ui| {
                        for label in [
                            self.t("标题", "Title"),
                            self.t("艺人", "Artist"),
                            self.t("专辑", "Album"),
                            self.t("时长", "Length"),
                            "",
                            "",
                        ] {
                            ui.label(RichText::new(label).strong().color(p.accent));
                        }
                        ui.end_row();

                        for (i, item) in items.iter().enumerate() {
                            let title = item["title"].as_str().unwrap_or("未命名").to_string();
                            let artist = item["artist"].as_str().unwrap_or("—").to_string();
                            let album = item["album"].as_str().unwrap_or("—").to_string();
                            let length = item["duration"]
                                .as_f64()
                                .map(audcore::config::format_time)
                                .unwrap_or_else(|| "--:--".into());
                            ui.label(format!("{}. {}", i + 1, elide(&title, 28)));
                            ui.label(elide(&artist, 18));
                            ui.label(elide(&album, 18));
                            ui.label(length);
                            ui.horizontal(|ui| {
                                if ui
                                    .small_button("▶")
                                    .on_hover_text(self.t("播放", "Play"))
                                    .clicked()
                                {
                                    self.resolve_media(item, true);
                                }
                                if ui
                                    .small_button("＋")
                                    .on_hover_text(self.t("加入列表", "Add"))
                                    .clicked()
                                {
                                    self.resolve_media(item, false);
                                }
                            });
                            if ui
                                .small_button("☆")
                                .on_hover_text(self.t("加入喜欢", "Like"))
                                .clicked()
                            {
                                like_request = Some(i);
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some(i) = like_request {
            if let Some(item) = items.get(i) {
                self.like_online(item);
            }
        }
    }

    fn like_online(&mut self, item: &serde_json::Value) {
        let plugin = self
            .media_plugins
            .get(self.search_plugin)
            .map(|p| p.name())
            .unwrap_or_default();
        let entry = LikedItem {
            title: item["title"].as_str().unwrap_or("未命名").to_string(),
            artist: item["artist"].as_str().unwrap_or_default().to_string(),
            album: item["album"].as_str().unwrap_or_default().to_string(),
            duration: item["duration"].as_f64().unwrap_or(0.0),
            path: None,
            plugin: Some(plugin),
            item: Some(item.clone()),
            added_at: now_secs(),
        };
        if self.liked.add(entry) {
            self.toast("已加入喜欢");
        } else {
            self.toast("这首歌已经在喜欢里了");
        }
    }

    fn liked_tab(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let items = self.liked.items().to_vec();
        let mut play: Option<usize> = None;
        let mut remove: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("liked-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(format!(
                            "{}（{}）",
                            self.t("喜欢", "Liked"),
                            self.liked.len()
                        ))
                        .strong()
                        .color(p.accent),
                    );
                    ui.label(
                        RichText::new(self.t(
                            "搜索结果里的 ☆、歌曲右键「加入喜欢」都会到这里",
                            "Stars from search and the right-click menu land here",
                        ))
                        .small()
                        .color(p.dim),
                    );
                });
                ui.separator();

                if items.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(self.t("还没有喜欢的歌", "Nothing liked yet"))
                                .color(p.dim),
                        );
                    });
                }
                for (i, item) in items.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        if ui.small_button("▶").clicked() {
                            play = Some(i);
                        }
                        let online = item.plugin.is_some();
                        ui.label(
                            RichText::new(elide(&item.title, 34))
                                .color(if online { p.accent } else { p.text }),
                        );
                        ui.label(
                            RichText::new(elide(&item.subtitle(), 22))
                                .small()
                                .color(p.dim),
                        );
                        if online {
                            ui.label(
                                RichText::new(item.plugin.clone().unwrap_or_default())
                                    .small()
                                    .color(p.dim),
                            );
                        }
                        ui.label(RichText::new(item.length_text()).small().color(p.dim));
                        if ui.small_button(self.t("移除", "Remove")).clicked() {
                            remove = Some(i);
                        }
                    });
                }

                // 喜欢页同样显示当前插件的主页
                self.plugin_home_ui(ui, p);
            });

        if let Some(i) = play {
            if let Some(item) = items.get(i).cloned() {
                self.play_liked(item);
            }
        }
        if let Some(i) = remove {
            self.liked.remove(i);
            self.toast("已从喜欢里移除");
        }
    }

    fn play_liked(&mut self, item: LikedItem) {
        if let Some(path) = item.path {
            let index = {
                let Ok(mut pl) = self.shared.playlist.lock() else {
                    return;
                };
                pl.add_path(&path)
            };
            self.shared.send(Cmd::Refresh);
            self.shared.send(Cmd::PlayIndex(index));
            return;
        }
        let (Some(plugin_name), Some(js_item)) = (item.plugin, item.item) else {
            self.toast("这条记录没有可播放的来源");
            return;
        };
        let Some(index) = self
            .media_plugins
            .iter()
            .position(|p| p.name() == plugin_name)
        else {
            self.toast(format!("找不到插件「{plugin_name}」，先重新扫描插件"));
            return;
        };
        self.search_plugin = index;
        self.resolve_media(&js_item, true);
    }

    fn playlist_files() -> Vec<PathBuf> {
        let dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(audcore::APP_NAME)
            .join("playlists");
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|path| {
                        matches!(
                            path.extension().and_then(|e| e.to_str()),
                            Some("m3u") | Some("m3u8")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files.reverse();
        files
    }

    fn playlists_tab(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new(self.t("播放列表", "Playlists")).strong().color(p.accent));
            ui.label(
                RichText::new(self.t(
                    "「保存列表」存在这个目录，点「载入」或「播放」即可恢复",
                    "Saved lists live in this folder; load or play them here",
                ))
                .small()
                .color(p.dim),
            );
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.add_space(12.0);
                    if ui.button(self.t("保存当前列表", "Save list")).clicked() {
                        self.save_playlist();
                    }
                },
            );
        });
        ui.separator();

        let files = App::playlist_files();
        if files.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(self.t(
                        "还没有保存的播放列表 —— 点右上角「保存当前列表」",
                        "No saved playlists yet — use Save list",
                    ))
                    .color(p.dim),
                );
            });
            return;
        }

        let mut load: Option<(PathBuf, bool)> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for path in &files {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        if ui.button("▶").on_hover_text(self.t("播放", "Play")).clicked() {
                            load = Some((path.clone(), true));
                        }
                        if ui.button(self.t("载入", "Load")).clicked() {
                            load = Some((path.clone(), false));
                        }
                        let name = path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("playlist")
                            .to_string();
                        let count = audcore::plugin::load_playlist(path)
                            .map(|p| p.len())
                            .unwrap_or(0);
                        ui.label(RichText::new(name).color(p.text));
                        ui.label(
                            RichText::new(format!("{} {}", count, self.t("首", "tracks")))
                                .small()
                                .color(p.dim),
                        );
                    });
                }
            });
        if let Some((path, play)) = load {
            self.load_playlist_file(&path, play);
        }
    }

    fn load_playlist_file(&mut self, path: &std::path::Path, play: bool) {
        let paths = match audcore::plugin::load_playlist(path) {
            Ok(paths) if !paths.is_empty() => paths,
            Ok(_) => {
                self.toast("这个播放列表是空的");
                return;
            }
            Err(err) => {
                self.toast(format!("载入失败：{err}"));
                return;
            }
        };
        let start = self.playlist.len();
        let added = {
            let Ok(mut pl) = self.shared.playlist.lock() else {
                return;
            };
            ipc_server::add_paths(&mut pl, &paths)
        };
        self.shared.send(Cmd::Refresh);
        if play && added > 0 {
            self.shared.send(Cmd::PlayIndex(start));
        }
        self.toast(format!("已载入 {added} 首"));
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn open_in_file_manager(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

/// 表格里把过长的文字截断，避免把窗口撑宽。
/// 歌词页右上角的模式胶囊（选中实心、未选中描边）。
fn lyric_pill(ui: &mut egui::Ui, p: &Palette, label: &str, active: bool) -> egui::Response {
    let text = RichText::new(label).size(12.0).color(if active {
        Color32::WHITE
    } else {
        p.text
    });
    ui.add(
        egui::Button::new(text)
            .fill(if active { p.accent } else { p.bg })
            .stroke(Stroke::new(
                1.0_f32,
                if active { p.accent } else { p.panel },
            ))
            .corner_radius(egui::CornerRadius::same(6))
            .min_size(vec2(74.0, 27.0)),
    )
}

fn elide(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// 从插件返回的 JSON 里挖歌词文本：优先 rawLrc / lrc，其次 lyric(s) / text。
fn extract_lyric_text(value: &serde_json::Value) -> Option<String> {
    fn walk(value: &serde_json::Value, best: &mut Option<String>) {
        match value {
            serde_json::Value::String(s) => {
                if s.len() > best.as_ref().map(|b| b.len()).unwrap_or(0) {
                    *best = Some(s.clone());
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, best);
                }
            }
            serde_json::Value::Object(map) => {
                for (key, item) in map {
                    let lk = key.to_ascii_lowercase();
                    if lk.contains("lrc")
                        || lk.contains("lyric")
                        || lk == "text"
                        || lk == "content"
                        || lk == "contenttext"
                    {
                        walk(item, best);
                    }
                }
            }
            _ => {}
        }
    }
    let mut best = None;
    walk(value, &mut best);
    best.filter(|s| !s.trim().is_empty())
}

/// 解析 LRC：`[mm:ss.xx]文本`，返回 (秒, 文本)。
fn parse_lrc(raw: &str) -> Vec<(f64, String)> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut rest = line.trim();
        let mut stamp = None;
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[1..end];
            let mut parts = tag.split(':');
            if let (Some(mm), Some(ss)) = (parts.next(), parts.next()) {
                if let (Ok(m), Ok(s)) = (mm.parse::<f64>(), ss.parse::<f64>()) {
                    stamp = Some(m * 60.0 + s);
                }
            }
            rest = rest[end + 1..].trim_start();
        }
        if let Some(sec) = stamp {
            let text = rest.trim();
            if !text.is_empty() {
                out.push((sec, text.to_string()));
            }
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod lyric_parse_tests {
    use super::{extract_lyric_text, parse_lrc};

    #[test]
    fn parse_lrc_reads_timestamps_and_text() {
        let raw = "[00:03.00] 第一句\n[00:08.50]第二句\n不是时间戳的行\n[01:02.25] 第三句 ";
        let lines = parse_lrc(raw);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].0, 3.0);
        assert_eq!(lines[0].1, "第一句");
        assert!((lines[1].0 - 8.5).abs() < 0.001);
        assert!((lines[2].0 - 62.25).abs() < 0.001);
    }

    #[test]
    fn extract_lyric_text_prefers_longest_field() {
        let value = serde_json::json!({
            "rawLrc": "",
            "lrc": "[00:01.00] 一句\n[00:05.00] 第二句",
            "extra": { "text": "短" }
        });
        let text = extract_lyric_text(&value).unwrap();
        assert!(text.contains("第二句"));
    }
}

/// 把嵌进二进制的 `assets/icon.png` 变成 egui 纹理。
fn load_logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let bytes = include_bytes!("../assets/icon.png");
    let image = image::load_from_memory(bytes)
        .map(|img| img.to_rgba8())
        .unwrap_or_else(|_| image::RgbaImage::from_pixel(32, 32, image::Rgba([210, 120, 34, 255])));
    let size = [image.width() as usize, image.height() as usize];
    let pixels = image.into_raw();
    let color = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
    ctx.load_texture("app-logo", color, egui::TextureOptions::LINEAR)
}

/// 插件设置里的英文标签 / 说明翻成中文；没收录的原样返回。
fn zh_plugin_text(text: &str) -> String {
    let exact: &[(&str, &str)] = &[
        // 字段名
        ("Local library index", "本地曲库索引位置"),
        ("LRCLLib endpoint", "歌词库服务地址"),
        ("Cache lifetime (hours)", "歌词缓存时间（小时）"),
        ("Request timeout (ms)", "请求超时（毫秒）"),
        ("Write .lrc beside local songs", "在本地歌曲旁写入 .lrc"),
        ("Songs per background sync", "每次后台同步的歌曲数"),
        ("Background sync interval (minutes)", "后台同步间隔（分钟）"),
        ("Source script URLs", "曲源脚本地址列表"),
        ("Source script (pasted)", "粘贴的曲源脚本"),
        ("Catalog", "目录"),
        ("Catalog browsing", "目录浏览"),
        ("Sync credential (optional)", "同步凭据（可选）"),
        ("Download quality (lossless / high / standard)", "下载音质（无损 / 高 / 标准）"),
        // 说明
        (
            "Claims the Concept Edition's free VIP once a day while signed in. Enter 0 to turn it off.",
            "登录后每天可领取一次概念版免费 VIP；填 0 关闭。",
        ),
        (
            "Path to MusicGo's local/index.json. The default is the one beside this plugin's data folder.",
            "MusicGo 本地曲库索引 local/index.json 的路径；留空时用插件数据目录旁的那一份。",
        ),
        (
            "A LRCLLib-compatible server. Defaults: https://lrclib.net. Its host must be in the plugin's network permission.",
            "兼容 LRCLLib 的服务器地址，默认 https://lrclib.net；该域名要写进插件的网络权限里。",
        ),
        (
            "How long a found lyric is kept before LRCLLib is asked again. Default: 168 (one week). Set 0 to turn caching off.",
            "找到的歌词缓存多久后重新向 LRCLLib 请求。默认 168 小时（一周），填 0 关闭缓存。",
        ),
        ("How long to wait for LRCLLib per request. Default: 8000.", "单次请求 LRCLLib 的最长等待时间，默认 8000 毫秒。"),
        (
            "Default: true. Writes found lyrics beside your audio files, so they show while playing from Local Files too. Existing .lrc files are never overwritten. Set to false to stop.",
            "默认开启：把找到的歌词写在音频文件旁边，本地文件播放时也能显示；已有的 .lrc 不会被覆盖，填 false 关闭。",
        ),
        (
            "Default: 25. How many songs may be looked up each time the app starts or plays a song; keeps the load on the public LRCLLib service small.",
            "默认 25：每次启动或播放时最多补多少首歌的歌词，避免给公共 LRCLLib 服务造成压力。",
        ),
        (
            "Default: 15. Minimum minutes between two background lookups.",
            "默认 15：两次后台补词之间的最小间隔（分钟）。",
        ),
        (
            "One or more LX Music custom source js links, separated by new lines. Commas or semicolons only split when another link follows. Every source is asked at once when a song plays, and the fastest answer plays.",
            "一个或多个 LX Music 自定义源 js 链接，每行一个。逗号/分号只有在后面还跟链接时才作为分隔符。播放时所有源同时请求，谁先返回就用谁。",
        ),
        (
            "Optional. Paste a script here and it joins the one above.",
            "可选：把脚本内容粘贴到这里，会和上面的链接一起加载。",
        ),
        (
            "wy, b, or leave it empty for both. These are source keys, not catalog names; the choice covers search and the discovery pages.",
            "填 wy、b，或留空表示两者都用。这里填的是音源标识而不是目录名，会同时影响搜索和发现页。",
        ),
        (
            "on or off. Turn it off to hide charts, playlists, daily picks, synced lists and the account menu, leaving search and the source adapter.",
            "开或关。关掉后隐藏排行榜、歌单、每日推荐、同步列表和账号菜单，只留搜索和音源适配。",
        ),
        (
            "Only needed when QR sign-in is unavailable. Paste the cookie of your own signed-in web session; it stays on this device and is used just to read your own lists.",
            "只有扫码登录不可用时才需要。粘贴你自己网页端登录后的 Cookie；只保存在本机，仅用于读取你自己的歌单。",
        ),
        (
            "默认无损：优先 FLAC，没有 FLAC 时自动回退到最高音质 MP3(320)，可改为 high(MP3 320) 或 standard(MP3 128)。",
            "默认无损：优先 FLAC，没有 FLAC 时自动回退到最高音质 MP3(320)，可改为 high(MP3 320) 或 standard(MP3 128)。",
        ),
        (
            "Default: lossless first, then fall back to high quality MP3 (320) automatically if LOAC is absent. Can be set to high (MP3 320) or standard (MP3 128).",
            "默认无损：优先 FLAC，没有 FLAC 时自动回退到最高音质 MP3(320)；也可以改成 high(MP3 320) 或 standard(MP3 128)。",
        ),
    ];
    let trimmed = text.trim();
    if let Some((_, zh)) = exact.iter().find(|(en, _)| *en == trimmed) {
        return (*zh).to_string();
    }
    text.to_string()
}

#[cfg(test)]
mod zh_text_tests {
    use super::zh_plugin_text;

    #[test]
    fn translates_known_labels() {
        assert_eq!(zh_plugin_text("LRCLLib endpoint"), "歌词库服务地址");
        assert_eq!(zh_plugin_text("  Request timeout (ms)  "), "请求超时（毫秒）");
    }

    #[test]
    fn keeps_unknown_text() {
        assert_eq!(zh_plugin_text("Some custom option"), "Some custom option");
    }
}

/// 顶部搜索条右侧的小方按钮（选中时强调色描边 + 浅底）。
fn top_icon_button(ui: &mut egui::Ui, p: &Palette, label: &str, active: bool) -> egui::Response {
    let text = RichText::new(label).size(13.0).color(if active { p.accent } else { p.dim });
    ui.add(
        egui::Button::new(text)
            .fill(if active { p.accent_soft } else { p.panel_alt })
            .stroke(Stroke::new(1.0_f32, if active { p.accent } else { p.panel }))
            .corner_radius(egui::CornerRadius::same(6))
            .min_size(vec2(30.0, 28.0)),
    )
}

/// 把内嵌封面解码成 40px 缩略图纹理；失败返回 None（界面退回字母块）。
fn cover_texture(ctx: &egui::Context, bytes: &[u8], key: &str) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?.thumbnail(40, 40).to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = image.into_raw();
    let color = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
    Some(ctx.load_texture(
        format!("cover-{key}"),
        color,
        egui::TextureOptions::LINEAR,
    ))
}

/// 圆形封面纹理：先把封面缩到 d×d，再把圆外的像素 alpha 置 0。
fn round_texture(
    ctx: &egui::Context,
    bytes: &[u8],
    key: &str,
    d: u32,
) -> Option<egui::TextureHandle> {
    let mut image = image::load_from_memory(bytes).ok()?.thumbnail(d, d).to_rgba8();
    let radius = d as f32 * 0.5;
    for (x, y, px) in image.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - radius;
        let dy = y as f32 + 0.5 - radius;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist > radius {
            px[3] = 0;
        } else if dist > radius - 1.2 {
            // 边缘一像素做点抗锯齿
            let edge = (radius - dist) / 1.2;
            px[3] = (px[3] as f32 * edge.clamp(0.0, 1.0)) as u8;
        }
    }
    let size = [image.width() as usize, image.height() as usize];
    let pixels = image.into_raw();
    let color = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
    Some(ctx.load_texture(
        format!("cover-round-{key}"),
        color,
        egui::TextureOptions::LINEAR,
    ))
}

fn mini_icon_color(resp: &egui::Response) -> Color32 {
    if resp.hovered() {
        Color32::from_rgb(224, 228, 234)
    } else {
        Color32::from_rgb(150, 156, 164)
    }
}

/// 心：两个圆 + 一个朝下的三角拼成，顶上留出中间的凹口。
fn mini_heart_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "heart", color);
}

/// 上一首：靠左一根竖条 + 朝左的三角（条和三角尖贴着）。
fn mini_prev_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "player-skip-back", color);
}

/// 下一首：朝右的三角 + 靠右的竖条。
fn mini_next_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "player-skip-forward", color);
}

/// 播放：白色实心三角，底边略短、尖端略钝，跟设计稿的圆内三角一致。
fn mini_play_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let inner = rect.shrink(rect.width() * 0.20);
    tabler(painter, inner, "filled-player-play", color);
}

/// 暂停：两根白竖条
fn mini_pause_icon(painter: &egui::Painter, rect: egui::Rect) {
    let inner = rect.shrink(rect.width() * 0.20);
    tabler(painter, inner, "filled-player-pause", Color32::WHITE);
}

/// 待播清单：三条横线，最后一条末尾带个指向右下的小箭头（看齐设计稿那个「队列」图标）。
fn mini_list_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "playlist", color);
}

/// 关闭：两个交叉的短线
fn mini_close_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "x", color);
}

#[cfg(test)]
mod round_cover_tests {
    #[test]
    fn round_mask_clears_corners() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/screenshot-ui-topbar-5.12.2.png");
        let bytes = std::fs::read(path).expect("测试图片");
        let img = image::load_from_memory(&bytes).unwrap().thumbnail(88, 88).to_rgba8();
        let radius = 88.0_f32 * 0.5;
        let px = img.get_pixel(1, 1);
        let dx = 1.5 - radius;
        let dy = 1.5 - radius;
        assert!((dx * dx + dy * dy).sqrt() > radius, "角上那点确实在圆外");
        let center = img.get_pixel(44, 44);
        assert!(center[3] > 0, "中心不能是透明的");
        let _ = px;
    }
}

/// 喜欢：实心心形（两个圆 + 一个尖角三角），跟迷你卡片里那颗心同一套画法。
fn heart_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "heart", color);
}

/// 工具栏上的图标按钮：30×30，悬停变亮 + 淡淡底色，带 tooltip。
fn icon_button(
    ui: &mut egui::Ui,
    tip: &str,
    p: &crate::theme::Palette,
    draw: fn(&egui::Painter, egui::Rect, Color32),
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), p.accent_soft);
    }
    let color = if resp.hovered() { p.accent } else { p.text };
    draw(ui.painter(), rect, color);
    resp.on_hover_text(tip).clicked()
}

/// 工具栏中间的播放键：强调色圆底，播放中双竖条、暂停白三角。
fn transport_button(ui: &mut egui::Ui, p: &crate::theme::Palette, tip: &str, playing: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(38.0, 32.0), Sense::click());
    let center = rect.center();
    ui.painter().circle_filled(center, 16.0, p.accent);
    if playing {
        mini_pause_icon(ui.painter(), rect);
    } else {
        mini_play_icon(ui.painter(), rect, Color32::WHITE);
    }
    resp.on_hover_text(tip).clicked()
}

/// 文件：圆角文档轮廓 + 右下角加号。
fn icon_file_add(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "file-plus", color);
}

/// 目录：文件夹轮廓 + 加号。
fn icon_folder_add(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "folder-plus", color);
}

/// 保存列表：三条横线 + 底下一个向下箭头。
fn icon_save_list(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "device-floppy", color);
}

/// 均衡器：三根竖线 + 三个滑块点。
fn icon_eq(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "adjustments-horizontal", color);
}

/// 插件：2×2 的小方块。
fn icon_plugins(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "puzzle", color);
}

/// 设置：齿轮（圆 + 外侧四个短齿）。
fn icon_settings(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "settings", color);
}

/// 迷你：一个小圆角窗口 + 里面一条细进度。
fn icon_mini(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "picture-in-picture", color);
}

/// 关于：圆圈里一个 i。
fn icon_info(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "info-circle", color);
}

/// 停止：一个小方块。
fn icon_stop(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    tabler(painter, rect, "filled-player-stop", color);
}

/// Tabler 图标：白像素 + alpha 的 PNG，绘制时用 tint 上色。
fn tabler_bytes(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "file-plus" => include_bytes!("../assets/tabler/file-plus.png"),
        "folder-plus" => include_bytes!("../assets/tabler/folder-plus.png"),
        "device-floppy" => include_bytes!("../assets/tabler/device-floppy.png"),
        "adjustments-horizontal" => include_bytes!("../assets/tabler/adjustments-horizontal.png"),
        "puzzle" => include_bytes!("../assets/tabler/puzzle.png"),
        "settings" => include_bytes!("../assets/tabler/settings.png"),
        "picture-in-picture" => include_bytes!("../assets/tabler/picture-in-picture.png"),
        "info-circle" => include_bytes!("../assets/tabler/info-circle.png"),
        "heart" => include_bytes!("../assets/tabler/heart.png"),
        "filled-heart" => include_bytes!("../assets/tabler/filled-heart.png"),
        "player-skip-back" => include_bytes!("../assets/tabler/player-skip-back.png"),
        "player-skip-forward" => include_bytes!("../assets/tabler/player-skip-forward.png"),
        "filled-player-play" => include_bytes!("../assets/tabler/filled-player-play.png"),
        "filled-player-pause" => include_bytes!("../assets/tabler/filled-player-pause.png"),
        "filled-player-skip-back" => include_bytes!("../assets/tabler/filled-player-skip-back.png"),
        "filled-player-skip-forward" => include_bytes!("../assets/tabler/filled-player-skip-forward.png"),
        "filled-player-stop" => include_bytes!("../assets/tabler/filled-player-stop.png"),
        "playlist" => include_bytes!("../assets/tabler/playlist.png"),
        "x" => include_bytes!("../assets/tabler/x.png"),
        "palette" => include_bytes!("../assets/tabler/palette.png"),
        "search" => include_bytes!("../assets/tabler/search.png"),
        "arrows-shuffle" => include_bytes!("../assets/tabler/arrows-shuffle.png"),
        "repeat" => include_bytes!("../assets/tabler/repeat.png"),
        "repeat-once" => include_bytes!("../assets/tabler/repeat-once.png"),
        "volume-2" => include_bytes!("../assets/tabler/volume-2.png"),
        "volume-off" => include_bytes!("../assets/tabler/volume-off.png"),
        "music" => include_bytes!("../assets/tabler/music.png"),
        "trash" => include_bytes!("../assets/tabler/trash.png"),
        "refresh" => include_bytes!("../assets/tabler/refresh.png"),
        "check" => include_bytes!("../assets/tabler/check.png"),
        "chevron-down" => include_bytes!("../assets/tabler/chevron-down.png"),
        "plus" => include_bytes!("../assets/tabler/plus.png"),
        "waveform" => include_bytes!("../assets/tabler/waveform.png"),
        "disc" => include_bytes!("../assets/tabler/disc.png"),
        "plug" => include_bytes!("../assets/tabler/plug.png"),
        "file-music" => include_bytes!("../assets/tabler/file-music.png"),
        "loader-2" => include_bytes!("../assets/tabler/loader-2.png"),
        "checkbox" => include_bytes!("../assets/tabler/checkbox.png"),
        "arrow-up" => include_bytes!("../assets/tabler/arrow-up.png"),
        "arrow-down" => include_bytes!("../assets/tabler/arrow-down.png"),
        "color-picker" => include_bytes!("../assets/tabler/color-picker.png"),
        "playlist-add" => include_bytes!("../assets/tabler/playlist-add.png"),
        "home" => include_bytes!("../assets/tabler/home.png"),
        _ => return None,
    })
}

/// 图标纹理按名字缓存到 ctx 的临时数据里，只解码一次。
fn tabler_texture(ctx: &egui::Context, name: &str) -> Option<egui::TextureHandle> {
    let id = egui::Id::new(("tabler-icon", name));
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(tex);
    }
    let bytes = tabler_bytes(name)?;
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &img.into_raw());
    let tex = ctx.load_texture(format!("tabler-{name}"), color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

/// 在 rect 里画一个 Tabler 图标，按 color 上色（图标本身是白的）。
fn tabler(painter: &egui::Painter, rect: egui::Rect, name: &str, color: Color32) {
    if let Some(tex) = tabler_texture(painter.ctx(), name) {
        // Tabler 的 24 网格留白较大，画的时候放大一点才和字号视觉重量匹配
        let side = rect.width().min(rect.height()) * 1.12;
        let r = egui::Rect::from_center_size(rect.center(), vec2(side, side));
        painter.image(
            tex.id(),
            r,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            color,
        );
    }
}
