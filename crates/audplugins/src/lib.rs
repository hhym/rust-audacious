//! MusicFree 插件宿主。
//!
//! 插件标准与 <https://github.com/hhym/MusicFreePlugins> 保持一致：一个 `.js` 文件，
//! `module.exports = { platform, version, search(query, page, type), getMediaSource(item, quality), ... }`，
//! 运行时可 `require` 到 axios / cheerio / crypto-js / dayjs / qs / he / big-integer 等库。
//!
//! 实现方式：用 `boa_engine` 跑 JS，库由 `polyfill.js` 提供；HTTP、哈希、存储这些
//! 重活交给 Rust 原生函数（`__http` / `__hash` / `__storage`）。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsString, JsValue, NativeFunction, Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 插件目录：`<config>/audacious-rs/plugins`
pub fn plugins_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(audcore::APP_NAME)
        .join("plugins")
}

/// 插件对外的一层描述，UI 用这个列表。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginInfo {
    pub path: PathBuf,
    pub platform: String,
    pub version: String,
    pub author: String,
    /// 插件实现了哪些方法（search / getMediaSource / ...）
    pub methods: Vec<String>,
    /// 加载失败时的原因
    pub error: Option<String>,
}

impl PluginInfo {
    pub fn supports(&self, method: &str) -> bool {
        self.methods.iter().any(|m| m == method)
    }
}

/// 一个已加载的插件（每次调用都新建 JS 上下文，插件之间互不影响）。
#[derive(Clone)]
pub struct Plugin {
    pub info: PluginInfo,
    code: String,
}

impl Plugin {
    /// 读文件并跑一遍，拿到 platform / version / 方法列表。
    pub fn load(path: &Path) -> Plugin {
        let code = match std::fs::read_to_string(path) {
            Ok(code) => code,
            Err(err) => {
                return Plugin {
                    info: PluginInfo {
                        path: path.to_path_buf(),
                        error: Some(format!("读取失败: {err}")),
                        ..Default::default()
                    },
                    code: String::new(),
                }
            }
        };

        let mut info = PluginInfo {
            path: path.to_path_buf(),
            ..Default::default()
        };

        match describe(&code) {
            Ok(meta) => {
                info.platform = meta.platform;
                info.version = meta.version;
                info.author = meta.author;
                info.methods = meta.methods;
            }
            Err(err) => info.error = Some(err.to_string()),
        }

        Plugin { info, code }
    }

    /// 调用插件方法，参数和返回值都是 JSON（和 MusicFree 的签名一致）。
    pub fn call(&self, method: &str, args: &[Value]) -> anyhow::Result<Value> {
        if let Some(err) = &self.info.error {
            anyhow::bail!("插件不可用: {err}");
        }
        call_method(&self.code, method, args)
    }

    /// 搜索：`search(query, page, type)`，返回 `{ isEnd, data: [...] }`
    pub fn search(&self, query: &str, page: u32, kind: &str) -> anyhow::Result<Value> {
        self.call(
            "search",
            &[
                Value::String(query.to_string()),
                Value::from(page),
                Value::String(kind.to_string()),
            ],
        )
    }

    /// 取播放地址：`getMediaSource(item, quality)`，返回 `{ url, headers, ... }`
    pub fn media_source(&self, item: &Value, quality: &str) -> anyhow::Result<Value> {
        self.call(
            "getMediaSource",
            &[item.clone(), Value::String(quality.to_string())],
        )
    }

    /// 歌词：`getLyric(item)`，返回 `{ rawLrc, lrc }`
    pub fn lyric(&self, item: &Value) -> anyhow::Result<Value> {
        self.call("getLyric", &[item.clone()])
    }
}

/// 扫描目录，加载所有 `.js` 插件。
pub fn discover(dir: &Path) -> Vec<Plugin> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("js") {
            continue;
        }
        let plugin = Plugin::load(&path);
        log::info!(
            "插件 {} → {}{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            plugin.info.platform,
            if plugin.info.error.is_some() {
                "（加载失败）"
            } else {
                ""
            }
        );
        out.push(plugin);
    }
    out.sort_by(|a, b| a.info.platform.cmp(&b.info.platform));
    out
}

#[derive(Debug, Default)]
struct Meta {
    platform: String,
    version: String,
    author: String,
    methods: Vec<String>,
}

/// 跑一遍插件代码，把导出的元信息和方法名读出来。
fn describe(code: &str) -> anyhow::Result<Meta> {
    let mut ctx = new_context();
    eval_plugin(&mut ctx, code)?;

    let summary = ctx
        .eval(Source::from_bytes(
            r#"(function () {
  var m = module.exports || {};
  if (m && m.default && typeof m.default === "object") m = m.default;
  return JSON.stringify({
    platform: String(m.platform || ""),
    version: String(m.version || ""),
    author: String(m.author || ""),
    methods: Object.keys(m).filter(function (k) { return typeof m[k] === "function"; })
  });
})()"#,
        ))
        .map_err(|e| anyhow::anyhow!("读取插件信息失败: {e}"))?
        .as_string()
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default();

    let v: Value = serde_json::from_str(&summary).unwrap_or(Value::Null);
    Ok(Meta {
        platform: v["platform"].as_str().unwrap_or_default().to_string(),
        version: v["version"].as_str().unwrap_or_default().to_string(),
        author: v["author"].as_str().unwrap_or_default().to_string(),
        methods: v["methods"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn call_method(code: &str, method: &str, args: &[Value]) -> anyhow::Result<Value> {
    let mut ctx = new_context();
    let exports = eval_plugin(&mut ctx, code)?;
    let args_value = JsValue::from_json(&Value::Array(args.to_vec()), &mut ctx)
        .map_err(|e| anyhow::anyhow!("参数转换失败: {e}"))?;

    ctx.register_global_property(js_string!("__plugin"), exports, Attribute::all())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.register_global_property(js_string!("__args"), args_value, Attribute::all())
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let src = format!(
        r#"
__invokeDone = false; __invokeResult = null; __invokeError = null;
(function () {{
  try {{
    var m = __plugin;
    if (m && m.default && typeof m.default === "object" && typeof m[{method}] !== "function") m = m.default;
    var fn = m[{method}];
    if (typeof fn !== "function") {{
      __invokeError = "插件未实现 " + {method};
      __invokeDone = true;
      return;
    }}
    Promise.resolve(fn.apply(m, __args)).then(
      function (v) {{ __invokeResult = JSON.stringify(v === undefined ? null : v); __invokeDone = true; }},
      function (e) {{ __invokeError = (e && e.message) ? (e.message + "\n" + (e.stack || "")) : String(e); __invokeDone = true; }}
    );
  }} catch (e) {{
    __invokeError = (e && e.message) ? (e.message + "\n" + (e.stack || "")) : String(e);
    __invokeDone = true;
  }}
}})();
"#,
        method = serde_json::to_string(method).unwrap_or_else(|_| "\"\"".to_string()),
    );

    ctx.eval(Source::from_bytes(&src))
        .map_err(|e| anyhow::anyhow!("调用 {method} 失败: {e}"))?;

    // 把微任务队列跑干净；宿主 HTTP 是同步的，通常一两轮就结束
    for _ in 0..64 {
        ctx.run_jobs().ok();
        if global_bool(&mut ctx, "__invokeDone") {
            break;
        }
    }

    if let Some(err) = global_string(&mut ctx, "__invokeError") {
        anyhow::bail!("{err}");
    }
    let result = global_string(&mut ctx, "__invokeResult")
        .ok_or_else(|| anyhow::anyhow!("{method} 没有返回结果"))?;
    Ok(serde_json::from_str(&result)?)
}

fn global_bool(ctx: &mut Context, key: &str) -> bool {
    ctx.global_object()
        .get(js_string!(key), ctx)
        .map(|v| v.as_boolean().unwrap_or(false))
        .unwrap_or(false)
}

fn global_string(ctx: &mut Context, key: &str) -> Option<String> {
    ctx.global_object()
        .get(js_string!(key), ctx)
        .ok()
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
}

/// 在上下文里执行插件代码，导出 `module.exports`。
fn eval_plugin(ctx: &mut Context, code: &str) -> anyhow::Result<JsValue> {
    let wrapped = format!(
        "var module = {{ exports: {{}} }};\nvar exports = module.exports;\n{code}\n;module.exports;"
    );
    ctx.eval(Source::from_bytes(&wrapped))
        .map_err(|e| anyhow::anyhow!("插件代码执行失败: {e}"))
}

fn new_context() -> Context {
    let mut ctx = Context::default();

    ctx.register_global_property(
        js_string!("__platform"),
        JsValue::from(JsString::from(platform_name())),
        Attribute::all(),
    )
    .ok();
    ctx.register_global_property(
        js_string!("__appVersion"),
        JsValue::from(JsString::from(audcore::VERSION)),
        Attribute::all(),
    )
    .ok();

    ctx.register_global_callable(
        js_string!("__http"),
        1,
        NativeFunction::from_fn_ptr(native_http),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__hash"),
        3,
        NativeFunction::from_fn_ptr(native_hash),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__storage"),
        2,
        NativeFunction::from_fn_ptr(native_storage),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__log"),
        1,
        NativeFunction::from_fn_ptr(native_log),
    )
    .ok();

    if let Err(err) = ctx.eval(Source::from_bytes(include_str!("polyfill.js"))) {
        log::error!("插件运行时初始化失败: {err}");
    }
    ctx
}

fn platform_name() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// 把在线音源落成缓存文件，返回本地路径。
///
/// 播放器（rodio）需要一个可随机访问的文件，这里先下载到 `<cache>/audacious-rs/streams`。
pub fn fetch_to_cache(url: &str) -> anyhow::Result<PathBuf> {
    use std::io::Write;

    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(audcore::APP_NAME)
        .join("streams");
    std::fs::create_dir_all(&dir)?;

    let hash = format!("{:x}", {
        use md5::{Digest, Md5};
        Md5::digest(url.as_bytes())
    });
    let ext = url_extension(url);
    let path = dir.join(format!("{hash}.{ext}"));
    if path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(path);
    }

    let resp = client().get(url).send()?.error_for_status()?;
    let tmp = path.with_extension("part");
    {
        let mut file = std::fs::File::create(&tmp)?;
        let mut reader = resp;
        std::io::copy(&mut reader, &mut file)?;
        file.flush()?;
    }
    std::fs::rename(&tmp, &path)?;
    log::info!("在线音源已缓存 {} bytes → {}", path.metadata().map(|m| m.len()).unwrap_or(0), path.display());
    Ok(path)
}

fn url_extension(url: &str) -> &'static str {
    let lower = url.to_lowercase();
    for ext in ["mp3", "flac", "m4a", "aac", "ogg", "opus", "wav", "webm", "mkv", "mp4"] {
        if lower.contains(&format!(".{ext}")) {
            return ext;
        }
    }
    "mp3"
}

// ------------------------------------------------------------------ 原生函数

fn native_log(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let msg = args
        .first()
        .map(|v| {
            v.to_string(ctx)
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    log::info!("[plugin] {msg}");
    Ok(JsValue::undefined())
}

fn native_http(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let raw = args
        .first()
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_default();
    Ok(JsValue::from(JsString::from(http_request(&raw))))
}

fn native_hash(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let algo = args
        .first()
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_default();
    let data = args
        .get(1)
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_default();
    let key = args
        .get(2)
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()));
    Ok(JsValue::from(JsString::from(hash_hex_b64(
        &algo,
        &data,
        key.as_deref(),
    ))))
}

fn native_storage(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let key = args
        .first()
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_default();
    let value = args
        .get(1)
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()));
    Ok(JsValue::from(JsString::from(storage_access(
        &key,
        value.as_deref(),
    ))))
}

// ------------------------------------------------------------------ HTTP

fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent("Mozilla/5.0 audacious-rs")
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
}

/// 把 JS 传进来的配置转成一次真实请求，返回 JSON 字符串。
fn http_request(config_json: &str) -> String {
    let cfg: Value = serde_json::from_str(config_json).unwrap_or(Value::Null);
    let url = cfg["url"].as_str().unwrap_or_default().to_string();
    let method = cfg["method"].as_str().unwrap_or("GET").to_uppercase();

    if url.is_empty() {
        return serde_json::json!({ "error": "url 为空" }).to_string();
    }

    let mut builder = client().request(
        method
            .parse()
            .unwrap_or(reqwest::Method::GET),
        &url,
    );

    if let Some(headers) = cfg["headers"].as_object() {
        for (k, v) in headers {
            if let Some(value) = v.as_str() {
                builder = builder.header(k, value);
            }
        }
    }

    if let Some(body) = cfg["body"].as_str() {
        builder = builder.body(body.to_string());
    }

    let timeout = cfg["timeout"].as_u64().unwrap_or(20_000);
    builder = builder.timeout(std::time::Duration::from_millis(timeout.min(60_000)));

    match builder.send() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let final_url = resp.url().to_string();
            let headers: serde_json::Map<String, Value> = resp
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        Value::String(v.to_str().unwrap_or_default().to_string()),
                    )
                })
                .collect();
            let body = resp.text().unwrap_or_default();
            serde_json::json!({
                "status": status,
                "headers": headers,
                "body": body,
                "url": final_url,
            })
            .to_string()
        }
        Err(err) => serde_json::json!({ "error": err.to_string() }).to_string(),
    }
}

// ------------------------------------------------------------------ 哈希 / 存储

fn hash_hex_b64(algo: &str, data: &str, key: Option<&str>) -> String {
    use hmac::{Hmac, Mac};

    let (hex, b64) = match algo {
        "sha1" => {
            use sha1::{Digest, Sha1};
            let digest = Sha1::digest(data.as_bytes());
            (hex::encode(digest), b64(digest.to_vec()))
        }
        "md5" => {
            use md5::{Digest, Md5};
            let digest = Md5::digest(data.as_bytes());
            (hex::encode(digest), b64(digest.to_vec()))
        }
        "hmac-sha1" => {
            type H = Hmac<sha1::Sha1>;
            let mut mac = H::new_from_slice(key.unwrap_or("").as_bytes()).expect("hmac key");
            mac.update(data.as_bytes());
            let bytes = mac.finalize().into_bytes().to_vec();
            (hex::encode(&bytes), b64(bytes))
        }
        "hmac-md5" => {
            use md5::Md5;
            type H = Hmac<Md5>;
            let mut mac = H::new_from_slice(key.unwrap_or("").as_bytes()).expect("hmac key");
            mac.update(data.as_bytes());
            let bytes = mac.finalize().into_bytes().to_vec();
            (hex::encode(&bytes), b64(bytes))
        }
        "hmac-sha256" => {
            use sha2::Sha256;
            type H = Hmac<Sha256>;
            let mut mac = H::new_from_slice(key.unwrap_or("").as_bytes()).expect("hmac key");
            mac.update(data.as_bytes());
            let bytes = mac.finalize().into_bytes().to_vec();
            (hex::encode(&bytes), b64(bytes))
        }
        _ => {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(data.as_bytes());
            (hex::encode(digest), b64(digest.to_vec()))
        }
    };

    serde_json::json!({ "hex": hex, "b64": b64 }).to_string()
}

fn b64(bytes: Vec<u8>) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn storage_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(audcore::APP_NAME)
        .join("plugin-storage.json")
}

fn storage_access(key: &str, value: Option<&str>) -> String {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let path = storage_path();
    let mut map: serde_json::Map<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();

    let result = map
        .get(key)
        .map(|v| v.to_string())
        .unwrap_or_else(|| "null".to_string());

    if let Some(raw) = value {
        if raw.trim() == "null" {
            map.remove(key);
        } else {
            map.insert(key.to_string(), serde_json::from_str(raw).unwrap_or(Value::Null));
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&map).unwrap_or_default());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCK: &str = r#"
const cheerio = require("cheerio");
module.exports = {
  platform: "MockFM",
  version: "1.2.3",
  author: "tests",
  async search(query, page, type) {
    const $ = cheerio.load('<div class="song" data-id="42"><span class="t">' + query + '</span><b>Artist</b></div>');
    const data = [];
    $(".song").each((i, el) => {
      data.push({
        id: $(el).data("id"),
        title: $(el).find(".t").text(),
        artist: $(el).find("b").text(),
        album: "mock",
        duration: 3,
      });
    });
    return { isEnd: true, data };
  },
  async getMediaSource(item) {
    return { url: "http://example.invalid/" + item.id + ".mp3" };
  },
  async getLyric() { return { rawLrc: "[00:00]hello" }; },
};
"#;

    #[test]
    fn describes_plugin() {
        let mut ctx = Context::default();
        drop(ctx);
        let meta = describe(MOCK).unwrap();
        assert_eq!(meta.platform, "MockFM");
        assert_eq!(meta.version, "1.2.3");
        assert!(meta.methods.contains(&"search".to_string()));
        assert!(meta.methods.contains(&"getMediaSource".to_string()));
    }

    #[test]
    fn search_with_cheerio() {
        let out = call_method(MOCK, "search", &[
            Value::String("hey".into()),
            Value::from(1),
            Value::String("music".into()),
        ])
        .unwrap();
        assert_eq!(out["isEnd"], Value::Bool(true));
        assert_eq!(out["data"][0]["id"], Value::from(42));
        assert_eq!(out["data"][0]["title"], Value::from("hey"));
        assert_eq!(out["data"][0]["artist"], Value::from("Artist"));
    }

    #[test]
    fn media_source_and_lyric() {
        let out = call_method(
            MOCK,
            "getMediaSource",
            &[serde_json::json!({"id": 7}), Value::String("standard".into())],
        )
        .unwrap();
        assert_eq!(out["url"], Value::from("http://example.invalid/7.mp3"));

        let lrc = call_method(MOCK, "getLyric", &[serde_json::json!({"id": 7})]).unwrap();
        assert_eq!(lrc["rawLrc"], Value::from("[00:00]hello"));
    }
}
