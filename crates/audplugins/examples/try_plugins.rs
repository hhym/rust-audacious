//! 临时工具：对若干插件目录逐个跑「加载 / 搜索 / 取流 / 歌词」，供人工核对。
//!
//! 用法：cargo run -p audplugins --example try_plugins -- /path/plugin_dir ...
use std::path::PathBuf;

use audplugins::{MediaSource, MygoPlugin, OnlinePlugin};

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if dirs.is_empty() {
        eprintln!("usage: try_plugins <plugin dir>...");
        return;
    }
    for dir in dirs {
        println!("================ {}", dir.display());
        let plugin: OnlinePlugin = OnlinePlugin::Mygo(MygoPlugin::load(&dir));
        println!("name    : {}", plugin.name());
        println!("version : {}", plugin.version());
        if let Some(err) = plugin.error() {
            println!("error   : {err}");
        }
        let mut methods = Vec::new();
        for m in [
            "search",
            "getMediaSource",
            "getLyric",
            "home",
            "loginQr",
            "loginPoll",
        ] {
            if plugin.supports(m) {
                methods.push(m);
            }
        }
        println!("methods : {}", methods.join(", "));

        let query = std::env::var("TRY_QUERY").unwrap_or_else(|_| "晴天".to_string());
        match plugin.search_tracks(&query, 1) {
            Ok(items) => {
                println!("search  : {} 条（query={query}）", items.len());
                for item in items.iter().take(3) {
                    let title = item
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let artist = item.get("artist").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("          - {title} / {artist}");
                }
                for (i, item) in items.iter().take(5).enumerate() {
                    match plugin.media(item, "standard") {
                        Ok(MediaSource { url, .. }) => {
                            println!("media[{i}] : OK  {url}");
                        }
                        Err(err) => println!(
                            "media[{i}] : FAIL {}",
                            format!("{err:#}").lines().next().unwrap_or("")
                        ),
                    }
                }
                if let Some(first) = items.first() {
                    match plugin.lyric(first) {
                        Ok(value) => {
                            let text = value.to_string();
                            println!(
                                "lyric   : OK  {} 字节 {}",
                                text.len(),
                                if text.contains("00:0") { "（像 LRC）" } else { "" }
                            );
                        }
                        Err(err) => println!(
                            "lyric   : FAIL {}",
                            format!("{err:#}").lines().next().unwrap_or("")
                        ),
                    }
                }
            }
            Err(err) => println!("search  : FAIL {err:#}"),
        }
    }
}
