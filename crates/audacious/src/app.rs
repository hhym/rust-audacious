//! 主界面：仿 Audacious GTK 界面的 egui 实现。
//!
//! 布局自上而下：菜单栏 / 工具栏（播放控制 + 进度 + 音量）/ 播放列表表格 /
//! Now Playing 信息带（封面 + 频谱）/ 状态栏；迷你模式下只保留信息带。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use audcore::{plugin, Config, Playlist, RepeatMode};
use egui::{
    Align2, Color32, CornerRadius, FontId, RichText, Sense, Stroke, StrokeKind, vec2,
};

use crate::engine::{Cmd, PlaybackState, Shared, Status};
use crate::ipc_server;
use crate::theme::{self, Palette};

pub struct Browser {
    dir: PathBuf,
    entries: Vec<(PathBuf, bool)>,
    selected: Option<usize>,
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
        };
        app.plugins = app.collect_plugins();

        // 调试用：AUDACIOUS_DEBUG_WINDOWS=eq,settings,plugins,browser,about
        // 启动时打开指定窗口，方便截图与自动化验收。
        if let Ok(list) = std::env::var("AUDACIOUS_DEBUG_WINDOWS") {
            for name in list.split(',') {
                match name.trim() {
                    "eq" => app.show_eq = true,
                    "settings" => app.show_settings = true,
                    "plugins" => app.show_plugins = true,
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

        if self.config.mini != self.mini_applied {
            self.mini_applied = self.config.mini;
            let size = if self.config.mini {
                vec2(660.0, 150.0)
            } else {
                vec2(1180.0, 720.0)
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }

        let p = theme::palette(&self.config);
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
            self.add_paths(&dropped);
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
            self.playlist_panel(ctx, &p);
        }

        self.theme_menu_ui(ctx, &p);
        self.settings_ui(ctx, &p);
        self.eq_ui(ctx, &p);
        self.plugins_ui(ctx, &p);
        self.about_ui(ctx, &p);

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
                    if ui.button(self.t("保存列表", "Save list")).clicked() {
                        self.save_playlist();
                    }
                    ui.separator();
                    if ui.button(self.t("均衡器", "Equalizer")).clicked() {
                        self.show_eq = true;
                    }
                    if ui.button(self.t("插件", "Plugins")).clicked() {
                        self.show_plugins = true;
                    }
                    if ui.button(self.t("设置", "Settings")).clicked() {
                        self.show_settings = true;
                    }
                    ui.separator();
                    if ui.button(self.t("迷你", "Mini")).clicked() {
                        self.config.mini = true;
                        let _ = self.config.save();
                    }
                    if ui.button(self.t("关于", "About")).clicked() {
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
                    if ui.button(self.t("上一首", "Prev")).clicked() {
                        self.shared.send(Cmd::Prev);
                    }
                    let main_label = if playing {
                        self.t("暂停", "Pause")
                    } else {
                        self.t("播放", "Play")
                    };
                    if ui
                        .add(
                            egui::Button::new(RichText::new(main_label).strong())
                                .fill(p.accent_soft)
                                .stroke(Stroke::new(1.0_f32, p.accent)),
                        )
                        .clicked()
                    {
                        self.shared.send(Cmd::Toggle);
                    }
                    if ui.button(self.t("停止", "Stop")).clicked() {
                        self.shared.send(Cmd::Stop);
                    }
                    if ui.button(self.t("下一首", "Next")).clicked() {
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

    fn playlist_panel(&mut self, ctx: &egui::Context, p: &Palette) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.bg))
            .show(ctx, |ui| {
                self.playlist_table(ui, p);
            });
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
        let mut play_request = None;
        let mut selection = None;
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
                    if selected == Some(index) {
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
                        selection = Some(index);
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

        if let Some(i) = selection {
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

    fn nowband_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        egui::TopBottomPanel::bottom("nowband")
            .exact_height(96.0)
            .frame(egui::Frame::NONE.fill(if p.dark {
                Color32::from_rgb(17, 18, 21)
            } else {
                p.panel_alt
            }))
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(78.0, 78.0), Sense::hover());
                    let painter = ui.painter();
                    painter.rect_filled(rect, CornerRadius::same(6), p.accent_soft);
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(6),
                        Stroke::new(1.0_f32, p.accent),
                        StrokeKind::Inside,
                    );
                    let initial = self
                        .state
                        .title
                        .chars()
                        .next()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "♪".into());
                    painter.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        initial,
                        FontId::proportional(30.0),
                        p.accent,
                    );

                    ui.add_space(14.0);
                    ui.vertical(|ui| {
                        ui.add_space(10.0);
                        let title = if self.state.title.is_empty() {
                            self.t("未在播放", "Nothing playing").to_string()
                        } else {
                            self.state.title.clone()
                        };
                        ui.label(RichText::new(title).size(17.0).color(p.text).strong());
                        ui.add_space(2.0);
                        let sub = if self.state.title.is_empty() {
                            self.t("拖入音频文件开始", "Drop audio to start").to_string()
                        } else if self.state.artist.is_empty() && self.state.album.is_empty() {
                            self.t("无标签信息", "No tag info").to_string()
                        } else {
                            format!("{} · {}", self.state.artist, self.state.album)
                        };
                        ui.label(RichText::new(sub).size(12.0).color(p.dim));
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} / {}",
                                    theme::time_text(self.state.position),
                                    theme::time_text(self.state.length)
                                ))
                                .monospace()
                                .size(11.5)
                                .color(p.dim),
                            );
                            if self.state.status == Status::Playing {
                                ui.label(
                                    RichText::new(format!("· {}", self.state.message))
                                        .size(11.0)
                                        .color(p.dim),
                                );
                            }
                        });
                    });

                    // 频谱与采样率固定在右下角，不随信息文字宽度浮动
                    let panel = ui.max_rect();
                    let spec = egui::Rect::from_min_size(
                        egui::pos2(panel.right() - 16.0 - 220.0, panel.top() + 12.0),
                        vec2(220.0, 52.0),
                    );
                    paint_spectrum(ui.painter(), spec, &self.spectrum, p);
                    ui.painter().text(
                        egui::pos2(spec.left(), spec.bottom() + 12.0),
                        Align2::LEFT_CENTER,
                        format!(
                            "{} Hz  ·  {}",
                            self.state.sample_rate,
                            self.state.status.label()
                        ),
                        FontId::proportional(11.0),
                        p.dim,
                    );
                });
            });
    }

    fn mini_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.panel))
            .show(ctx, |ui| {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    let (rect, _) = ui.allocate_exact_size(vec2(60.0, 60.0), Sense::hover());
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(6), p.accent_soft);
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        let title = if self.state.title.is_empty() {
                            self.t("未在播放", "Nothing playing").to_string()
                        } else {
                            self.state.title.clone()
                        };
                        ui.label(RichText::new(title).size(15.0).color(p.text).strong());
                        ui.label(
                            RichText::new(format!(
                                "{} · {}",
                                self.state.artist,
                                theme::time_text(self.state.position)
                            ))
                            .small()
                            .color(p.dim),
                        );
                        let mut pos = self.state.position;
                        let len = self.state.length.max(1.0);
                        if ui
                            .add(
                                egui::Slider::new(&mut pos, 0.0..=len)
                                    .show_value(false),
                            )
                            .changed()
                        {
                            self.shared.send(Cmd::Seek(pos));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        if ui.button(self.t("展开", "Expand")).clicked() {
                            self.config.mini = false;
                            let _ = self.config.save();
                        }
                        if ui.button(self.t("下一首", "Next")).clicked() {
                            self.shared.send(Cmd::Next);
                        }
                        if ui.button(self.t("停止", "Stop")).clicked() {
                            self.shared.send(Cmd::Stop);
                        }
                        let label = if self.state.status == Status::Playing {
                            self.t("暂停", "Pause")
                        } else {
                            self.t("播放", "Play")
                        };
                        if ui
                            .add(
                                egui::Button::new(RichText::new(label).strong())
                                    .fill(p.accent_soft),
                            )
                            .clicked()
                        {
                            self.shared.send(Cmd::Toggle);
                        }
                        if ui.button(self.t("上一首", "Prev")).clicked() {
                            self.shared.send(Cmd::Prev);
                        }
                    });
                });
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

        // 每个主题的底色小方块，先算好避免在闭包里反复借用 self
        let items: Vec<(audcore::Theme, &'static str, egui::Color32, bool)> =
            audcore::Theme::all()
                .into_iter()
                .map(|t| {
                    let bg = theme::palette_for(t, self.config.accent).bg;
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
                                    .allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
                                ui.painter().rect_filled(
                                    rect,
                                    egui::CornerRadius::same(3),
                                    *bg,
                                );
                                ui.painter().rect_stroke(
                                    rect,
                                    egui::CornerRadius::same(3),
                                    Stroke::new(1.0_f32, p.dim),
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
                ui.horizontal(|ui| {
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

    fn plugins_ui(&mut self, ctx: &egui::Context, p: &Palette) {
        let mut open = self.show_plugins;
        egui::Window::new(self.t("插件", "Plugins"))
            .open(&mut open)
            .default_width(460.0)
            .show(ctx, |ui| {
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
    let r = rect.shrink(4.0);
    let p = |fx: f32, fy: f32| egui::pos2(r.left() + r.width() * fx, r.top() + r.height() * fy);

    // 身体
    painter.rect_filled(
        egui::Rect::from_min_max(p(0.34, 0.28), p(0.66, 0.92)),
        egui::CornerRadius::same(2),
        color,
    );
    // 左右袖子（梯形）
    painter.add(egui::Shape::convex_polygon(
        vec![p(0.35, 0.28), p(0.12, 0.38), p(0.18, 0.56), p(0.35, 0.48)],
        color,
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![p(0.65, 0.28), p(0.88, 0.38), p(0.82, 0.56), p(0.65, 0.48)],
        color,
        Stroke::NONE,
    ));
    // 领口：两条斜线勾出 V 形
    painter.line_segment([p(0.42, 0.28), p(0.50, 0.42)], Stroke::new(1.5_f32, collar));
    painter.line_segment([p(0.58, 0.28), p(0.50, 0.42)], Stroke::new(1.5_f32, collar));
}
