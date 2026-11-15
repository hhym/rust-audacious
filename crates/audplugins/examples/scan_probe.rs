//! 扫描插件目录，打印耗时和加载失败的插件：
//! `cargo run -p audplugins --example scan_probe -- <目录>`

use std::time::Instant;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "examples/plugins".to_string());
    let start = Instant::now();
    let rx = audplugins::scan_async(std::path::PathBuf::from(&dir));
    let plugins = rx.recv().unwrap_or_default();
    let elapsed = start.elapsed();

    println!("{} 个插件，耗时 {:?}", plugins.len(), elapsed);
    for p in &plugins {
        match p.error() {
            Some(err) => println!("  [失败] {} :: {} :: {}", p.name(), p.path().display(), first_line(err)),
            None => println!(
                "  [OK] {} v{} {:?}",
                p.name(),
                p.version(),
                p.methods_brief()
            ),
        }
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").to_string()
}
