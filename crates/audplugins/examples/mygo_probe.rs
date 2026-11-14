//! 手工验证 MyGo 插件：`cargo run -p audplugins --example mygo_probe -- <插件目录> [关键词]`

use serde_json::Value;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "examples/mygo-plugins/itunes".to_string());
    let query = args.get(2).cloned().unwrap_or_else(|| "hello".to_string());

    let plugin = audplugins::MygoPlugin::load(std::path::Path::new(&dir));
    println!("name: {}\nversion: {}\nerror: {:?}\nmethods: {:?}", plugin.manifest.name, plugin.manifest.version, plugin.info.error, plugin.info.methods);

    match plugin.search_tracks(&query, 0) {
        Ok(items) => {
            println!("search: {} items", items.len());
            if let Some(first) = items.first() {
                println!("{}", serde_json::to_string_pretty(first).unwrap_or_default());
                if plugin.info.supports("stream") {
                    match plugin.stream(first, "standard") {
                        Ok(src) => println!("stream: {}", serde_json::to_string(&src).unwrap_or_default()),
                        Err(err) => println!("stream error: {err}"),
                    }
                }
                if plugin.info.supports("lyrics") {
                    match plugin.lyrics(first) {
                        Ok(v) => println!("lyrics: {}", serde_json::to_string(&v).unwrap_or_default()),
                        Err(err) => println!("lyrics error: {err}"),
                    }
                }
            }
        }
        Err(err) => println!("search error: {err}"),
    }

    // 也验证一下设置项存储
    let settings: Vec<Value> = plugin
        .settings()
        .iter()
        .map(|s| serde_json::json!({ "key": s.key, "label": s.label, "value": plugin.setting(&s.key) }))
        .collect();
    println!("settings: {}", serde_json::to_string(&settings).unwrap_or_default());
}
