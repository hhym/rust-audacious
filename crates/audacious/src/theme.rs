//! 主题、字体与排版工具。
//!
//! egui 自带字体没有中文字形，启动时从系统里找一份 CJK 字体装上；
//! 取不到时退回英文标签也不至于显示方块（由 `has_cjk` 标记控制）。

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};
use std::path::PathBuf;
use std::sync::Arc;

pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub panel_alt: Color32,
    pub field: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub text: Color32,
    pub dim: Color32,
    /// 是否深色系（决定 base visuals 与表格遮罩）
    pub dark: bool,
}

fn shade(c: Color32, factor: f32) -> Color32 {
    let f = |v: u8| ((v as f32) * factor).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

fn tint(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

/// 从封面里挑一个「能当强调色」的颜色：按 4bit 量化分桶，偏向鲜艳、避开近白近黑的像素。
pub fn accent_from_image(bytes: &[u8], dark_theme: bool) -> Option<[u8; 3]> {
    use std::collections::HashMap;
    let img = image::load_from_memory(bytes).ok()?.thumbnail(48, 48).to_rgba8();
    let mut buckets: HashMap<u8, (u32, u32, u32, u32)> = HashMap::new();
    for px in img.pixels() {
        let [r, g, b, a] = px.0;
        if a < 32 {
            continue;
        }
        let max = r.max(g).max(b) as f32;
        let min = r.min(g).min(b) as f32;
        let sat = if max <= 0.0 { 0.0 } else { (max - min) / max };
        let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        // 太灰、太白、太黑的像素不参与，不然挑出来是灰白
        if sat < 0.18 || lum < 24.0 || lum > 236.0 {
            continue;
        }
        let key = (((r as u16 >> 4) << 8) | ((g as u16 >> 4) << 4) | (b as u16 >> 4)) as u8;
        let e = buckets.entry(key).or_insert((0, 0, 0, 0));
        e.0 += 1;
        e.1 += r as u32;
        e.2 += g as u32;
        e.3 += b as u32;
    }
    let best = buckets
        .values()
        .max_by_key(|(count, ..)| *count)
        .copied()?;
    let n = best.0.max(1);
    let (r, g, b) = ((best.1 / n) as u8, (best.2 / n) as u8, (best.3 / n) as u8);
    let pick = vividify([r, g, b], dark_theme);
    let c = Color32::from_rgb(pick[0], pick[1], pick[2]);
    if (c.r() as f32 + c.g() as f32 + c.b() as f32) < 120.0 {
        return None;
    }
    Some(pick)
}

/// 把封面挑出来的颜色调成「能当强调色」的版本：先把饱和度拉到 0.55 以上（避免粉彩感），
/// 再按深浅主题调亮度——深色主题提亮到看得清，浅色主题压暗一点保对比度。
fn vividify(c: [u8; 3], dark_theme: bool) -> [u8; 3] {
    let (r, g, b) = (c[0] as f32, c[1] as f32, c[2] as f32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let sat = if max <= 0.0 { 0.0 } else { (max - min) / max };
    let target_sat = if dark_theme { 0.62 } else { 0.58 };
    let k = if sat > 0.02 {
        (target_sat / sat).clamp(1.0, 4.0)
    } else {
        1.0
    };
    let boost = |v: f32| (min + (v - min) * k).clamp(0.0, 255.0);
    let (mut r, mut g, mut b) = (boost(r), boost(g), boost(b));
    let max = r.max(g).max(b);
    if dark_theme {
        // 深底上的强调色要够亮才看得清
        let target = 205.0_f32;
        if max < target {
            let f = (target / max.max(1.0)).min(2.6);
            r *= f;
            g *= f;
            b *= f;
        }
    } else if max > 185.0 {
        let f = 185.0 / max;
        r *= f;
        g *= f;
        b *= f;
    }
    [r as u8, g as u8, b as u8]
}

/// 给菜单里的小色块用：只关心主题底色。
pub fn palette_for(theme: audcore::Theme, accent: [u8; 3]) -> Palette {
    let mut cfg = audcore::Config::default();
    cfg.theme = theme;
    cfg.accent = accent;
    palette(&cfg)
}

/// 按主题与强调色生成整套配色。
pub fn palette(cfg: &audcore::Config) -> Palette {
    let raw = Color32::from_rgb(cfg.accent[0], cfg.accent[1], cfg.accent[2]);

    // 墨水屏：只保留纯黑/白/浅灰，避免彩色反光率低、灰阶糊成一团
    if cfg.eink {
        return Palette {
            bg: Color32::from_rgb(255, 255, 255),
            panel: Color32::from_rgb(255, 255, 255),
            panel_alt: Color32::from_rgb(232, 232, 232),
            field: Color32::from_rgb(255, 255, 255),
            accent: Color32::from_rgb(0, 0, 0),
            accent_soft: Color32::from_rgb(214, 214, 214),
            text: Color32::from_rgb(0, 0, 0),
            dim: Color32::from_rgb(90, 90, 90),
            dark: false,
        };
    }

    match cfg.theme {
        audcore::Theme::Dark => Palette {
            bg: Color32::from_rgb(23, 24, 27),
            panel: Color32::from_rgb(31, 33, 37),
            panel_alt: Color32::from_rgb(38, 40, 45),
            field: Color32::from_rgb(18, 19, 22),
            accent: raw,
            accent_soft: tint(raw, 46),
            text: Color32::from_rgb(226, 228, 232),
            dim: Color32::from_rgb(138, 142, 150),
            dark: true,
        },
        audcore::Theme::Light => Palette {
            bg: Color32::from_rgb(245, 246, 248),
            panel: Color32::from_rgb(236, 238, 241),
            panel_alt: Color32::from_rgb(227, 230, 234),
            field: Color32::from_rgb(255, 255, 255),
            accent: shade(raw, 0.78),
            accent_soft: tint(raw, 42),
            text: Color32::from_rgb(32, 35, 40),
            dim: Color32::from_rgb(106, 112, 121),
            dark: false,
        },
        // 深青：墨青底 + 青绿强调，字色偏冷白
        audcore::Theme::Teal => Palette {
            bg: Color32::from_rgb(13, 27, 29),
            panel: Color32::from_rgb(18, 36, 39),
            panel_alt: Color32::from_rgb(24, 45, 49),
            field: Color32::from_rgb(9, 20, 22),
            accent: Color32::from_rgb(45, 189, 175),
            accent_soft: tint(Color32::from_rgb(45, 189, 175), 52),
            text: Color32::from_rgb(222, 238, 236),
            dim: Color32::from_rgb(126, 158, 156),
            dark: true,
        },
        // 护眼绿：豆沙绿纸底，长时间盯歌词不刺眼
        audcore::Theme::EyeGreen => Palette {
            bg: Color32::from_rgb(199, 237, 204),
            panel: Color32::from_rgb(186, 228, 192),
            panel_alt: Color32::from_rgb(171, 218, 179),
            field: Color32::from_rgb(226, 245, 228),
            accent: Color32::from_rgb(38, 122, 76),
            accent_soft: tint(Color32::from_rgb(38, 122, 76), 62),
            text: Color32::from_rgb(24, 46, 30),
            dim: Color32::from_rgb(78, 112, 88),
            dark: false,
        },
        // 天空蓝：浅蓝底 + 天蓝强调
        audcore::Theme::SkyBlue => Palette {
            bg: Color32::from_rgb(226, 240, 253),
            panel: Color32::from_rgb(212, 231, 250),
            panel_alt: Color32::from_rgb(197, 220, 245),
            field: Color32::from_rgb(247, 251, 255),
            accent: Color32::from_rgb(28, 116, 205),
            accent_soft: tint(Color32::from_rgb(28, 116, 205), 62),
            text: Color32::from_rgb(20, 38, 58),
            dim: Color32::from_rgb(83, 110, 138),
            dark: false,
        },
        // 绚丽紫：深紫底 + 亮紫强调
        audcore::Theme::Purple => Palette {
            bg: Color32::from_rgb(26, 18, 38),
            panel: Color32::from_rgb(36, 25, 52),
            panel_alt: Color32::from_rgb(47, 33, 68),
            field: Color32::from_rgb(19, 13, 28),
            accent: Color32::from_rgb(178, 108, 255),
            accent_soft: tint(Color32::from_rgb(178, 108, 255), 52),
            text: Color32::from_rgb(236, 228, 248),
            dim: Color32::from_rgb(158, 142, 182),
            dark: true,
        },
        // 暗夜金：近黑底 + 香槟金强调
        audcore::Theme::NightGold => Palette {
            bg: Color32::from_rgb(20, 19, 17),
            panel: Color32::from_rgb(31, 29, 26),
            panel_alt: Color32::from_rgb(42, 39, 34),
            field: Color32::from_rgb(15, 14, 12),
            accent: Color32::from_rgb(212, 168, 88),
            accent_soft: tint(Color32::from_rgb(212, 168, 88), 52),
            text: Color32::from_rgb(238, 232, 221),
            dim: Color32::from_rgb(160, 151, 134),
            dark: true,
        },
        // 樱粉：淡樱粉纸底 + 玫粉强调
        audcore::Theme::Sakura => Palette {
            bg: Color32::from_rgb(253, 240, 244),
            panel: Color32::from_rgb(250, 226, 233),
            panel_alt: Color32::from_rgb(246, 214, 223),
            field: Color32::from_rgb(255, 249, 251),
            accent: Color32::from_rgb(206, 82, 122),
            accent_soft: tint(Color32::from_rgb(206, 82, 122), 52),
            text: Color32::from_rgb(58, 30, 40),
            dim: Color32::from_rgb(136, 96, 110),
            dark: false,
        },
        // 橘黄：暖米底 + 橙色面板，强调色压暗一点保证对比度
        audcore::Theme::Amber => Palette {
            bg: Color32::from_rgb(253, 242, 227),
            panel: Color32::from_rgb(247, 224, 197),
            panel_alt: Color32::from_rgb(240, 210, 176),
            field: Color32::from_rgb(255, 251, 244),
            accent: shade(raw, 0.80),
            accent_soft: tint(raw, 64),
            text: Color32::from_rgb(59, 40, 20),
            dim: Color32::from_rgb(133, 98, 62),
            dark: false,
        },
    }
}

fn cjk_candidates() -> Vec<PathBuf> {
    let mut list = Vec::new();
    if cfg!(windows) {
        if let Ok(win_dir) = std::env::var("WINDIR") {
            let fonts = PathBuf::from(win_dir).join("Fonts");
            for name in ["msyh.ttc", "msyh.ttf", "msyhbd.ttc", "simhei.ttf", "simsun.ttc"] {
                list.push(fonts.join(name));
            }
        }
    } else if cfg!(target_os = "android") {
        // Android 系统的 CJK 字体就在这几个固定位置
        for p in [
            "/system/fonts/NotoSansCJK-Regular.ttc",
            "/system/fonts/NotoSansCJKsc-Regular.otf",
            "/system/fonts/DroidSansFallbackFull.ttf",
            "/system/fonts/DroidSansFallback.ttf",
        ] {
            list.push(PathBuf::from(p));
        }
    } else if cfg!(target_os = "macos") {
        for p in [
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
            "/Library/Fonts/Arial Unicode.ttf",
        ] {
            list.push(PathBuf::from(p));
        }
    } else {
        for p in [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/truetype/arphic/uming.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ] {
            list.push(PathBuf::from(p));
        }
    }
    list
}

/// 安装系统 CJK 字体；返回是否成功（失败时 UI 改走英文标签）。
pub fn install_fonts(ctx: &egui::Context) -> bool {
    let found = cjk_candidates()
        .into_iter()
        .find(|p| p.exists())
        .and_then(|p| std::fs::read(p).ok());

    let Some(bytes) = found else {
        log::warn!("未找到系统中文字体，界面使用英文标签");
        return false;
    };

    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("system-cjk".into(), Arc::new(FontData::from_owned(bytes)));
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "system-cjk".into());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push("system-cjk".into());
    ctx.set_fonts(fonts);
    true
}

/// 应用配色与控件样式。
pub fn apply_style(ctx: &egui::Context, cfg: &audcore::Config) {
    let p = palette(cfg);
    let mut style = (*ctx.style()).clone();

    let mut visuals = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.override_text_color = Some(p.text);
    visuals.panel_fill = p.panel;
    visuals.window_fill = p.panel;
    visuals.extreme_bg_color = p.field;
    visuals.faint_bg_color = p.panel_alt;
    visuals.hyperlink_color = p.accent;
    visuals.selection.bg_fill = p.accent_soft;
    visuals.selection.stroke = Stroke::new(1.0_f32, p.accent);
    visuals.widgets.noninteractive.bg_fill = p.panel;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, shade(p.panel, 1.18));
    visuals.widgets.inactive.bg_fill = p.panel_alt;
    visuals.widgets.inactive.weak_bg_fill = p.panel_alt;
    visuals.widgets.hovered.bg_fill = shade(p.accent, 0.55);
    visuals.widgets.active.bg_fill = p.accent;
    if cfg.eink {
        // 墨水屏：直角、无悬停变色（避免局部刷新闪一下）、字号整体放大
        let r = egui::CornerRadius::ZERO;
        visuals.widgets.noninteractive.corner_radius = r;
        visuals.widgets.inactive.corner_radius = r;
        visuals.widgets.hovered.corner_radius = r;
        visuals.widgets.active.corner_radius = r;
        visuals.widgets.open.corner_radius = r;
        visuals.window_corner_radius = r;
        visuals.menu_corner_radius = r;
        visuals.widgets.hovered.bg_fill = p.panel_alt;
        visuals.widgets.hovered.weak_bg_fill = p.panel_alt;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(0, 0, 0));
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(120, 120, 120));
        visuals.widgets.active.bg_stroke = Stroke::new(2.0_f32, Color32::from_rgb(0, 0, 0));
        style.visuals = visuals;
        style.spacing.item_spacing = egui::vec2(10.0, 9.0);
        style.spacing.button_padding = egui::vec2(12.0, 8.0);
        style.spacing.interact_size.y = 30.0;
        style.text_styles.insert(TextStyle::Body, FontId::new(16.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(16.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Small, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Heading, FontId::new(19.0, FontFamily::Proportional));
        fonts_monospace_hack(&mut style);
        ctx.set_style(style);
        return;
    }

    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.text_styles.insert(
        TextStyle::Body,
        FontId::new(13.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(13.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.5, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(16.0, FontFamily::Proportional),
    );
    // 数字与时间用等宽，表格对齐更稳
    fonts_monospace_hack(&mut style);

    ctx.set_style(style);
}

fn fonts_monospace_hack(style: &mut egui::Style) {
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(12.5, FontFamily::Monospace),
    );
}

/// 按可用宽度估算截断，避免长标题撑破列。
pub fn fit_text(text: &str, width: f32, font_size: f32) -> String {
    let approx_char = font_size * 0.68;
    let max_chars = (width / approx_char).max(1.0) as usize;
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// 省略号形式的时长。
pub fn time_text(seconds: f64) -> String {
    audcore::config::format_time(seconds)
}

#[cfg(test)]
mod cover_accent_tests {
    use super::*;

    fn solid(r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut img = image::RgbaImage::new(40, 40);
        for px in img.pixels_mut() {
            px.0 = [r, g, b, 255];
        }
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn picks_the_colorful_pixel() {
        let png = solid(220, 40, 60);
        let pick = accent_from_image(&png, false).expect("纯红封面应该挑得出颜色");
        assert!(pick[0] > pick[1] && pick[0] > pick[2], "挑出来偏红：{pick:?}");
    }

    #[test]
    fn gray_cover_has_no_accent() {
        let png = solid(128, 128, 128);
        assert!(accent_from_image(&png, false).is_none(), "灰封面不该挑出颜色");
    }
}
