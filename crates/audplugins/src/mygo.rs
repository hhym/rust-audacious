//! MyGo 插件宿主。
//!
//! 对应 <https://github.com/hhym/mygo-music> 的插件约定：插件是一个目录，
//! 里面有 `plugin.json` 和一个 JS/TS 模块（默认 `index.js`）：
//!
//! ```json
//! {
//!   "id": "example", "name": "Example", "version": "1.0.0", "main": "index.js",
//!   "playback": "stream",
//!   "permissions": { "network": ["api.example.com"] },
//!   "settings": [{ "key": "apiKey", "label": "API Key" }]
//! }
//! ```
//!
//! 模块按名字导出方法，每个方法接收一个对象参数：
//! `search({query, kind, offset})`、`stream({id, quality})`、`lyrics({id})` 等。
//! 宿主在 `mygo_host.js` 里补了 `host` / `fetch` / `URL` / 定时器，
//! 网络访问按 `permissions.network` 白名单限制。

use std::path::{Path, PathBuf};

use boa_engine::{js_string, Context, JsValue, Source};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{invoke_exports, platform_name, storage_access};

/// mygo 的插件清单。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MygoManifest {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, rename = "minAppVersion")]
    pub min_app_version: Option<String>,
    #[serde(default)]
    pub main: Option<String>,
    /// `stream`（宿主播放）/ `remote`（插件自己出声）
    #[serde(default)]
    pub playback: Option<String>,
    #[serde(default)]
    pub permissions: MygoPermissions,
    #[serde(default)]
    pub settings: Vec<SettingDef>,
    #[serde(default)]
    pub navigation: Vec<Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MygoPermissions {
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default)]
    pub process: Value,
}

/// 插件声明的一个设置项（界面上用一个文本框编辑）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SettingDef {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub link: String,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub options: Vec<Value>,
}

/// 一个已加载的 MyGo 插件。
#[derive(Clone)]
pub struct MygoPlugin {
    pub info: PluginInfo,
    pub manifest: MygoManifest,
    pub dir: PathBuf,
    code: String,
}

/// 和 MusicFree 那边保持同样形状的对外描述，方便 UI 统一展示。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginInfo {
    pub path: PathBuf,
    pub platform: String,
    pub version: String,
    pub author: String,
    pub methods: Vec<String>,
    pub error: Option<String>,
}

impl PluginInfo {
    pub fn supports(&self, method: &str) -> bool {
        self.methods.iter().any(|m| m == method)
    }
}

impl MygoPlugin {
    /// 从目录加载：读 `plugin.json`，再跑一遍模块拿到导出的方法。
    pub fn load(dir: &Path) -> MygoPlugin {
        MygoPlugin::load_using(dir, &std::collections::HashMap::new()).0
    }

    /// 带缓存加载：`plugin.json` 或主脚本没改过，就直接用上次的结果。
    pub(crate) fn load_using(
        dir: &Path,
        cache: &std::collections::HashMap<String, crate::cache::Entry>,
    ) -> (MygoPlugin, Option<(String, crate::cache::Entry)>) {
        let manifest_path = dir.join("plugin.json");
        let manifest: MygoManifest = match std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            Some(m) => m,
            None => {
                return (
                    MygoPlugin {
                        info: PluginInfo {
                            path: dir.to_path_buf(),
                            error: Some("缺少或无法解析 plugin.json".into()),
                            ..Default::default()
                        },
                        manifest: MygoManifest::default(),
                        dir: dir.to_path_buf(),
                        code: String::new(),
                    },
                    None,
                )
            }
        };

        let main = manifest.main.clone().unwrap_or_else(|| "index.js".into());
        let script_path = dir.join(&main);
        let code = match std::fs::read_to_string(&script_path) {
            Ok(code) => code,
            Err(err) => {
                return (
                    MygoPlugin {
                        info: PluginInfo {
                            path: dir.to_path_buf(),
                            platform: manifest.name.clone(),
                            version: manifest.version.clone(),
                            error: Some(format!("读取 {main} 失败: {err}")),
                            ..Default::default()
                        },
                        manifest,
                        dir: dir.to_path_buf(),
                        code: String::new(),
                    },
                    None,
                )
            }
        };

        let key = format!(
            "{}|{}",
            crate::cache::key(&manifest_path),
            crate::cache::key(&script_path)
        );
        if let Some(entry) = cache.get(&key) {
            return (
                MygoPlugin {
                    info: PluginInfo {
                        path: dir.to_path_buf(),
                        platform: manifest.name.clone(),
                        version: manifest.version.clone(),
                        methods: entry.methods.clone(),
                        error: entry.error.clone(),
                        ..Default::default()
                    },
                    manifest,
                    dir: dir.to_path_buf(),
                    code,
                },
                None,
            );
        }

        let mut info = PluginInfo {
            path: dir.to_path_buf(),
            platform: manifest.name.clone(),
            version: manifest.version.clone(),
            ..Default::default()
        };
        match inspect(dir, &manifest, &code) {
            Ok(methods) => info.methods = methods,
            Err(err) => info.error = Some(crate::friendly_error(&err.to_string())),
        }

        let entry = crate::cache::Entry {
            platform: info.platform.clone(),
            version: info.version.clone(),
            methods: info.methods.clone(),
            error: info.error.clone(),
            ..Default::default()
        };
        (
            MygoPlugin {
                info,
                manifest,
                dir: dir.to_path_buf(),
                code,
            },
            Some((key, entry)),
        )
    }

    /// 加载超时等情况的占位对象。
    pub(crate) fn synthetic(dir: &Path, error: String) -> MygoPlugin {
        MygoPlugin {
            info: PluginInfo {
                path: dir.to_path_buf(),
                error: Some(error),
                ..Default::default()
            },
            manifest: MygoManifest::default(),
            dir: dir.to_path_buf(),
            code: String::new(),
        }
    }

    pub fn display_name(&self) -> String {
        format!("{} · MyGo", self.manifest.name)
    }

    pub fn settings(&self) -> &[SettingDef] {
        &self.manifest.settings
    }

    /// 读设置项（界面展示用），值的字符串形式。
    pub fn setting(&self, key: &str) -> String {
        let raw = storage_access(&setting_key(&self.manifest.id, key), None);
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::String(s)) => s,
            Ok(other) => other.to_string(),
            Err(_) => String::new(),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        let key = setting_key(&self.manifest.id, key);
        storage_access(&key, Some(&json!(value).to_string()));
    }

    /// 调用任意导出方法，参数是一个对象（和 mygo 的签名一致）。
    pub fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        if let Some(err) = &self.info.error {
            anyhow::bail!("插件不可用: {err}");
        }
        let mut ctx = mygo_context(self);
        let exports = eval_module(&mut ctx, &self.code)
            .map_err(|e| anyhow::anyhow!("插件代码执行失败: {e}"))?;
        invoke_exports(&mut ctx, exports, method, &[params])
    }

    /// 搜索音乐：`search({query, kind:"tracks", offset})`，返回规范化后的条目。
    pub fn search_tracks(&self, query: &str, offset: u64) -> anyhow::Result<Vec<Value>> {
        let out = self.call(
            "search",
            json!({ "query": query, "kind": "tracks", "offset": offset }),
        )?;
        let tracks = out["tracks"].as_array().cloned().unwrap_or_default();
        Ok(tracks.iter().map(track_item).collect())
    }

    /// 取播放地址：`stream({id, quality})` → `{url, type, headers, ...}`
    pub fn stream(&self, item: &Value, quality: &str) -> anyhow::Result<Value> {
        let track = item
            .get("__mygo_raw")
            .cloned()
            .unwrap_or_else(|| item.clone());
        let id = track["id"].clone();
        self.call("stream", json!({ "id": id, "quality": quality, "track": track }))
    }

    /// 插件是否支持扫码登录（导出 `loginQr` 与 `loginPoll`）。
    pub fn supports_login(&self) -> bool {
        self.info.supports("loginQr") && self.info.supports("loginPoll")
    }

    /// 让插件生成一张登录二维码：`loginQr({})` →
    /// `{ key, qrImage?（图片 URL 或 data:image/png;base64,）, qrUrl?（二维码里编的链接）, tip? }`
    pub fn login_qr(&self) -> anyhow::Result<Value> {
        self.call("loginQr", json!({}))
    }

    /// 轮询登录状态：`loginPoll({ key })` → `{ status: waiting|scanned|done|expired|failed, message? }`
    pub fn login_poll(&self, key: &str) -> anyhow::Result<Value> {
        self.call("loginPoll", json!({ "key": key }))
    }

    /// 歌词：`lyrics({id})` → `{lrc, translation}` 或 `{lines}`
    pub fn lyrics(&self, item: &Value) -> anyhow::Result<Value> {
        let track = item
            .get("__mygo_raw")
            .cloned()
            .unwrap_or_else(|| item.clone());
        self.call("lyrics", json!({ "id": track["id"].clone(), "track": track }))
    }
}

/// 把 mygo 的 Track 转成宿主统一使用的条目形状（MusicFree 那套字段 + 原始对象）。
pub fn track_item(track: &Value) -> Value {
    let artists = track["artists"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let album = &track["album"];
    let duration_ms = track["duration"].as_f64().unwrap_or(0.0);
    json!({
        "id": track["id"].clone(),
        "title": track["name"].as_str().unwrap_or("在线歌曲"),
        "artist": artists,
        "album": album["name"].as_str().unwrap_or("在线"),
        "duration": if duration_ms > 0.0 { json!(duration_ms / 1000.0) } else { Value::Null },
        "artwork": album["imageUrl"].as_str().or(track["imageUrl"].as_str()).unwrap_or(""),
        "__mygo_raw": track.clone(),
    })
}

/// 扫描目录，加载所有含 `plugin.json` 的子目录。
pub fn discover_mygo(dir: &Path) -> Vec<MygoPlugin> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join("plugin.json").is_file() {
            continue;
        }
        let plugin = MygoPlugin::load(&path);
        log::info!(
            "MyGo 插件 {} → {}{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            plugin.manifest.name,
            if plugin.info.error.is_some() { "（加载失败）" } else { "" }
        );
        out.push(plugin);
    }
    out.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    out
}

fn setting_key(id: &str, key: &str) -> String {
    format!("mygo/{id}/setting/{key}")
}

fn syntax_check(code: &str) -> Result<(), String> {
    use boa_engine::parser::Parser;
    let mut parser = Parser::new(boa_engine::Source::from_bytes(code));
    let scope = boa_engine::ast::scope::Scope::new_global();
    let mut interner = boa_engine::interner::Interner::new();
    parser
        .parse_script(&scope, &mut interner)
        .map(|_| ())
        .map_err(|e| crate::friendly_error(&format!("{e}")))
}

/// 从源码读 `export function` / `export const` 的导出名，不用执行模块。
fn static_exports(code: &str) -> Vec<String> {
    let re = regex::Regex::new(
        r#"export\s+(?:default\s+)?(?:async\s+)?(?:function\*?|const|let|var|class)\s+([A-Za-z_$][A-Za-z0-9_$]*)"#,
    );
    let mut out = Vec::new();
    if let Ok(re) = re {
        for c in re.captures_iter(code) {
            if let Some(name) = c.get(1) {
                out.push(name.as_str().to_string());
            }
        }
    }
    out
}

fn inspect(dir: &Path, manifest: &MygoManifest, code: &str) -> anyhow::Result<Vec<String>> {
    // 语法检查要看改写后的脚本（原文件是 ES 模块，直接按 Script 解析会报 export 错）
    let (body, _) = transform_module(code);
    syntax_check(&body).map_err(|e| anyhow::anyhow!(e))?;
    let names = static_exports(code);
    if !names.is_empty() {
        return Ok(names);
    }
    describe(dir, manifest, code)
}

fn describe(dir: &Path, manifest: &MygoManifest, code: &str) -> anyhow::Result<Vec<String>> {
    let mut ctx = mygo_context_parts_with_limit(dir, manifest, code, crate::LOAD_LOOP_LIMIT);
    let exports = eval_module(&mut ctx, code)?;
    ctx.register_global_property(
        js_string!("__mygoExports"),
        exports,
        boa_engine::property::Attribute::all(),
    )
    .ok();
    let summary = ctx
        .eval(Source::from_bytes(
            r#"(function () {
  var e = __mygoExports || {};
  var names = Object.keys(e).filter(function (k) { return typeof e[k] === "function"; });
  return JSON.stringify(names);
})()"#,
        ))
        .map_err(|e| anyhow::anyhow!("读取插件导出失败: {e}"))?
        .as_string()
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_else(|| "[]".into());
    Ok(serde_json::from_str::<Vec<String>>(&summary).unwrap_or_default())
}

fn mygo_context(plugin: &MygoPlugin) -> Context {
    mygo_context_parts(&plugin.dir, &plugin.manifest, &plugin.code)
}

fn mygo_context_parts(dir: &Path, manifest: &MygoManifest, code: &str) -> Context {
    mygo_context_parts_with_limit(dir, manifest, code, crate::RUNTIME_LOOP_LIMIT)
}

fn mygo_context_parts_with_limit(
    dir: &Path,
    manifest: &MygoManifest,
    _code: &str,
    loop_limit: u64,
) -> Context {
    let mut ctx = crate::new_context_with_limit(loop_limit);

    let data_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(audcore::APP_NAME)
        .join("mygo-data")
        .join(&manifest.id);

    let locale = load_locale(dir);
    let platform = match platform_name() {
        "win32" => "windows",
        other => other,
    };
    let info = json!({
        "id": manifest.id,
        "dir": dir.to_string_lossy(),
        "dataDir": data_dir.to_string_lossy(),
        "platform": platform,
        "arch": std::env::consts::ARCH,
        "language": language(),
        "network": manifest.permissions.network,
        "locale": locale,
    });

    ctx.register_global_property(
        js_string!("__mygoInfo"),
        JsValue::from(boa_engine::JsString::from(info.to_string())),
        boa_engine::property::Attribute::all(),
    )
    .ok();

    if let Err(err) = ctx.eval(Source::from_bytes(include_str!("mygo_host.js"))) {
        log::error!("MyGo 宿主环境初始化失败: {err}");
    }
    ctx
}

fn language() -> &'static str {
    let lang = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default()
        .to_lowercase();
    if lang.contains("zh") || lang.is_empty() {
        "zh-CN"
    } else {
        "en"
    }
}

fn load_locale(dir: &Path) -> Value {
    let lang = language();
    let path = dir.join("locales").join(format!("{lang}.json"));
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

/// 把 ES 模块的 `import` / `export` 改写成宿主能跑的形式，并记下导出的名字。
fn transform_module(src: &str) -> (String, Vec<(String, String)>) {
    let mut exported: Vec<(String, String)> = Vec::new();
    let mut out = src.to_string();

    // import：转换成 require
    let re_import_all = Regex::new(r#"(?m)import\s+\*\s+as\s+([A-Za-z_$][\w$]*)\s+from\s+['"]([^'"]+)['"]\s*;"#).unwrap();
    out = re_import_all
        .replace_all(&out, |c: &regex::Captures| format!("const {} = require(\"{}\");", &c[1], &c[2]))
        .into_owned();

    let re_import_default =
        Regex::new(r#"(?m)import\s+([A-Za-z_$][\w$]*)\s+from\s+['"]([^'"]+)['"]\s*;"#).unwrap();
    out = re_import_default
        .replace_all(&out, |c: &regex::Captures| format!("const {} = require(\"{}\");", &c[1], &c[2]))
        .into_owned();

    let re_import_named =
        Regex::new(r#"(?m)import\s+\{(.*?)\}\s+from\s+['"]([^'"]+)['"]\s*;"#).unwrap();
    out = re_import_named
        .replace_all(&out, |c: &regex::Captures| {
            let list = c[1]
                .split(',')
                .map(|p| p.trim().replace(" as ", ": "))
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            format!("const {{ {list} }} = require(\"{}\");", &c[2])
        })
        .into_owned();

    let re_import_side = Regex::new(r#"(?m)import\s+['"]([^'"]+)['"]\s*;"#).unwrap();
    out = re_import_side
        .replace_all(&out, |c: &regex::Captures| format!("require(\"{}\");", &c[1]))
        .into_owned();

    // export default function NAME / async function NAME
    let re_default_fn = Regex::new(
        r#"(?m)export\s+default\s+(async\s+)?function\s+([A-Za-z_$][\w$]*)"#,
    )
    .unwrap();
    out = re_default_fn
        .replace_all(&out, |c: &regex::Captures| {
            let name = c[2].to_string();
            exported.push(("default".to_string(), name.clone()));
            exported.push((name.clone(), name.clone()));
            format!("{}function {}", c.get(1).map(|m| m.as_str()).unwrap_or(""), name)
        })
        .into_owned();

    // export [async] function NAME
    let re_export_fn =
        Regex::new(r#"(?m)export\s+(async\s+)?function\s+([A-Za-z_$][\w$]*)"#).unwrap();
    out = re_export_fn
        .replace_all(&out, |c: &regex::Captures| {
            let name = c[2].to_string();
            exported.push((name.clone(), name.clone()));
            format!("{}function {}", c.get(1).map(|m| m.as_str()).unwrap_or(""), name)
        })
        .into_owned();

    // export const/let/var NAME
    let re_export_var =
        Regex::new(r#"(?m)export\s+(const|let|var)\s+([A-Za-z_$][\w$]*)"#).unwrap();
    out = re_export_var
        .replace_all(&out, |c: &regex::Captures| {
            exported.push((c[2].to_string(), c[2].to_string()));
            format!("{} {}", &c[1], &c[2])
        })
        .into_owned();

    // export class NAME
    let re_export_class = Regex::new(r#"(?m)export\s+class\s+([A-Za-z_$][\w$]*)"#).unwrap();
    out = re_export_class
        .replace_all(&out, |c: &regex::Captures| {
            exported.push((c[1].to_string(), c[1].to_string()));
            format!("class {}", &c[1])
        })
        .into_owned();

    // export { a, b as c }
    let re_export_list = Regex::new(r#"(?m)export\s+\{(.*?)\};?"#).unwrap();
    out = re_export_list
        .replace_all(&out, |c: &regex::Captures| {
            for piece in c[1].split(',') {
                let piece = piece.trim();
                if piece.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = piece.split(" as ").map(|s| s.trim()).collect();
                if parts.len() == 2 {
                    exported.push((parts[1].to_string(), parts[0].to_string()));
                } else {
                    exported.push((parts[0].to_string(), parts[0].to_string()));
                }
            }
            String::new()
        })
        .into_owned();

    // export default <expression>
    let re_export_default_expr = Regex::new(r#"(?m)export\s+default\s+"#).unwrap();
    out = re_export_default_expr
        .replace_all(&out, "const __default = ")
        .into_owned();
    exported.push(("default".to_string(), "__default".to_string()));

    (out, exported)
}

/// 执行模块，返回它的导出对象。
fn eval_module(ctx: &mut Context, code: &str) -> anyhow::Result<JsValue> {
    let (body, exported) = transform_module(code);
    let mut script = String::new();
    script.push_str("var exports = {};\n");
    script.push_str(&body);
    script.push_str("\n;(function () { var e = {};\n");
    for (key, name) in exported {
        script.push_str(&format!(
            "if (typeof {name} !== 'undefined') e[{}] = {name};\n",
            serde_json::to_string(&key).unwrap_or_else(|_| "\"\"".to_string())
        ));
    }
    script.push_str("exports = e; return e; })()");

    ctx.eval(Source::from_bytes(&script))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plugin(dir: &Path, manifest: &str, js: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("plugin.json"), manifest).unwrap();
        std::fs::write(dir.join("index.js"), js).unwrap();
    }

    #[test]
    fn esm_transform_collects_exports() {
        let (body, names) = transform_module(
            r#"
import { a as b } from "lib";
export async function search({ query }) { return { tracks: [{ id: "1", name: query, artists: [{ name: "A" }] }], more: false }; }
export function stream({ id }) { return { url: "https://example.com/" + id + ".mp3", type: "mp3" }; }
export const extra = 1;
"#,
        );
        assert!(body.contains("const { a: b } = require(\"lib\");"));
        assert!(body.contains("async function search"));
        assert!(body.contains("function stream"));
        let got: Vec<&str> = names.iter().map(|(k, _)| k.as_str()).collect();
        assert!(got.contains(&"search"));
        assert!(got.contains(&"stream"));
    }

    #[test]
    fn mygo_qr_login() {
        let dir = std::env::temp_dir().join(format!("mygo-qr-{}", std::process::id()));
        write_plugin(
            &dir,
            r#"{"id":"qr","name":"QR","version":"1.0.0","main":"index.js"}"#,
            r#"
export async function loginQr() {
  return { key: "k1", qrUrl: "https://example.com/login", tip: "用手机扫" };
}
export async function loginPoll({ key }) {
  return { status: key === "k1" ? "done" : "failed", message: "ok" };
}
export async function search() { return { tracks: [], more: false }; }
"#,
        );
        let plugin = MygoPlugin::load(&dir);
        assert!(plugin.info.error.is_none(), "{:?}", plugin.info.error);
        assert!(plugin.supports_login());
        let qr = plugin.login_qr().unwrap();
        assert_eq!(qr["key"], "k1");
        assert_eq!(qr["qrUrl"], "https://example.com/login");
        let poll = plugin.login_poll("k1").unwrap();
        assert_eq!(poll["status"], "done");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mygo_abort_controller() {
        let dir = std::env::temp_dir().join(format!("mygo-abort-{}", std::process::id()));
        write_plugin(
            &dir,
            r#"{"id":"abort","name":"Abort","version":"1.0.0","main":"index.js"}"#,
            r#"
export async function search() {
  const controller = new AbortController();
  let fired = false;
  controller.signal.addEventListener("abort", () => { fired = true; });
  controller.abort();
  const aborted = AbortSignal.abort();
  return {
    tracks: [{
      id: "1",
      name: (controller.signal.aborted && fired && aborted.aborted) ? "aborted" : "bad",
      artists: [{ id: "a", name: "t" }],
    }],
    more: false,
  };
}
"#,
        );
        let plugin = MygoPlugin::load(&dir);
        assert!(plugin.info.error.is_none(), "{:?}", plugin.info.error);
        let tracks = plugin.search_tracks("x", 1).unwrap();
        assert_eq!(tracks[0]["title"], "aborted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mygo_plugin_search_stream_lyrics() {
        let dir = std::env::temp_dir().join(format!("mygo-test-{}", std::process::id()));
        write_plugin(
            &dir,
            r#"{"id":"t","name":"T","version":"1.0.0","main":"index.js","permissions":{"network":["api.example.com"]}}"#,
            r#"
export async function search({ query, kind, offset }) {
  return { tracks: [{ id: "t1", name: "Song " + query, artists: [{ id: "a", name: "Artist" }],
    album: { id: "al", name: "Album", imageUrl: "https://img.example.com/c.jpg" }, duration: 125000 }], more: false };
}
export function stream({ id, quality }) {
  return { url: "https://api.example.com/" + id + ".mp3", type: "mp3", headers: { referer: "x" } };
}
export function lyrics({ id }) { return { lrc: "[00:00]" + id }; }
export function broken() { throw new Error("boom"); }
"#,
        );
        let plugin = MygoPlugin::load(&dir);
        assert!(plugin.info.error.is_none(), "{:?}", plugin.info.error);
        assert!(plugin.info.supports("search"));
        assert!(plugin.info.supports("stream"));

        let items = plugin.search_tracks("Hello", 0).unwrap();
        assert_eq!(items[0]["title"], Value::from("Song Hello"));
        assert_eq!(items[0]["artist"], Value::from("Artist"));
        assert_eq!(items[0]["duration"], Value::from(125.0));

        let src = plugin.stream(&items[0], "standard").unwrap();
        assert_eq!(src["url"], Value::from("https://api.example.com/t1.mp3"));
        assert_eq!(src["type"], Value::from("mp3"));

        let lrc = plugin.lyrics(&items[0]).unwrap();
        assert_eq!(lrc["lrc"], Value::from("[00:00]t1"));

        let err = plugin.call("broken", json!({})).unwrap_err().to_string();
        assert!(err.contains("boom"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mygo_settings_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mygo-set-{}", std::process::id()));
        write_plugin(
            &dir,
            r#"{"id":"s","name":"S","settings":[{"key":"apiKey","label":"API Key"}]}"#,
            r#"export function ping() { return { v: host.setting("apiKey") }; }"#,
        );
        let plugin = MygoPlugin::load(&dir);
        plugin.set_setting("apiKey", "secret-token");
        let out = plugin.call("ping", json!({})).unwrap();
        assert_eq!(out["v"], Value::from("secret-token"));
        plugin.set_setting("apiKey", "");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
