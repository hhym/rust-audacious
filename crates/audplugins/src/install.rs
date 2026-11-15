//! 在线插件安装：从 .zip 压缩包、插件目录、下载地址三种来源装进插件目录。
//!
//! 规则和 MyGo 规范一致：一个插件 = 一个目录，里面有 `plugin.json`（+ `index.js`）。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use crate::plugins_dir;

/// 安装结果，喂给界面弹提示用。
#[derive(Clone, Debug)]
pub struct InstalledPlugin {
    pub name: String,
    pub id: String,
    pub dir: PathBuf,
}

fn sanitize(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "plugin".to_string()
    } else {
        trimmed
    }
}

/// 读 `plugin.json` 里的 `id` / `name`，读不到就用目录名兜底。
fn manifest_of(dir: &Path) -> Option<(String, String)> {
    let raw = std::fs::read_to_string(dir.join("plugin.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if id.is_empty() && name.is_empty() {
        None
    } else {
        Some((id, name))
    }
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("建目录失败 {}", dst.display()))?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).with_context(|| format!("复制失败 {}", from.display()))?;
        }
    }
    Ok(())
}

/// 把一个「含 plugin.json 的插件目录」装进插件目录（同名旧目录先删掉）。
pub fn install_from_dir(src: &Path) -> Result<InstalledPlugin> {
    let src = src
        .canonicalize()
        .with_context(|| format!("目录不存在 {}", src.display()))?;
    if !src.is_dir() {
        bail!("{} 不是目录", src.display());
    }
    if !src.join("plugin.json").is_file() {
        bail!("{} 里没有 plugin.json，不是 MyGo 插件目录", src.display());
    }
    let (id, name) = manifest_of(&src).unwrap_or_else(|| {
        let folder = src
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "plugin".to_string());
        (folder.clone(), folder)
    });
    let id = sanitize(&id);
    let name = if name.is_empty() { id.clone() } else { name };
    let dest = plugins_dir().join(&id);
    if src == dest {
        return Ok(InstalledPlugin {
            name,
            id,
            dir: dest,
        });
    }
    if dest.exists() {
        std::fs::remove_dir_all(&dest)
            .with_context(|| format!("清理旧插件失败 {}", dest.display()))?;
    }
    copy_dir(&src, &dest)?;
    Ok(InstalledPlugin {
        name,
        id,
        dir: dest,
    })
}

fn find_manifest_root(root: &Path, depth: usize) -> Option<PathBuf> {
    if root.join("plugin.json").is_file() {
        return Some(root.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    children.sort();
    for child in children {
        if let Some(hit) = find_manifest_root(&child, depth - 1) {
            return Some(hit);
        }
    }
    None
}

/// 从 .zip 安装：解压后找 plugin.json，把它所在的目录整体装进去。
pub fn install_from_zip(zip_path: &Path) -> Result<InstalledPlugin> {
    let file = std::fs::File::open(zip_path)
        .with_context(|| format!("打不开压缩包 {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("不是有效的 zip 压缩包")?;

    let staging = plugins_dir().join(".install-tmp");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let out = staging.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            let mut f = std::fs::File::create(&out)?;
            f.write_all(&buf)?;
        }
    }

    let result = match find_manifest_root(&staging, 2) {
        Some(root) => install_from_dir(&root),
        None => {
            // 没有 plugin.json：可能是单个 MusicFree .js
            let mut js_files: Vec<PathBuf> = std::fs::read_dir(&staging)?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map(|e| e == "js").unwrap_or(false))
                .collect();
            js_files.sort();
            if let Some(js) = js_files.first() {
                let dest = plugins_dir().join(
                    js.file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "plugin.js".to_string()),
                );
                std::fs::copy(js, &dest)?;
                let stem = dest
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                Ok(InstalledPlugin {
                    name: stem.clone(),
                    id: stem,
                    dir: dest,
                })
            } else {
                Err(anyhow!(
                    "压缩包里没找到 plugin.json（MyGo）或 .js（MusicFree）"
                ))
            }
        }
    };
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// 从网址安装：下载后按内容判断是 zip 还是单个 .js。
pub fn install_from_url(url: &str) -> Result<InstalledPlugin> {
    let url = url.trim();
    if url.is_empty() {
        bail!("先填一个插件下载地址");
    }
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?
        .get(url)
        .send()
        .with_context(|| format!("下载失败 {url}"))?;
    if !resp.status().is_success() {
        bail!("下载失败 {}：HTTP {}", url, resp.status());
    }
    let bytes = resp.bytes()?;
    if bytes.is_empty() {
        bail!("下载到的内容是空的");
    }
    let staging = plugins_dir().join(".download-tmp");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let result = if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        let path = staging.join("plugin.zip");
        std::fs::write(&path, &bytes)?;
        install_from_zip(&path)
    } else if bytes.starts_with(b"<!DOCTYPE") || bytes.starts_with(b"<html") {
        Err(anyhow!(
            "这个地址返回的是网页，不是插件包（请填直链 .zip 或 .js）"
        ))
    } else {
        let name = url
            .split('?')
            .next()
            .unwrap_or(url)
            .rsplit('/')
            .next()
            .filter(|s| s.ends_with(".js"))
            .unwrap_or("plugin.js")
            .to_string();
        let dest = plugins_dir().join(&name);
        std::fs::write(&dest, &bytes)?;
        let id = name.trim_end_matches(".js").to_string();
        Ok(InstalledPlugin {
            name: id.clone(),
            id,
            dir: dest,
        })
    };
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// 统一入口：拖进来的路径可能是 zip、插件目录或 .js。
pub fn install_from_path(path: &Path) -> Result<InstalledPlugin> {
    if path.is_dir() {
        install_from_dir(path)
    } else if path.extension().map(|e| e == "zip").unwrap_or(false) {
        install_from_zip(path)
    } else if path.extension().map(|e| e == "js").unwrap_or(false) {
        let dest = plugins_dir().join(
            path.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "plugin.js".to_string()),
        );
        std::fs::copy(path, &dest)?;
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok(InstalledPlugin {
            name: id.clone(),
            id,
            dir: dest,
        })
    } else {
        bail!("只认 .zip 压缩包、插件目录或 .js 单文件")
    }
}

#[cfg(test)]
mod install_tests {
    use super::{manifest_of, sanitize};

    #[test]
    fn sanitize_keeps_safe_chars() {
        assert_eq!(sanitize("itunes-mygo"), "itunes-mygo");
        assert_eq!(sanitize("扫码 登录/示例"), "plugin");
        assert_eq!(sanitize("a b@c"), "a-b-c");
        assert_eq!(sanitize(""), "plugin");
    }

    #[test]
    fn manifest_reads_id_and_name() {
        let dir = std::env::temp_dir().join("audacious-rs-insall-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"id":"demo-plugin","name":"演示插件","version":"0.1.0"}"#,
        )
        .unwrap();
        let found = manifest_of(&dir).expect("should parse manifest");
        assert_eq!(found.0, "demo-plugin");
        assert_eq!(found.1, "演示插件");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn manifest_missing_returns_none() {
        let dir = std::env::temp_dir().join("audacious-rs-insall-test-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(manifest_of(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
