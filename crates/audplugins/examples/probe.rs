//! 手工验证：`cargo run -p audplugins --example probe -- <plugin.js> [query]`
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe <plugin.js> [query]");
    let query = std::env::args().nth(2).unwrap_or_else(|| "guitar".into());
    let plugin = audplugins::Plugin::load(Path::new(&path));
    println!("info: {:#?}", plugin.info);
    if plugin.info.supports("search") {
        match plugin.search(&query, 1, "music") {
            Ok(v) => {
                let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                println!("result ({} chars):\n{}", text.len(), text.chars().take(1200).collect::<String>());
            }
            Err(e) => println!("search error: {e}"),
        }
    }
}
