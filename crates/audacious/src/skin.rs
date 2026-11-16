//! Winamp 2.x 经典皮肤（.wsz）支持。
//!
//! 解析思路：.wsz 就是个 zip，里面是一堆位图 + `pledit.txt` / `viscolor.txt`。
//! 这里只做「解包 → 解码成贴图 → 按精灵坐标切片绘制」，布局坐标参考 Winamp 2.91
//! 的 275×116 主窗口网格（和 Webamp 的 CSS 布局一致）。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use egui::{Color32, Painter, Pos2, Rect, TextureHandle, TextureOptions};

/// 主窗口逻辑尺寸
pub const MAIN_W: f32 = 275.0;
pub const MAIN_H: f32 = 116.0;

/// `text.bmp` 字模：5×6，按 (行, 列) 取图。键是小写字符，查不到的字符画空格。
const CHAR_W: f32 = 5.0;
const CHAR_H: f32 = 6.0;

pub struct Skin {
    pub name: String,
    textures: HashMap<String, TextureHandle>,
    sizes: HashMap<String, (u32, u32)>,
    pub pledit: Pledit,
}

/// `pledit.txt` 里的播放列表配色（缺省用经典绿）。
#[derive(Clone, Debug)]
pub struct Pledit {
    pub normal: Color32,
    pub current: Color32,
    pub normal_bg: Color32,
    pub selected_bg: Color32,
    pub text: Color32,
}

impl Default for Pledit {
    fn default() -> Self {
        Pledit {
            normal: Color32::from_rgb(0x00, 0xFF, 0x00),
            current: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            normal_bg: Color32::from_rgb(0x00, 0x00, 0x00),
            selected_bg: Color32::from_rgb(0x00, 0x00, 0x80),
            text: Color32::from_rgb(0x00, 0xFF, 0x00),
        }
    }
}

impl Skin {
    /// 从 .wsz（zip）加载。
    pub fn load_wsz(ctx: &egui::Context, path: &Path) -> Result<Skin> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("打不开皮肤 {}", path.display()))?;
        let mut zip = zip::ZipArchive::new(file).context("不是合法的 zip/wsz")?;
        let mut images: HashMap<String, image::RgbaImage> = HashMap::new();
        let mut pledit_text = String::new();

        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            // 有些皮肤把位图塞在子目录里（3DNow/main.bmp），统一取 basename
            let raw = entry.name().to_string();
            let base = raw
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&raw)
                .to_ascii_lowercase();
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes)?;

            if base.ends_with(".txt") && base.starts_with("pledit") {
                pledit_text = String::from_utf8_lossy(&bytes).into_owned();
                continue;
            }
            if !(base.ends_with(".bmp") || base.ends_with(".png")) {
                continue;
            }
            match image::load_from_memory(&bytes) {
                Ok(img) => {
                    images.insert(base, img.to_rgba8());
                }
                Err(err) => log::warn!("皮肤里的 {raw} 解不开：{err}"),
            }
        }

        let pledit = Pledit::parse(&pledit_text);
        Ok(Self::build(ctx, path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "skin".into()), images, pledit))
    }

    fn build(
        ctx: &egui::Context,
        name: String,
        images: HashMap<String, image::RgbaImage>,
        pledit: Pledit,
    ) -> Skin {
        let mut textures = HashMap::new();
        let mut sizes = HashMap::new();
        for (key, img) in images {
            let size = (img.width(), img.height());
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [size.0 as usize, size.1 as usize],
                img.as_raw(),
            );
            // 像素风要最近邻，否则缩放后糊成一团
            let handle = ctx.load_texture(format!("skin/{key}"), color, TextureOptions::NEAREST);
            textures.insert(key.clone(), handle);
            sizes.insert(key, size);
        }
        Skin {
            name,
            textures,
            sizes,
            pledit,
        }
    }

    pub fn has(&self, sheet: &str) -> bool {
        self.textures.contains_key(sheet)
    }

    pub fn size(&self, sheet: &str) -> Option<(u32, u32)> {
        self.sizes.get(sheet).copied()
    }

    /// 按源矩形切一块贴图画到 `dst`。
    #[allow(clippy::too_many_arguments)]
    pub fn sprite(
        &self,
        painter: &Painter,
        sheet: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        dst: Rect,
        tint: Color32,
    ) {
        let Some(tex) = self.textures.get(sheet) else {
            return;
        };
        let Some((sw, sh)) = self.sizes.get(sheet).copied() else {
            return;
        };
        if sw == 0 || sh == 0 {
            return;
        }
        let uv = Rect::from_min_max(
            Pos2::new(x / sw as f32, y / sh as f32),
            Pos2::new((x + w) / sw as f32, (y + h) / sh as f32),
        );
        painter.image(tex.id(), dst, uv, tint);
    }

    /// 用 `text.bmp` 写字。返回写完后的 x 坐标。
    pub fn text(
        &self,
        painter: &Painter,
        s: &str,
        origin: Pos2,
        scale: f32,
        max_x: f32,
        tint: Color32,
    ) -> f32 {
        let mut x = origin.x;
        for ch in s.chars() {
            let cell = char_cell(ch);
            if let Some((row, col)) = cell {
                if x + CHAR_W * scale > max_x {
                    break;
                }
                let dst = Rect::from_min_size(
                    Pos2::new(x, origin.y),
                    egui::vec2(CHAR_W * scale, CHAR_H * scale),
                );
                self.sprite(
                    painter,
                    "text.bmp",
                    col as f32 * CHAR_W,
                    row as f32 * CHAR_H,
                    CHAR_W,
                    CHAR_H,
                    dst,
                    tint,
                );
            }
            x += CHAR_W * scale;
        }
        x
    }

    /// 用 `numbers.bmp` 画数字串（mm:ss 之类）。
    pub fn digits(
        &self,
        painter: &Painter,
        s: &str,
        origin: Pos2,
        scale: f32,
        tint: Color32,
    ) {
        let mut x = origin.x;
        for ch in s.chars() {
            let col = match ch {
                '0'..='9' => ch as u32 - '0' as u32,
                ' ' => 10,
                _ => continue,
            };
            let dst = Rect::from_min_size(
                Pos2::new(x, origin.y),
                egui::vec2(9.0 * scale, 13.0 * scale),
            );
            self.sprite(
                painter,
                "numbers.bmp",
                col as f32 * 9.0,
                0.0,
                9.0,
                13.0,
                dst,
                tint,
            );
            x += 9.0 * scale;
        }
    }
}

fn char_cell(ch: char) -> Option<(usize, usize)> {
    let lower = ch.to_ascii_lowercase();
    // 常用字符（来自 Winamp 的 FONT_LOOKUP 表，行/列）
    let cell = match lower {
        'a'..='z' => (0, lower as usize - 'a' as usize),
        '"' => (0, 26),
        '@' => (0, 27),
        ' ' => (0, 30),
        '0'..='9' => (1, lower as usize - '0' as usize),
        '.' => (1, 11),
        ':' => (1, 12),
        '(' => (1, 13),
        ')' => (1, 14),
        '-' => (1, 15),
        '\'' => (1, 16),
        '!' => (1, 17),
        '_' => (1, 18),
        '+' => (1, 19),
        '\\' => (1, 20),
        '/' => (1, 21),
        '[' | '<' | '{' => (1, 22),
        ']' | '>' | '}' => (1, 23),
        '^' => (1, 24),
        '&' => (1, 25),
        '%' => (1, 26),
        ',' => (1, 27),
        '=' => (1, 28),
        '$' => (1, 29),
        '#' => (1, 30),
        '?' => (2, 3),
        '*' => (2, 4),
        _ => return None,
    };
    Some(cell)
}

impl Pledit {
    /// 解析 `pledit.txt`（INI 格式，颜色是 `#RRGGBB`）。
    pub fn parse(text: &str) -> Pledit {
        let mut colors = HashMap::new();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].to_ascii_lowercase();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            if section == "text" {
                colors.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        let get = |k: &str, default: Color32| {
            colors
                .get(k)
                .and_then(|v| parse_hex_color(v))
                .unwrap_or(default)
        };
        let d = Pledit::default();
        Pledit {
            normal: get("normal", d.normal),
            current: get("current", d.current),
            normal_bg: get("normalbg", d.normal_bg),
            selected_bg: get("selectedbg", d.selected_bg),
            text: get("normal", d.text),
        }
    }
}

fn parse_hex_color(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color32::from_rgb(r, g, b))
}

/// 绘制通道：装了 .wsz 就切精灵图，没装就按内建经典配色用几何图形画。
/// 这样「经典界面」不依赖用户先导入皮肤。
pub struct Chrome<'a> {
    pub skin: Option<&'a Skin>,
    pub accent: [u8; 3],
}

impl<'a> Chrome<'a> {
    pub fn new(skin: Option<&'a Skin>, accent: [u8; 3]) -> Self {
        Chrome { skin, accent }
    }

    pub fn has(&self, sheet: &str) -> bool {
        self.skin.map(|s| s.has(sheet)).unwrap_or(false)
    }

    pub fn skin(&self) -> Option<&'a Skin> {
        self.skin
    }

    /// 内建配色的播放列表色板（没皮肤时用橙色系）
    pub fn pledit(&self) -> Pledit {
        match self.skin {
            Some(s) => s.pledit.clone(),
            None => Pledit {
                normal: Color32::from_rgb(255, 170, 80),
                current: Color32::from_rgb(255, 235, 190),
                normal_bg: Color32::from_rgb(26, 14, 8),
                selected_bg: Color32::from_rgb(96, 44, 12),
                text: Color32::from_rgb(255, 170, 80),
            },
        }
    }

    fn panel(&self, k: f32) -> Color32 {
        let c = self.accent;
        Color32::from_rgb(
            ((c[0] as f32) * k) as u8,
            ((c[1] as f32) * k) as u8,
            ((c[2] as f32) * k) as u8,
        )
    }

    /// 按源矩形切片；没皮肤时按图块语义画几何形状。
    #[allow(clippy::too_many_arguments)]
    pub fn sprite(
        &self,
        painter: &Painter,
        sheet: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        dst: Rect,
        tint: Color32,
    ) {
        if let Some(skin) = self.skin {
            skin.sprite(painter, sheet, x, y, w, h, dst, tint);
            return;
        }
        match sheet {
            "main.bmp" => {
                painter.rect_filled(dst, 0.0, self.panel(0.10));
                // 顶部标题条
                let top = Rect::from_min_size(dst.min, egui::vec2(dst.width(), 14.0 / 116.0 * dst.height()));
                painter.rect_filled(top, 0.0, self.panel(0.55));
                // 显示区/分析器底
                let well = |rx: f32, ry: f32, rw: f32, rh: f32, k: f32| {
                    let r = Rect::from_min_size(
                        dst.min + egui::vec2(rx / 275.0 * dst.width(), ry / 116.0 * dst.height()),
                        egui::vec2(rw / 275.0 * dst.width(), rh / 116.0 * dst.height()),
                    );
                    painter.rect_filled(r, 0.0, self.panel(k));
                };
                well(24.0, 43.0, 76.0, 36.0, 0.06);
                well(39.0, 25.0, 59.0, 14.0, 0.06);
                well(111.0, 23.0, 154.0, 7.0, 0.06);
                well(111.0, 42.0, 66.0, 8.0, 0.06);
                // 进度条 / 音量槽
                well(16.0, 72.0, 248.0, 10.0, 0.05);
                well(107.0, 57.0, 68.0, 13.0, 0.05);
                well(177.0, 57.0, 38.0, 13.0, 0.05);
            }
            "titlebar.bmp" => {
                painter.rect_filled(dst, 0.0, self.panel(if y == 0.0 { 0.7 } else { 0.55 }));
            }
            "cbuttons.bmp" => {
                let face = self.panel(if y >= 18.0 { 0.62 } else { 0.40 });
                painter.rect_filled(dst.shrink(0.5), 1.0, face);
                let c = self.panel(1.0);
                let stroke = egui::Stroke::new(1.5, c);
                let cx = dst.center();
                let s = dst.height() * 0.32;
                match x as i32 {
                    0 => { // 上一首
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(cx.x - s, cx.y),
                                egui::pos2(cx.x, cx.y - s),
                                egui::pos2(cx.x, cx.y + s),
                            ],
                            c, stroke,
                        ));
                        painter.rect_filled(
                            Rect::from_min_size(egui::pos2(cx.x + 1.0, cx.y - s), egui::vec2(s * 0.35, s * 2.0)),
                            0.0,
                            c,
                        );
                    }
                    23 => { // 播放
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(cx.x - s * 0.8, cx.y - s),
                                egui::pos2(cx.x + s * 0.9, cx.y),
                                egui::pos2(cx.x - s * 0.8, cx.y + s),
                            ],
                            c, stroke,
                        ));
                    }
                    46 => { // 暂停
                        for dx in [-s * 0.5, s * 0.5] {
                            painter.rect_filled(
                                Rect::from_center_size(cx + egui::vec2(dx, 0.0), egui::vec2(s * 0.5, s * 2.0)),
                                0.0,
                                c,
                            );
                        }
                    }
                    69 => { // 停止
                        painter.rect_filled(Rect::from_center_size(cx, egui::vec2(s * 1.7, s * 1.7)), 0.0, c);
                    }
                    92 => { // 下一首
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(cx.x + s, cx.y),
                                egui::pos2(cx.x, cx.y - s),
                                egui::pos2(cx.x, cx.y + s),
                            ],
                            c, stroke,
                        ));
                        painter.rect_filled(
                            Rect::from_min_size(egui::pos2(cx.x - 1.0 - s * 0.35, cx.y - s), egui::vec2(s * 0.35, s * 2.0)),
                            0.0,
                            c,
                        );
                    }
                    _ => { // eject：一个朝上的三角 + 下面一条
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(cx.x, cx.y - s),
                                egui::pos2(cx.x - s, cx.y + s * 0.1),
                                egui::pos2(cx.x + s, cx.y + s * 0.1),
                            ],
                            c, stroke,
                        ));
                        painter.rect_filled(
                            Rect::from_center_size(cx + egui::vec2(0.0, s * 0.55), egui::vec2(s * 2.0, s * 0.3)),
                            0.0,
                            c,
                        );
                    }
                }
            }
            "playpaus.bmp" => {
                let c = match x as i32 {
                    0 => Color32::from_rgb(120, 230, 120),
                    9 => Color32::from_rgb(240, 200, 90),
                    _ => Color32::from_rgb(150, 150, 150),
                };
                painter.circle_filled(dst.center(), dst.height() * 0.32, c);
            }
            "posbar.bmp" => {
                if x >= 248.0 {
                    painter.rect_filled(dst, 0.0, self.panel(0.85));
                } else {
                    painter.rect_filled(dst, 0.0, self.panel(0.12));
                }
            }
            "volume.bmp" => {
                if y >= 400.0 {
                    painter.rect_filled(dst.shrink(0.5), 1.0, self.panel(0.95));
                } else {
                    painter.rect_filled(dst, 0.0, self.panel(0.12));
                    // 已填充的部分按帧号长度示意
                    let frac = 1.0 - (y / 15.0) / 27.0;
                    let fill = Rect::from_min_size(dst.min, egui::vec2(dst.width() * frac, dst.height()));
                    painter.rect_filled(fill, 0.0, self.panel(0.5).gamma_multiply(0.8));
                }
            }
            "shufrep.bmp" => {
                let on = y >= 30.0;
                painter.rect_filled(
                    dst.shrink(0.5),
                    1.0,
                    if on { self.panel(0.75) } else { self.panel(0.32) },
                );
            }
            "eqmain.bmp" => {
                if w <= 12.0 && h <= 12.0 {
                    painter.rect_filled(dst, 1.0, self.panel(0.95));
                } else {
                    painter.rect_filled(dst, 0.0, self.panel(0.14));
                }
            }
            "pledit.bmp" => {
                let k = if y >= 72.0 { 0.30 } else if y >= 42.0 { 0.22 } else { 0.42 };
                painter.rect_filled(dst, 0.0, self.panel(k));
            }
            _ => {
                painter.rect_filled(dst, 0.0, self.panel(0.3));
            }
        }
    }

    /// 文字：有皮肤用 text.bmp 字模，没皮肤用系统字体
    pub fn text(
        &self,
        painter: &Painter,
        s: &str,
        origin: Pos2,
        scale: f32,
        max_x: f32,
        tint: Color32,
    ) -> f32 {
        if let Some(skin) = self.skin {
            return skin.text(painter, s, origin, scale, max_x, tint);
        }
        let font = egui::FontId::new(6.5 * scale.max(0.8), egui::FontFamily::Proportional);
        let galley = painter.layout_no_wrap(s.to_string(), font, tint);
        let w = galley.size().x.min(max_x - origin.x);
        painter.galley(origin, galley, tint);
        origin.x + w
    }

    /// 数字：有皮肤用 numbers.bmp，没皮肤用等宽字体
    pub fn digits(&self, painter: &Painter, s: &str, origin: Pos2, scale: f32, tint: Color32) {
        if let Some(skin) = self.skin {
            skin.digits(painter, s, origin, scale, tint);
            return;
        }
        let font = egui::FontId::new(12.5 * scale.max(0.8), egui::FontFamily::Monospace);
        painter.text(origin, egui::Align2::LEFT_TOP, s, font, tint);
    }
}

/// 皮肤目录：`<data_dir>/skins/`，当前生效的是 `current.wsz`。
pub fn skins_dir() -> std::path::PathBuf {
    audcore::config::data_dir().join("skins")
}

pub fn current_skin_path() -> std::path::PathBuf {
    skins_dir().join("current.wsz")
}

#[cfg(test)]
mod tests {
    use super::*;

    // 手工验证用：把 /tmp 下的真实皮肤丢进来跑一遍，确认解包 / 解码没踩坑
    #[test]
    fn loads_local_wsz_if_present() {
        let ctx = egui::Context::default();
        for id in [
            "8a2cfd657fa97b1a511b372f5249d63a",
            "9355a7476613879cfd6cd7f711494108",
            "aa0b1aae867bc4bac437b60c9d78c200",
        ] {
            let path = std::path::PathBuf::from(format!("/tmp/{id}.wsz"));
            if !path.exists() {
                continue;
            }
            let skin = Skin::load_wsz(&ctx, &path).expect("皮肤应当能加载");
            assert!(skin.has("main.bmp"), "{id} 缺 main.bmp");
            assert!(skin.has("cbuttons.bmp"), "{id} 缺 cbuttons.bmp");
            assert_eq!(skin.size("main.bmp"), Some((275, 116)));
        }
    }

    #[test]
    fn parses_pledit_colors() {
        let p = Pledit::parse("[Text]\nNormal=#00FF00\nCurrent=#FF0000\nNormalBG=#000000\nSelectedBG=#000080\n");
        assert_eq!(p.normal, Color32::from_rgb(0, 255, 0));
        assert_eq!(p.current, Color32::from_rgb(255, 0, 0));
    }
}
