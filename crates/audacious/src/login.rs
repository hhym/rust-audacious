//! 扫码登录窗口。
//!
//! MyGo 插件如果导出 `loginQr` / `loginPoll`，这里就用宿主线程去拉二维码、轮询状态：
//! `loginQr({})` → `{ key, qrImage?, qrUrl?, tip? }`
//! `loginPoll({ key })` → `{ status: waiting|scanned|done|expired|failed, message? }`
//!
//! 登录态本身留在插件手里（`host.storage` / `host.setting`），宿主只负责显示与轮询。

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use audplugins::MygoPlugin;
use base64::Engine;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// 正在向插件要二维码
    Loading,
    /// 二维码已显示，等待扫码
    Waiting,
    /// 已扫码，等待手机端确认
    Scanned,
    Done,
    Failed(String),
    Expired,
}

pub enum Event {
    Qr(Result<QrData, String>),
    Poll(Result<(String, String), String>),
}

pub struct QrData {
    pub key: String,
    pub tip: String,
    pub link: String,
    pub image: Option<Vec<u8>>,
}

pub struct LoginState {
    pub open: bool,
    pub plugin_name: String,
    plugin: Option<MygoPlugin>,
    key: String,
    tip: String,
    pub link: String,
    pub status: Status,
    pub message: String,
    texture: Option<egui::TextureHandle>,
    rx: Option<Receiver<Event>>,
    next_poll: Option<Instant>,
    image_bytes: Option<Vec<u8>>,
}

impl Default for LoginState {
    fn default() -> Self {
        Self {
            open: false,
            plugin_name: String::new(),
            plugin: None,
            key: String::new(),
            tip: String::new(),
            link: String::new(),
            status: Status::Loading,
            message: String::new(),
            texture: None,
            rx: None,
            next_poll: None,
            image_bytes: None,
        }
    }
}

impl LoginState {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 打开窗口并立刻向插件要一张新二维码。
    pub fn open(&mut self, plugin: MygoPlugin) {
        self.open = true;
        self.plugin_name = plugin.display_name();
        self.plugin = Some(plugin);
        self.reset_qr();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.plugin = None;
        self.texture = None;
        self.image_bytes = None;
        self.key.clear();
        self.tip.clear();
        self.link.clear();
        self.message.clear();
        self.status = Status::Loading;
        self.rx = None;
        self.next_poll = None;
    }

    /// 重新拉一张二维码（原二维码过期时用）。
    pub fn refresh(&mut self) {
        self.reset_qr();
    }

    fn reset_qr(&mut self) {
        let Some(plugin) = self.plugin.clone() else {
            self.status = Status::Failed("插件已卸载".into());
            return;
        };
        self.status = Status::Loading;
        self.message.clear();
        self.key.clear();
        self.tip.clear();
        self.link.clear();
        self.texture = None;
        self.image_bytes = None;
        self.next_poll = None;

        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let result = plugin.login_qr().map_err(|e| e.to_string()).and_then(parse_qr);
            let _ = tx.send(Event::Qr(result));
        });
    }

    /// 每帧调用：收结果、按节奏安排下一次轮询。
    pub fn tick(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }

        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(Event::Qr(Ok(qr))) => {
                    self.rx = None;
                    self.key = qr.key;
                    self.tip = qr.tip;
                    self.link = qr.link;
                    self.message.clear();
                    self.status = Status::Waiting;
                    self.next_poll = Some(Instant::now() + Duration::from_secs(2));
                    if let Some(bytes) = qr.image {
                        self.image_bytes = Some(bytes.clone());
                        if let Some(tex) = decode_texture(ctx, &bytes) {
                            self.texture = Some(tex);
                        }
                    }
                }
                Ok(Event::Qr(Err(err))) => {
                    self.rx = None;
                    self.status = Status::Failed(err);
                }
                Ok(Event::Poll(Ok((status, message)))) => {
                    self.rx = None;
                    self.status = match status.as_str() {
                        "scanned" => Status::Scanned,
                        "done" => Status::Done,
                        "expired" => Status::Expired,
                        "failed" => Status::Failed(message.clone()),
                        _ => Status::Waiting,
                    };
                    if message.is_empty() {
                        self.message.clear();
                    } else {
                        self.message = message;
                    }
                    match self.status {
                        Status::Done => {}
                        Status::Waiting | Status::Scanned => {
                            self.next_poll = Some(Instant::now() + Duration::from_secs(2))
                        }
                        _ => self.next_poll = None,
                    }
                }
                Ok(Event::Poll(Err(err))) => {
                    self.rx = None;
                    self.status = Status::Failed(err);
                    self.next_poll = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.rx = None,
            }
        }

        if self.rx.is_none() && matches!(self.status, Status::Waiting | Status::Scanned) {
            if let (Some(plugin), Some(due)) = (self.plugin.clone(), self.next_poll) {
                if Instant::now() >= due && !self.key.is_empty() {
                    let key = self.key.clone();
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.rx = Some(rx);
                    self.next_poll = None;
                    std::thread::spawn(move || {
                        let result = plugin.login_poll(&key).map(|value| {
                            let status = value["status"].as_str().unwrap_or("waiting").to_string();
                            let message = value["message"].as_str().unwrap_or("").to_string();
                            (status, message)
                        });
                        let _ = tx.send(Event::Poll(result.map_err(|e| e.to_string())));
                    });
                }
            }
        }
    }

    /// 窗口本体。返回 `Some(提示语)` 时由调用方弹 toast。
    pub fn ui(&mut self, ctx: &egui::Context, p: &crate::theme::Palette, zh: bool) -> Option<String> {
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut win_open = true;
        let mut toast = None;
        let mut want_refresh = false;

        egui::Window::new(if zh {
            format!("扫码登录 · {}", self.plugin_name)
        } else {
            format!("Sign in · {}", self.plugin_name)
        })
        .open(&mut win_open)
        .default_width(340.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| match &self.status {
                Status::Loading => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(if zh {
                            "正在获取二维码…"
                        } else {
                            "Fetching QR code…"
                        });
                    });
                }
                Status::Waiting => {
                    ui.label(
                        egui::RichText::new(if zh {
                            "请用手机扫码"
                        } else {
                            "Scan with your phone"
                        })
                        .strong()
                        .color(p.accent),
                    );
                }
                Status::Scanned => {
                    ui.label(
                        egui::RichText::new(if zh {
                            "已扫码，请在手机上确认"
                        } else {
                            "Scanned, confirm on your phone"
                        })
                        .strong()
                        .color(p.accent),
                    );
                }
                Status::Done => {
                    ui.label(
                        egui::RichText::new(if zh { "登录成功" } else { "Signed in" })
                            .strong()
                            .color(p.accent),
                    );
                }
                Status::Expired => {
                    ui.label(
                        egui::RichText::new(if zh {
                            "二维码已过期，点“刷新二维码”"
                        } else {
                            "QR code expired, refresh it"
                        })
                        .color(egui::Color32::from_rgb(200, 120, 60)),
                    );
                }
                Status::Failed(err) => {
                    ui.label(
                        egui::RichText::new(err)
                            .small()
                            .color(egui::Color32::from_rgb(200, 80, 80)),
                    );
                }
            });

            ui.add_space(6.0);
            if let Some(tex) = &self.texture {
                let side = 220.0;
                ui.vertical_centered(|ui| {
                    ui.add(egui::Image::new((tex.id(), egui::vec2(side, side))));
                });
            } else if matches!(self.status, Status::Waiting | Status::Scanned) && self.link.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(if zh {
                            "插件没有返回二维码图片"
                        } else {
                            "Plugin returned no QR image"
                        })
                        .small()
                        .color(p.dim),
                    );
                });
            }

            if !self.tip.is_empty() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&self.tip).small().color(p.dim));
            }
            if !self.link.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(if zh { "链接" } else { "Link" }).small().color(p.dim));
                    ui.add(egui::Label::new(
                        egui::RichText::new(&self.link).small().monospace(),
                    ))
                    .on_hover_text(&self.link);
                });
            }
            if !self.message.is_empty() {
                ui.label(egui::RichText::new(&self.message).small().color(p.dim));
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(if zh { "刷新二维码" } else { "Refresh QR" }).clicked() {
                    want_refresh = true;
                }
                if self.status == Status::Done
                    && ui.button(if zh { "完成" } else { "Done" }).clicked()
                {
                    open = false;
                    toast = Some(if zh {
                        format!("{} 已登录", self.plugin_name)
                    } else {
                        format!("{} signed in", self.plugin_name)
                    });
                }
                if ui.button(if zh { "关闭" } else { "Close" }).clicked() {
                    open = false;
                }
            });

            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(if zh {
                    "登录信息由插件自己保存（host.storage / 设置项），宿主不碰账号密码。"
                } else {
                    "The plugin keeps the session itself via host.storage; the host never touches credentials."
                })
                .small()
                .color(p.dim),
            );
        });

        if want_refresh {
            self.refresh();
        }
        if !win_open {
            open = false;
        }
        if !open {
            self.close();
        }
        toast
    }
}

fn parse_qr(value: serde_json::Value) -> Result<QrData, String> {
    // 1x1 透明 PNG，用来验证 data URL 解码分支
    #[cfg(test)]
    const TINY_PNG: &str =
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    #[cfg(test)]
    let _ = TINY_PNG;
    let key = value["key"].as_str().unwrap_or("").to_string();
    let image_text = value["qrImage"]
        .as_str()
        .or_else(|| value["qr"].as_str())
        .or_else(|| value["image"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let link = value["qrUrl"]
        .as_str()
        .or_else(|| value["url"].as_str())
        .unwrap_or("")
        .to_string();
    let tip = value["tip"].as_str().unwrap_or("").to_string();

    let image = if image_text.is_empty() {
        None
    } else if image_text.starts_with("data:") || image_text.len() > 800 {
        let payload = match image_text.find("base64,") {
            Some(idx) => &image_text[idx + 7..],
            None => image_text.as_str(),
        };
        match base64::engine::general_purpose::STANDARD.decode(
            payload.trim().replace(['\n', '\r', ' '], ""),
        ) {
            Ok(bytes) => Some(bytes),
            Err(e) => return Err(format!("二维码图片不是合法 base64: {e}")),
        }
    } else if image_text.starts_with("http") {
        Some(audplugins::fetch_bytes(&image_text).map_err(|e| e.to_string())?)
    } else {
        None
    };

    if key.is_empty() && image.is_none() && link.is_empty() {
        return Err("插件返回的二维码数据是空的（需要 key / qrImage / qrUrl 至少一项）".into());
    }
    Ok(QrData {
        key,
        tip,
        link,
        image,
    })
}

fn decode_texture(ctx: &egui::Context, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let pixels = img.into_raw();
    let color = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
    Some(ctx.load_texture("login-qr", color, egui::TextureOptions::LINEAR))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_data_url_is_decoded() {
        let value = serde_json::json!({
            "key": "k",
            "qrImage": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
            "tip": "扫码"
        });
        let qr = parse_qr(value).unwrap();
        assert_eq!(qr.key, "k");
        assert_eq!(qr.tip, "扫码");
        assert!(qr.image.unwrap().len() > 20);
    }

    #[test]
    fn empty_qr_is_rejected() {
        assert!(parse_qr(serde_json::json!({})).is_err());
    }
}
