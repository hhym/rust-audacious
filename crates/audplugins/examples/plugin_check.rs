//! 插件体检小工具：扫一个插件目录，对每个插件跑一次搜索并打印结果。
//!
//! ```text
//! cargo +stable run -q --example plugin_check -- ~/.config/audacious-rs/plugins test 周杰伦
//! ```
//!
//! 输出每行：`插件名  关键词  OK/FAIL  条数  耗时`，失败时只留 `friendly_error` 第一行。

use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = match args.next() {
        Some(dir) => dir,
        None => {
            eprintln!("用法：plugin_check <插件目录> [关键词...]");
            return;
        }
    };
    let queries: Vec<String> = {
        let rest: Vec<String> = args.collect();
        if rest.is_empty() {
            vec!["test".to_string()]
        } else {
            rest
        }
    };

    let rx = audplugins::scan_async(dir.into());
    let plugins = match rx.recv_timeout(Duration::from_secs(90)) {
        Ok(list) => list,
        Err(err) => {
            eprintln!("扫描插件目录失败：{err}");
            return;
        }
    };
    println!("共 {} 个在线插件", plugins.len());

    for plugin in &plugins {
        let name = plugin.name();
        if let Some(err) = plugin.error() {
            println!("{name}\t加载失败\t{err}");
            continue;
        }
        for query in &queries {
            let started = Instant::now();
            match plugin.search_tracks(query, 1) {
                Ok(tracks) => println!(
                    "{name}\t{query}\tOK\t{}条\t{}ms",
                    tracks.len(),
                    started.elapsed().as_millis()
                ),
                Err(err) => {
                    let brief: String = audplugins::friendly_error(&err.to_string())
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(100)
                        .collect();
                    println!("{name}\t{query}\tFAIL\t{brief}\t{}ms", started.elapsed().as_millis());
                }
            }
        }
    }
}
