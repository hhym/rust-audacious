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
