//! 本地控制服务：给 `audtool` 用的 IPC 服务端。
//!
//! 监听 127.0.0.1 随机端口，端口号写入 `control.port`，
//! 与 Audacious 里 audtool ↔ GUI 的关系一致。

#[cfg(not(target_os = "android"))]
use std::io::{BufRead, BufReader, Write};
#[cfg(not(target_os = "android"))]
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

use audcore::ipc::{Response, Request};
#[cfg(not(target_os = "android"))]
use audcore::ipc::{DEFAULT_PORT, port_file};
use audcore::plugin;
use serde_json::json;

use crate::engine::{Cmd, Shared};

/// 启动控制服务，返回实际监听端口。
///
/// Android 上没有 `audtool`，也不该占一个本地端口，直接返回 0。
pub fn serve(shared: Shared) -> anyhow::Result<u16> {
    #[cfg(target_os = "android")]
    {
        let _ = shared;
        return Ok(0);
    }

    #[cfg(not(target_os = "android"))]
    {
        serve_desktop(shared)
    }
}

#[cfg(not(target_os = "android"))]
fn serve_desktop(shared: Shared) -> anyhow::Result<u16> {
    let listener = match TcpListener::bind(("127.0.0.1", DEFAULT_PORT)) {
        Ok(l) => l,
        Err(_) => TcpListener::bind(("127.0.0.1", 0))?,
    };
    let port = listener.local_addr()?.port();

    let path = port_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, port.to_string())?;
    log::info!("控制端口 {port}（audtool 可连接）");

    let shared_for_thread = shared.clone();
    std::thread::Builder::new()
        .name("audacious-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let shared = shared_for_thread.clone();
                std::thread::spawn(move || {
                    if let Err(err) = handle_client(stream, shared) {
                        log::debug!("控制连接结束: {err}");
                    }
                });
            }
        })?;

    Ok(port)
}

#[cfg(not(target_os = "android"))]
fn handle_client(mut stream: TcpStream, shared: Shared) -> anyhow::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim().is_empty() {
        return Ok(());
    }
    let response = match serde_json::from_str::<Request>(line.trim()) {
        Ok(req) => dispatch(&req, &shared),
        Err(err) => Response::err(format!("请求格式错误: {err}")),
    };
    stream.write_all(serde_json::to_string(&response)?.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

/// 处理一条命令。
pub fn dispatch(req: &Request, shared: &Shared) -> Response {
    let state = shared.state();
    let mut pl = match shared.playlist.lock() {
        Ok(p) => p,
        Err(_) => return Response::err("播放列表被占用"),
    };

    let cmd = req.cmd.to_lowercase();
    match cmd.as_str() {
        "ping" | "version" => Response::ok(json!(audcore::VERSION)),
        "status" | "playback-status" => Response::ok(json!(state.status.as_str())),
        "current-song" => Response::ok(json!(if state.title.is_empty() {
            String::new()
        } else {
            state.title.clone()
        })),
        "current-info" => Response::ok(json!({
            "title": state.title,
            "artist": state.artist,
            "album": state.album,
            "format": state.format,
            "length": state.length,
            "position": state.position,
            "index": state.current,
            "status": state.status.as_str(),
        })),
        "play-pause" | "toggle" => {
            shared.send(Cmd::Toggle);
            Response::empty()
        }
        "play" => {
            if let Some(i) = req.arg_u64(0) {
                shared.send(Cmd::PlayIndex(i as usize));
                Response::empty()
            } else {
                shared.send(Cmd::Play);
                Response::empty()
            }
        }
        "pause" => {
            shared.send(Cmd::Pause);
            Response::empty()
        }
        "stop" => {
            shared.send(Cmd::Stop);
            Response::empty()
        }
        "next" => {
            shared.send(Cmd::Next);
            Response::empty()
        }
        "prev" | "previous" => {
            shared.send(Cmd::Prev);
            Response::empty()
        }
        "seek" => match req.arg_f64(0) {
            Some(pos) => {
                shared.send(Cmd::Seek(pos));
                Response::empty()
            }
            None => Response::err("seek 需要一个秒数参数"),
        },
        "seek-relative" => match req.arg_f64(0) {
            Some(delta) => {
                shared.send(Cmd::SeekRelative(delta));
                Response::empty()
            }
            None => Response::err("seek-relative 需要一个秒数参数"),
        },
        "volume" => match req.arg_f64(0) {
            Some(percent) => {
                let volume = (percent / 100.0).clamp(0.0, 1.0) as f32;
                shared.send(Cmd::SetVolume(volume));
                if let Ok(mut cfg) = shared.config.lock() {
                    cfg.volume = volume;
                    let _ = cfg.save();
                }
                Response::ok(json!(percent))
            }
            None => Response::ok(json!((state.volume * 100.0).round())),
        },
        "get-volume" => Response::ok(json!((state.volume * 100.0).round())),
        "playlist-count" => Response::ok(json!(pl.len())),
        "playlist-total-length" => Response::ok(json!(pl.total_duration())),
        "playlist-song" => match req.arg_u64(0) {
            Some(one_based) => {
                let idx = one_based.saturating_sub(1) as usize;
                match pl.get(idx) {
                    Some(e) => Response::ok(json!({
                        "index": one_based,
                        "title": e.display_title(),
                        "artist": e.display_artist(),
                        "album": e.display_album(),
                        "length": e.length,
                        "format": e.format,
                        "path": e.path,
                    })),
                    None => Response::err("没有这一行"),
                }
            }
            None => Response::err("playlist-song 需要行号（从 1 开始）"),
        },
        "playlist-play" => match req.arg_u64(0) {
            Some(one_based) => {
                shared.send(Cmd::PlayIndex(one_based.saturating_sub(1) as usize));
                Response::empty()
            }
            None => Response::err("playlist-play 需要行号（从 1 开始）"),
        },
        "playlist-next" => {
            shared.send(Cmd::Next);
            Response::empty()
        }
        "playlist-remove" => match req.arg_u64(0) {
            Some(one_based) => {
                let idx = one_based.saturating_sub(1) as usize;
                pl.remove(idx);
                shared.send(Cmd::Refresh);
                Response::empty()
            }
            None => Response::err("playlist-remove 需要行号"),
        },
        "playlist-clear" => {
            pl.clear();
            shared.send(Cmd::Stop);
            Response::empty()
        }
        "playlist-add" => {
            let mut added = 0usize;
            for i in 0..req.args.len() {
                let Some(path) = req.arg_str(i) else { continue };
                let path = PathBuf::from(path);
                if path.is_dir() {
                    added += add_folder(&mut pl, &path);
                } else if audcore::tags::is_audio_file(&path) {
                    pl.add_path(&path);
                    added += 1;
                } else if plugin::playlist_plugin_for(&path).is_some() {
                    if let Ok(paths) = plugin::load_playlist(&path) {
                        for p in paths {
                            if audcore::tags::is_audio_file(&p) {
                                pl.add_path(&p);
                                added += 1;
                            }
                        }
                    }
                }
            }
            Response::ok(json!(added))
        }
        "playlist-shuffle" => {
            let arg = req.arg_str(0).map(|s| s.to_lowercase());
            pl.shuffle = match arg.as_deref() {
                Some("on") | Some("1") | Some("true") => true,
                Some("off") | Some("0") | Some("false") => false,
                _ => !pl.shuffle,
            };
            let on = pl.shuffle;
            drop(pl);
            if let Ok(mut cfg) = shared.config.lock() {
                cfg.shuffle = on;
                let _ = cfg.save();
            }
            Response::ok(json!(on))
        }
        "playlist-repeat" => {
            let arg = req.arg_str(0).map(|s| s.to_lowercase());
            pl.repeat = match arg.as_deref() {
                Some("off") | Some("0") => audcore::RepeatMode::Off,
                Some("all") | Some("playlist") | Some("1") => audcore::RepeatMode::All,
                Some("track") | Some("song") | Some("one") => audcore::RepeatMode::Track,
                _ => pl.repeat.cycle(),
            };
            let mode = pl.repeat;
            drop(pl);
            if let Ok(mut cfg) = shared.config.lock() {
                cfg.repeat = mode;
                let _ = cfg.save();
            }
            Response::ok(json!(match mode {
                audcore::RepeatMode::Off => "off",
                audcore::RepeatMode::All => "all",
                audcore::RepeatMode::Track => "track",
            }))
        }
        "playlist-save" => match req.arg_str(0) {
            Some(path) => {
                let entries = pl.entries.clone();
                drop(pl);
                match plugin::save_playlist(std::path::Path::new(path), &entries) {
                    Ok(()) => Response::empty(),
                    Err(err) => Response::err(err.to_string()),
                }
            }
            None => Response::err("playlist-save 需要文件路径"),
        },
        "playlist-load" => match req.arg_str(0) {
            Some(path) => {
                let path = PathBuf::from(path);
                match plugin::load_playlist(&path) {
                    Ok(paths) => {
                        let count = paths.len();
                        for p in &paths {
                            if audcore::tags::is_audio_file(&p) {
                                pl.add_path(&p);
                            }
                        }
                        Response::ok(json!(count))
                    }
                    Err(err) => Response::err(err.to_string()),
                }
            }
            None => Response::err("playlist-load 需要文件路径"),
        },
        "eq-enable" => {
            let on = match req.arg_str(0).map(|s| s.to_lowercase()).as_deref() {
                Some("on") | Some("1") | Some("true") => true,
                Some("off") | Some("0") | Some("false") => false,
                _ => {
                    let current = shared.config.lock().map(|c| c.eq_enabled).unwrap_or(false);
                    !current
                }
            };
            drop(pl);
            shared.eq.set_enabled(on);
            if let Ok(mut cfg) = shared.config.lock() {
                cfg.eq_enabled = on;
                let _ = cfg.save();
            }
            Response::ok(json!(on))
        }
        "eq-gain" => {
            let Some(band) = req.arg_u64(0) else {
                return Response::err("eq-gain 需要频段序号");
            };
            let Some(gain) = req.arg_f64(1) else {
                return Response::err("eq-gain 需要增益值（dB）");
            };
            drop(pl);
            let gains = {
                let mut cfg = match shared.config.lock() {
                    Ok(c) => c,
                    Err(_) => return Response::err("配置被占用"),
                };
                let idx = band as usize;
                if idx >= cfg.eq_gains.len() {
                    return Response::err("频段超出范围");
                }
                cfg.eq_gains[idx] = (gain as f32).clamp(-24.0, 24.0);
                let gains = cfg.eq_gains.clone();
                let _ = cfg.save();
                gains
            };
            shared.eq.set_gains(&gains);
            Response::ok(json!(gains))
        }
        "quit" => {
            drop(pl);
            shared.shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
            shared.send(Cmd::Shutdown);
            Response::empty()
        }
        other => Response::err(format!("未知命令: {other}")),
    }
}

fn add_folder(pl: &mut audcore::Playlist, dir: &std::path::Path) -> usize {
    let mut count = 0;
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += add_folder(pl, &path);
        } else if audcore::tags::is_audio_file(&path) {
            pl.add_path(&path);
            count += 1;
        }
    }
    count
}

/// 供 UI 复用：添加路径（文件/目录/播放列表），返回新增数量。
pub fn add_paths(playlist: &mut audcore::Playlist, paths: &[PathBuf]) -> usize {
    let mut added = 0;
    for path in paths {
        if path.is_dir() {
            added += add_folder(playlist, path);
        } else if audcore::tags::is_audio_file(path) {
            playlist.add_path(path);
            added += 1;
        } else if plugin::playlist_plugin_for(path).is_some() {
            if let Ok(items) = plugin::load_playlist(path) {
                for p in items {
                    if audcore::tags::is_audio_file(&p) {
                        playlist.add_path(&p);
                        added += 1;
                    }
                }
            }
        }
    }
    added
}
