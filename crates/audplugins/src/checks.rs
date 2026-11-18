//! 插件检测记录：把最近一次「检测」的结果存到磁盘，重启后行内标签还在。
//!
//! 和 `plugin-cache.json` 分开存，删缓存不会顺手把检测结论清掉。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 单个插件的最近一次检测结论。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckEntry {
    /// 是否通过（能跑通一次搜索）
    pub ok: bool,
    /// 一句话结论，例如「18 条 · 1477ms」或「search: 503」
    pub detail: String,
    /// 检测时间（Unix 秒）
    pub at: u64,
}

pub fn checks_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(audcore::APP_NAME)
        .join("plugin-checks.json")
}

pub fn load() -> HashMap<String, CheckEntry> {
    std::fs::read_to_string(checks_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(map: &HashMap<String, CheckEntry>) {
    let path = checks_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(path, text);
    }
}

/// 现在这一刻的 Unix 秒。
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 把时间戳说成人话：刚刚 / N 分钟前 / N 小时前 / N 天前。
pub fn humanize_age(at: u64) -> String {
    let elapsed = now().saturating_sub(at);
    match elapsed {
        0..=59 => "刚刚".to_string(),
        60..=3599 => format!("{} 分钟前", elapsed / 60),
        3600..=86399 => format!("{} 小时前", elapsed / 3600),
        _ => format!("{} 天前", elapsed / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_buckets() {
        // 用「现在往前推」构造，避免写死时间戳后越跑越久
        let now = now();
        assert_eq!(humanize_age(now), "刚刚");
        assert_eq!(humanize_age(now.saturating_sub(120)), "2 分钟前");
        assert_eq!(humanize_age(now.saturating_sub(7_200)), "2 小时前");
        assert_eq!(humanize_age(now.saturating_sub(259_200)), "3 天前");
    }
}
