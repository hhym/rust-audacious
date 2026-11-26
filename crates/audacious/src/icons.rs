//! IconPark 图标的运行时纹理缓存。
//!
//! 图标都是白色单色 PNG（assets/icons 下），绘制时用 tint 染成主题色，
//! 这样墨水屏（纯黑）和普通主题共用一套素材。素材由 IconPark 的 SVG 渲染而来。

use std::collections::HashMap;

use egui::{Color32, Context, TextureHandle, TextureOptions};

const SOURCES: &[(&str, &[u8])] = &[
    ("file_add", include_bytes!("../assets/icons/file_add.png")),
    ("folder", include_bytes!("../assets/icons/folder.png")),
    ("save", include_bytes!("../assets/icons/save.png")),
    ("setting", include_bytes!("../assets/icons/setting.png")),
    ("plugin", include_bytes!("../assets/icons/plugin.png")),
    ("theme", include_bytes!("../assets/icons/theme.png")),
    ("home", include_bytes!("../assets/icons/home.png")),
    ("search", include_bytes!("../assets/icons/search.png")),
    ("liked", include_bytes!("../assets/icons/liked.png")),
    ("songs", include_bytes!("../assets/icons/songs.png")),
    ("lyrics", include_bytes!("../assets/icons/lyrics.png")),
    ("play", include_bytes!("../assets/icons/play.png")),
    ("pause", include_bytes!("../assets/icons/pause.png")),
    ("prev", include_bytes!("../assets/icons/prev.png")),
    ("next", include_bytes!("../assets/icons/next.png")),
    ("sound", include_bytes!("../assets/icons/sound.png")),
    ("list", include_bytes!("../assets/icons/list.png")),
    ("shuffle", include_bytes!("../assets/icons/shuffle.png")),
    ("close", include_bytes!("../assets/icons/close.png")),
    ("plus", include_bytes!("../assets/icons/plus.png")),
];

#[derive(Default)]
pub struct IconSet {
    cache: HashMap<&'static str, TextureHandle>,
}

impl IconSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取一张图标纹理，第一次用到时才解码上传。
    pub fn texture(&mut self, ctx: &Context, name: &'static str) -> TextureHandle {
        if let Some(tex) = self.cache.get(name) {
            return tex.clone();
        }
        let bytes = SOURCES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, b)| *b)
            .unwrap_or_else(|| SOURCES[0].1);
        let img = image::load_from_memory(bytes)
            .expect("图标 PNG 解码失败")
            .to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        let tex = ctx.load_texture(format!("icon-{name}"), color, TextureOptions::LINEAR);
        self.cache.insert(name, tex.clone());
        tex
    }

    /// 在一帧里把图标画到指定矩形，按 tint 上色。
    pub fn paint(
        &mut self,
        ctx: &Context,
        painter: &egui::Painter,
        name: &'static str,
        rect: egui::Rect,
        tint: Color32,
    ) {
        let tex = self.texture(ctx, name);
        painter.image(
            tex.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
    }
}
