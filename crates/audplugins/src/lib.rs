//! MusicFree 插件宿主。
//!
//! 插件标准与 <https://github.com/hhym/MusicFreePlugins> 保持一致：一个 `.js` 文件，
//! `module.exports = { platform, version, search(query, page, type), getMediaSource(item, quality), ... }`，
//! 运行时可 `require` 到 axios / cheerio / crypto-js / dayjs / qs / he / big-integer 等库。
//!
//! 实现方式：用 `boa_engine` 跑 JS，库由 `polyfill.js` 提供；HTTP、哈希、存储这些
//! 重活交给 Rust 原生函数（`__http` / `__hash` / `__storage`）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

mod cache;
mod mygo;
pub use mygo::{discover_mygo, MygoManifest, MygoPlugin, SettingDef};
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
        Plugin::load_using(path, &HashMap::new()).0
    }

    /// 带缓存加载：命中「路径 + 修改时间 + 大小」就不再执行插件代码。
    /// 返回 `(插件, 需要写回缓存的新条目)`。
    pub(crate) fn load_using(
        path: &Path,
        cache: &HashMap<String, cache::Entry>,
    ) -> (Plugin, Option<(String, cache::Entry)>) {
        let code = match std::fs::read_to_string(path) {
            Ok(code) => code,
            Err(err) => {
                return (
                    Plugin {
                        info: PluginInfo {
                            path: path.to_path_buf(),
                            error: Some(format!("读取失败: {err}")),
                            ..Default::default()
                        },
                        code: String::new(),
                    },
                    None,
                )
            }
        };

        let key = cache::key(path);
        if let Some(entry) = cache.get(&key) {
            return (
                Plugin {
                    info: PluginInfo {
                        path: path.to_path_buf(),
                        platform: entry.platform.clone(),
                        version: entry.version.clone(),
                        author: entry.author.clone(),
                        methods: entry.methods.clone(),
                        error: entry.error.clone(),
                    },
                    code,
                },
                None,
            );
        }

        let mut info = PluginInfo {
            path: path.to_path_buf(),
            ..Default::default()
        };
        match inspect(&code) {
            Ok(mut meta) => {
                if meta.platform.is_empty() {
                    meta.platform = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("插件")
                        .to_string();
                }
                info.platform = meta.platform;
                info.version = meta.version;
                info.author = meta.author;
                info.methods = meta.methods;
            }
            Err(err) => {
                // 至少让列表里显示文件名叫什么，而不是一个「?」
                info.platform = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("插件")
                    .to_string();
                info.error = Some(friendly_error(&err.to_string()));
            }
        }

        let entry = cache::Entry {
            platform: info.platform.clone(),
            version: info.version.clone(),
            author: info.author.clone(),
            methods: info.methods.clone(),
            error: info.error.clone(),
        };
        (Plugin { info, code }, Some((key, entry)))
    }

    /// 加载失败（超时等）时构造一个只带错误信息的占位插件。
    pub(crate) fn synthetic(path: &Path, error: String) -> Plugin {
        Plugin {
            info: PluginInfo {
                path: path.to_path_buf(),
                error: Some(error),
                ..Default::default()
            },
            code: String::new(),
        }
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

/// 把底层报错翻译成人能看懂的一句话。
pub fn friendly_error(raw: &str) -> String {
    if raw.contains("LoopIteration") || raw.to_ascii_lowercase().contains("loop iteration") {
        return "插件在加载阶段死循环（常见于混淆过的插件，校验循环在本引擎里跑不通），已跳过".into();
    }
    if raw.contains("declared in lexically declared names") {
        let line = raw
            .split("at line ")
            .nth(1)
            .and_then(|s| s.split([',', ' ']).next())
            .unwrap_or("?");
        return format!(
            "插件源码有语法错误（第 {line} 行）：同名变量在同一作用域里重复声明，该插件在原客户端也会报错"
        );
    }
    if raw.contains("unexpected token 'export'") || raw.contains("unexpected token \"export\"") {
        let line = raw
            .split("at line ")
            .nth(1)
            .and_then(|s| s.split([',', ' ']).next())
            .unwrap_or("?");
        return format!(
            "插件第 {line} 行还残留 `export`：宿主已改写常见 ES 模块写法，这里是一种更少见的导出形式。把该行附近 5 行发给插件作者，或在「插件」页点「重新扫描」再试"
        );
    }
    if raw.contains("SyntaxError") {
        let line = raw
            .split("at line ")
            .nth(1)
            .and_then(|s| s.split([',', ' ']).next())
            .unwrap_or("?");
        return format!("插件源码有语法错误（第 {line} 行），当前 JS 引擎无法解析");
    }
    raw.to_string()
}

/// 插件标准里约定的导出方法名。
const KNOWN_METHODS: &[&str] = &[
    "search",
    "signIn",
    "loginQr",
    "loginPoll",
    "getMediaSource",
    "getLyric",
    "getAlbumInfo",
    "getArtistWorks",
    "getTopLists",
    "getTopListDetail",
    "getRecommendSheetTags",
    "getRecommendSheetsByTag",
    "getMusicSheetInfo",
    "importMusicSheet",
    "getMusicComments",
];

/// 只做语法检查，不执行代码（boa 的解析器，毫秒级）。
fn syntax_check(code: &str) -> Result<(), String> {
    use boa_engine::parser::Parser;
    let mut parser = Parser::new(Source::from_bytes(code));
    let scope = boa_engine::ast::scope::Scope::new_global();
    let mut interner = boa_engine::interner::Interner::new();
    parser
        .parse_script(&scope, &mut interner)
        .map(|_| ())
        .map_err(|e| friendly_error(&format!("{e}")))?;
    Ok(())
}

/// 在源码里抓字段的值：优先看 `module.exports` 之后那段（导出对象一般在文件末尾），
/// 找不到再全文找最后一次出现。
fn capture(code: &str, pattern: &str) -> Option<String> {
    let re = regex::Regex::new(pattern).ok()?;
    let tail = code
        .rfind("module.exports")
        .map(|idx| &code[idx..])
        .unwrap_or(code);
    if let Some(value) = last_capture(&re, tail) {
        return Some(value);
    }
    last_capture(&re, code)
}

fn last_capture(re: &regex::Regex, text: &str) -> Option<String> {
    re.captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
        .filter(|s| !s.is_empty() && !s.contains('\n'))
        .last()
}

/// 源码里直接读 platform / version / author / 方法名。
/// 混淆插件顶层有很重的解码循环，执行一遍要好几秒，扫描时没必要跑。
fn static_meta(code: &str) -> Option<Meta> {
    let platform = capture(
        code,
        r#"(?:^|[^A-Za-z0-9_$])["']?platform["']?\s*:\s*["']([^"'
]{1,80})["']"#,
    )?;
    let version = capture(
        code,
        r#"(?:^|[^A-Za-z0-9_$])["']?version["']?\s*:\s*["']([^"'
]{0,80})["']"#,
    )
    .unwrap_or_default();
    let author = capture(
        code,
        r#"(?:^|[^A-Za-z0-9_$])["']?author["']?\s*:\s*["']([^"'
]{0,80})["']"#,
    )
    .unwrap_or_default();

    let methods = KNOWN_METHODS
        .iter()
        .filter(|name| {
            let pattern = format!(
                r#"(?:^|[^A-Za-z0-9_$])["']?{name}["']?\s*[:(]"#,
                name = regex::escape(name)
            );
            regex::Regex::new(&pattern)
                .map(|re| re.is_match(code))
                .unwrap_or(false)
        })
        .map(|s| s.to_string())
        .collect();

    Some(Meta {
        platform,
        version,
        author,
        methods,
    })
}

/// 加载时的元信息：只做语法检查 + 源码静态读取，**不执行插件代码**。
///
/// 混淆过的插件把 platform 藏在解码后的字符串里，静态读不到，就先留空、
/// 按「默认支持搜索/取地址/歌词」处理，等到真的搜索时再执行。
fn inspect(code: &str) -> anyhow::Result<Meta> {
    syntax_check(code).map_err(|e| anyhow::anyhow!(e))?;
    Ok(static_meta(code).unwrap_or_else(|| Meta {
        platform: String::new(),
        version: String::new(),
        author: String::new(),
        methods: KNOWN_METHODS
            .iter()
            .take(3)
            .map(|s| s.to_string())
            .collect(),
    }))
}

/// 跑一遍插件代码，把导出的元信息和方法名读出来（静态读不出来时的兜底）。
#[allow(dead_code)]
fn describe(code: &str) -> anyhow::Result<Meta> {
    let mut ctx = new_context_with_limit(LOAD_LOOP_LIMIT);
    eval_plugin(&mut ctx, code).map_err(|e| anyhow::anyhow!(friendly_error(&e.to_string())))?;

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
    invoke_exports(&mut ctx, exports, method, args)
}

/// 在已经准备好的上下文里调用 `exports[method](...args)`，返回 JSON。
pub(crate) fn invoke_exports(
    ctx: &mut Context,
    exports: JsValue,
    method: &str,
    args: &[Value],
) -> anyhow::Result<Value> {
    invoke_exports_with(ctx, exports, method, args, std::time::Duration::from_secs(8))
}

/// 同 [`invoke_exports`]，但可以给更长的预算：扫码登录这类要等用户操作的方法需要。
pub(crate) fn invoke_exports_with(
    mut ctx: &mut Context,
    exports: JsValue,
    method: &str,
    args: &[Value],
    budget: std::time::Duration,
) -> anyhow::Result<Value> {
    let args_value = JsValue::from_json(&Value::Array(args.to_vec()), ctx)
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

    // 把微任务队列跑干净；宿主 HTTP 是同步的，通常一两轮就结束。
    // 插件也可能在等网络或等用户扫码，这里按预算一直转，预算用完再判超时。
    let deadline = std::time::Instant::now() + budget;
    loop {
        ctx.run_jobs().ok();
        if global_bool(&mut ctx, "__invokeDone") {
            break;
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "{method} 在 {} 秒内没有返回结果（可能还在等网络响应或用户操作）",
                budget.as_secs()
            );
        }
        // MyGo 插件可能有 setTimeout，顺手推进一下
        if global_bool(&mut ctx, "__hasTimers") {
            let _ = ctx.eval(Source::from_bytes("__runTimers();"));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    if let Some(err) = global_string(&mut ctx, "__invokeError") {
        anyhow::bail!("{err}");
    }
    let result = global_string(&mut ctx, "__invokeResult")
        .ok_or_else(|| anyhow::anyhow!("{method} 没有返回结果"))?;
    Ok(serde_json::from_str(&result)?)
}

pub(crate) fn global_bool(ctx: &mut Context, key: &str) -> bool {
    ctx.global_object()
        .get(js_string!(key), ctx)
        .map(|v| v.as_boolean().unwrap_or(false))
        .unwrap_or(false)
}

pub(crate) fn global_string(ctx: &mut Context, key: &str) -> Option<String> {
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

/// 普通调用（搜索/取地址）允许的循环次数上限，防止插件里的死循环卡住界面。
const RUNTIME_LOOP_LIMIT: u64 = 30_000_000;
/// 只加载插件读元信息时，循环上限收紧，混淆代码的校验循环会在这里被拦下。
const LOAD_LOOP_LIMIT: u64 = 2_000_000;

pub(crate) fn new_context() -> Context {
    new_context_with_limit(RUNTIME_LOOP_LIMIT)
}

pub(crate) fn new_context_with_limit(loop_limit: u64) -> Context {
    let mut ctx = Context::default();
    ctx.runtime_limits_mut().set_loop_iteration_limit(loop_limit);

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
        js_string!("__aes"),
        5,
        NativeFunction::from_fn_ptr(native_aes),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__storage"),
        2,
        NativeFunction::from_fn_ptr(native_storage),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__open"),
        1,
        NativeFunction::from_fn_ptr(native_open),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__log"),
        1,
        NativeFunction::from_fn_ptr(native_log),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__signInPrompt"),
        1,
        NativeFunction::from_fn_ptr(native_sign_in_prompt),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__spawn"),
        2,
        NativeFunction::from_fn_ptr(native_spawn),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__spawnWrite"),
        2,
        NativeFunction::from_fn_ptr(native_spawn_write),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__spawnReadLine"),
        2,
        NativeFunction::from_fn_ptr(native_spawn_read_line),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__spawnKill"),
        1,
        NativeFunction::from_fn_ptr(native_spawn_kill),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__spawnCloseStdin"),
        1,
        NativeFunction::from_fn_ptr(native_spawn_close_stdin),
    )
    .ok();
    ctx.register_global_callable(
        js_string!("__spawnWait"),
        1,
        NativeFunction::from_fn_ptr(native_spawn_wait),
    )
    .ok();

    if let Err(err) = ctx.eval(Source::from_bytes(include_str!("polyfill.js"))) {
        log::error!("插件运行时初始化失败: {err}");
    }
    ctx
}

thread_local! {
    /// 当前线程里插件允许启动的程序（插件目录 + permissions.process 里的模式）。
    static PROCESS_POLICY: std::cell::RefCell<(std::path::PathBuf, Vec<String>)> =
        std::cell::RefCell::new((std::path::PathBuf::new(), Vec::new()));
    /// 登录提示的接收端（插件调 host.signInPrompt 时把二维码推给界面）。
    static SIGN_IN_SINK: std::cell::RefCell<Option<std::sync::mpsc::Sender<serde_json::Value>>> =
        std::cell::RefCell::new(None);
}

/// 设置本线程执行插件代码时允许启动的程序。
pub fn set_process_policy(dir: std::path::PathBuf, patterns: Vec<String>) {
    PROCESS_POLICY.with(|slot| *slot.borrow_mut() = (dir, patterns));
}

/// 设置本线程登录提示的接收端（`None` 表示丢弃）。
pub fn set_sign_in_sink(sink: Option<std::sync::mpsc::Sender<serde_json::Value>>) {
    SIGN_IN_SINK.with(|slot| *slot.borrow_mut() = sink);
}

/// 简单通配符匹配（`*` 匹配任意字符）。
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.replace('\\', "/").to_lowercase();
    let text = text.replace('\\', "/").to_lowercase();
    let pb: Vec<char> = pattern.chars().collect();
    let tb: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut resume = 0usize;
    while ti < tb.len() {
        if pi < pb.len() && (pb[pi] == tb[ti] || pb[pi] == '?') {
            pi += 1;
            ti += 1;
        } else if pi < pb.len() && pb[pi] == '*' {
            star = Some(pi);
            resume = ti;
            pi += 1;
        } else if let Some(star_pos) = star {
            pi = star_pos + 1;
            resume += 1;
            ti = resume;
        } else {
            return false;
        }
    }
    while pi < pb.len() && pb[pi] == '*' {
        pi += 1;
    }
    pi == pb.len()
}

/// 把一段文字包成 JS 异常（boa 的 JsError 只从 JsNativeError 转换）。
fn js_err(msg: impl Into<String>) -> boa_engine::JsError {
    let text = msg.into();
    boa_engine::JsNativeError::typ()
        .with_message(text)
        .into()
}

fn js_arg_string(args: &[boa_engine::JsValue], index: usize) -> String {
    args.get(index)
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default()
}

fn native_sign_in_prompt(
    _this: &boa_engine::JsValue,
    args: &[boa_engine::JsValue],
    _ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<boa_engine::JsValue> {
    let raw = js_arg_string(args, 0);
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    SIGN_IN_SINK.with(|slot| {
        if let Some(tx) = slot.borrow().as_ref() {
            let _ = tx.send(value);
        }
    });
    Ok(boa_engine::JsValue::undefined())
}

struct ProcHandle {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    reader: std::io::BufReader<std::process::ChildStdout>,
}

fn process_registry() -> &'static std::sync::Mutex<std::collections::HashMap<u32, ProcHandle>> {
    static REGISTRY: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<u32, ProcHandle>>,
    > = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn static_counter() -> u32 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

fn resolve_program(dir: &std::path::Path, patterns: &[String], command: &str) -> Option<std::path::PathBuf> {
    let looks_like_path = command.contains('/') || command.contains('\\') || command.contains('.');
    if looks_like_path {
        let candidate = if std::path::Path::new(command).is_absolute() {
            std::path::PathBuf::from(command)
        } else {
            dir.join(command)
        };
        let canonical = candidate.canonicalize().ok()?;
        if !canonical.starts_with(dir.canonicalize().ok()?) {
            return None;
        }
        let rel = canonical.strip_prefix(dir.canonicalize().ok()?).ok()?;
        let rel_text = rel.to_string_lossy().to_string();
        let allowed = patterns.iter().any(|p| {
            glob_match(p, &rel_text)
                || glob_match(p, &format!("./{rel_text}"))
                || glob_match(p, &canonical.to_string_lossy())
        });
        if allowed && canonical.is_file() {
            return Some(canonical);
        }
        return None;
    }
    let allowed = patterns
        .iter()
        .any(|p| glob_match(p, command) || p == command);
    if allowed {
        Some(std::path::PathBuf::from(command))
    } else {
        None
    }
}

fn native_spawn(
    _this: &boa_engine::JsValue,
    args: &[boa_engine::JsValue],
    _ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<boa_engine::JsValue> {
    use std::process::{Command, Stdio};
    let raw = js_arg_string(args, 0);
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    let command = value.get("command").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let list = value
        .get("args")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let (dir, patterns) = PROCESS_POLICY.with(|slot| slot.borrow().clone());
    if dir.as_os_str().is_empty() {
        return Err(js_err("host.spawn: 没有插件上下文"));
    }
    let Some(program) = resolve_program(&dir, &patterns, &command) else {
        return Err(js_err(format!("host.spawn: 插件没有启动 {command} 的权限")));
    };
    let mut cmd = Command::new(program);
    for arg in list {
        cmd.arg(arg.as_str().unwrap_or_default());
    }
    let child = cmd
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| js_err(format!("host.spawn: {e}")))?;
    let mut child = child;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let id = static_counter();
    // stderr 单独抽干，避免管道写满把子进程卡住
    if let Some(mut err) = stderr {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            if !buf.is_empty() {
                let text = String::from_utf8_lossy(&buf).to_string();
                for line in text.lines().take(8) {
                    log::warn!("插件子进程 stderr: {line}");
                }
            }
        });
    }
    let handle = ProcHandle {
        child,
        stdin,
        reader: std::io::BufReader::new(stdout.expect("stdout piped")),
    };
    process_registry().lock().unwrap().insert(id, handle);
    Ok(boa_engine::JsValue::from(id as f64))
}

fn native_spawn_write(
    _this: &boa_engine::JsValue,
    args: &[boa_engine::JsValue],
    _ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<boa_engine::JsValue> {
    use std::io::Write;
    let id = args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
    let text = js_arg_string(args, 1);
    let mut map = process_registry().lock().unwrap();
    if let Some(handle) = map.get_mut(&id) {
        if let Some(stdin) = handle.stdin.as_mut() {
            stdin
                .write_all(text.as_bytes())
                .and_then(|_| stdin.flush())
                .map_err(|e| js_err(format!("host.spawn.write: {e}")))?;
        }
    }
    Ok(boa_engine::JsValue::undefined())
}

fn native_spawn_read_line(
    _this: &boa_engine::JsValue,
    args: &[boa_engine::JsValue],
    _ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<boa_engine::JsValue> {
    use std::io::BufRead;
    let id = args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
    let mut line = String::new();
    let read = {
        let mut map = process_registry().lock().unwrap();
        match map.get_mut(&id) {
            Some(handle) => handle.reader.read_line(&mut line),
            None => {
                return Err(js_err("host.spawn.readLine: 进程不存在"))
            }
        }
    };
    match read {
        Ok(0) => Err(js_err("host.spawn.readLine: 进程已退出")),
        Ok(_) => Ok(boa_engine::JsValue::from(boa_engine::JsString::from(
            line.trim_end_matches(['\n', '\r']).to_string(),
        ))),
        Err(e) => Err(js_err(format!("host.spawn.readLine: {e}"))),
    }
}

fn native_spawn_kill(
    _this: &boa_engine::JsValue,
    args: &[boa_engine::JsValue],
    _ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<boa_engine::JsValue> {
    let id = args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
    if let Some(mut handle) = process_registry().lock().unwrap().remove(&id) {
        let _ = handle.child.kill();
    }
    Ok(boa_engine::JsValue::undefined())
}

fn native_spawn_close_stdin(
    _this: &boa_engine::JsValue,
    args: &[boa_engine::JsValue],
    _ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<boa_engine::JsValue> {
    let id = args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
    if let Some(handle) = process_registry().lock().unwrap().get_mut(&id) {
        handle.stdin.take();
    }
    Ok(boa_engine::JsValue::undefined())
}

fn native_spawn_wait(
    _this: &boa_engine::JsValue,
    args: &[boa_engine::JsValue],
    _ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<boa_engine::JsValue> {
    let id = args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
    let mut map = process_registry().lock().unwrap();
    if let Some(handle) = map.get_mut(&id) {
        match handle.child.wait() {
            Ok(status) => Ok(boa_engine::JsValue::from(status.code().unwrap_or(-1) as f64)),
            Err(_) => Ok(boa_engine::JsValue::from(-1.0)),
        }
    } else {
        Ok(boa_engine::JsValue::from(-1.0))
    }
}

pub(crate) fn platform_name() -> &'static str {
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
    let headers = remembered_headers(url);
    fetch_to_cache_with(url, &headers)
}

/// 记住某个播放地址该带的 headers，下载时自动用上。
pub fn remember_headers(url: &str, headers: &[(String, String)]) {
    if headers.is_empty() {
        return;
    }
    let map = header_registry();
    if let Ok(mut map) = map.lock() {
        map.insert(url.to_string(), headers.to_vec());
    }
}

fn remembered_headers(url: &str) -> Vec<(String, String)> {
    header_registry()
        .lock()
        .ok()
        .and_then(|map| map.get(url).cloned())
        .unwrap_or_default()
}

fn header_registry() -> &'static std::sync::Mutex<HashMap<String, Vec<(String, String)>>> {
    static REGISTRY: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Vec<(String, String)>>>> =
        std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 带 headers 下载到缓存；同样的 url + headers 命中同一个文件。
pub fn fetch_to_cache_with(url: &str, headers: &[(String, String)]) -> anyhow::Result<PathBuf> {
    use std::io::Write;

    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(audcore::APP_NAME)
        .join("streams");
    std::fs::create_dir_all(&dir)?;

    let hash = format!("{:x}", {
        use md5::{Digest, Md5};
        let mut key = url.to_string();
        for (name, value) in headers {
            key.push('\n');
            key.push_str(name);
            key.push(':');
            key.push_str(value);
        }
        Md5::digest(key.as_bytes())
    });
    let ext = url_extension(url);
    let path = dir.join(format!("{hash}.{ext}"));
    if path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(path);
    }

    let mut request = client().get(url);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let resp = request.send()?.error_for_status()?;
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
    if lower.contains(".m4s") {
        return "m4a";
    }
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

fn native_aes(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let op = args.first().and_then(|v| v.as_string().map(|s| s.to_std_string_escaped())).unwrap_or_default();
    let data = args.get(1).and_then(|v| v.as_string().map(|s| s.to_std_string_escaped())).unwrap_or_default();
    let key = args.get(2).and_then(|v| v.as_string().map(|s| s.to_std_string_escaped())).unwrap_or_default();
    let iv = args.get(3).and_then(|v| v.as_string().map(|s| s.to_std_string_escaped())).unwrap_or_default();
    let padding = args.get(4).and_then(|v| v.as_string().map(|s| s.to_std_string_escaped())).unwrap_or_default();
    Ok(JsValue::from(JsString::from(aes_crypt(
        &op, &data, &key, &iv, &padding,
    ))))
}

fn native_open(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let url = args
        .first()
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_default();
    if !url.is_empty() {
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(&url).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
    Ok(JsValue::undefined())
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
/// 给「扫码登录」用：直接下载一张图片（二维码），返回原始字节。
pub fn fetch_bytes(url: &str) -> anyhow::Result<Vec<u8>> {
    let resp = client()
        .get(url)
        .send()
        .map_err(|e| anyhow::anyhow!("下载图片失败: {e}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("下载图片失败: HTTP {}", resp.status());
    }
    let bytes = resp.bytes().map_err(|e| anyhow::anyhow!("读取图片失败: {e}"))?;
    if bytes.len() > 4 * 1024 * 1024 {
        anyhow::bail!("图片太大（{} 字节）", bytes.len());
    }
    Ok(bytes.to_vec())
}

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
            // 同名头（尤其 Set-Cookie）可能有多条，用换行拼起来，JS 侧 getSetCookie() 再拆开
            let mut headers = serde_json::Map::new();
            for (k, v) in resp.headers().iter() {
                let key = k.as_str().to_string();
                let text = v.to_str().unwrap_or_default().to_string();
                let entry = headers
                    .entry(key)
                    .or_insert_with(|| Value::String(String::new()));
                if let Value::String(existing) = entry {
                    if !existing.is_empty() {
                        existing.push('\n');
                    }
                    existing.push_str(&text);
                }
            }
            // 同时给一份 UTF-8 文本和一份 base64：插件用 text()/json() 走前者，
            // arrayBuffer() 走后者，二进制内容（图片、音频）才不会被当成文本截坏。
            let bytes = resp.bytes().unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes).to_string();
            let b64_body = if bytes.len() <= 8 * 1024 * 1024 {
                b64(bytes.to_vec())
            } else {
                String::new()
            };
            serde_json::json!({
                "status": status,
                "headers": headers,
                "body": text,
                "body_b64": b64_body,
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


/// 把 4 种组合（CBC/ECB × 加密/解密 × PKCS7/无填充）收敛到一个宏里。
macro_rules! aes_run {
    ($ty:ty, $data:expr, $key:expr, $iv:expr, $enc:expr, $pad:expr) => {{
        use aes::cipher::block_padding::{NoPadding, Pkcs7};
        use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyInit, KeyIvInit};

        let data: Vec<u8> = $data;
        let key: Vec<u8> = $key;
        let iv: Vec<u8> = $iv;
        let enc: bool = $enc;
        let pad: bool = $pad;

        let run: Result<Vec<u8>, String> = if enc {
            let mut buf = data.clone();
            if pad {
                buf.resize(data.len() + 16 - (data.len() % 16) + 16, 0);
            }
            if iv.is_empty() {
                let cipher = match ecb::Encryptor::<$ty>::new_from_slice(&key) {
                    Ok(cipher) => cipher,
                    Err(err) => return serde_json::json!({ "error": format!("{err:?}") }).to_string(),
                };
                let out = if pad {
                    cipher.encrypt_padded_mut::<Pkcs7>(&mut buf, data.len())
                } else {
                    cipher.encrypt_padded_mut::<NoPadding>(&mut buf, data.len())
                };
                out.map(|b| b.to_vec()).map_err(|e| format!("{e:?}"))
            } else {
                let cipher = match cbc::Encryptor::<$ty>::new_from_slices(&key, &iv) {
                    Ok(cipher) => cipher,
                    Err(err) => return serde_json::json!({ "error": format!("{err:?}") }).to_string(),
                };
                let out = if pad {
                    cipher.encrypt_padded_mut::<Pkcs7>(&mut buf, data.len())
                } else {
                    cipher.encrypt_padded_mut::<NoPadding>(&mut buf, data.len())
                };
                out.map(|b| b.to_vec()).map_err(|e| format!("{e:?}"))
            }
        } else {
            let mut buf = data.clone();
            if iv.is_empty() {
                let cipher = match ecb::Decryptor::<$ty>::new_from_slice(&key) {
                    Ok(cipher) => cipher,
                    Err(err) => return serde_json::json!({ "error": format!("{err:?}") }).to_string(),
                };
                let out = if pad {
                    cipher.decrypt_padded_mut::<Pkcs7>(&mut buf)
                } else {
                    cipher.decrypt_padded_mut::<NoPadding>(&mut buf)
                };
                out.map(|b| b.to_vec()).map_err(|e| format!("{e:?}"))
            } else {
                let cipher = match cbc::Decryptor::<$ty>::new_from_slices(&key, &iv) {
                    Ok(cipher) => cipher,
                    Err(err) => return serde_json::json!({ "error": format!("{err:?}") }).to_string(),
                };
                let out = if pad {
                    cipher.decrypt_padded_mut::<Pkcs7>(&mut buf)
                } else {
                    cipher.decrypt_padded_mut::<NoPadding>(&mut buf)
                };
                out.map(|b| b.to_vec()).map_err(|e| format!("{e:?}"))
            }
        };

        run
    }};
}

/// AES-128/192/256 的 CBC / ECB，输入输出都是 base64。
fn aes_crypt(op: &str, data_b64: &str, key_b64: &str, iv_b64: &str, padding: &str) -> String {
    use base64::Engine;

    let engine = base64::engine::general_purpose::STANDARD;
    let data = match engine.decode(data_b64) {
        Ok(bytes) => bytes,
        Err(err) => return serde_json::json!({ "error": format!("数据不是合法 base64: {err}") }).to_string(),
    };
    let key = match engine.decode(key_b64) {
        Ok(bytes) => bytes,
        Err(err) => return serde_json::json!({ "error": format!("密钥不是合法 base64: {err}") }).to_string(),
    };
    let iv = engine.decode(iv_b64).unwrap_or_default();
    let encrypting = op == "encrypt";
    let use_pkcs7 = padding.is_empty() || padding == "pkcs7";

    let result = match key.len() {
        16 => aes_run!(aes::Aes128, data, key, iv, encrypting, use_pkcs7),
        24 => aes_run!(aes::Aes192, data, key, iv, encrypting, use_pkcs7),
        32 => aes_run!(aes::Aes256, data, key, iv, encrypting, use_pkcs7),
        other => Err(format!("密钥长度 {other} 字节，AES 只支持 16/24/32")),
    };

    match result {
        Ok(bytes) => serde_json::json!({ "data": engine.encode(bytes) }).to_string(),
        Err(err) => serde_json::json!({ "error": err }).to_string(),
    }
}

fn storage_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(audcore::APP_NAME)
        .join("plugin-storage.json")
}

/// 删除插件时可选调用：清掉 `plugin-storage.json` 里属于它的键，
/// 以及 MyGo 的 `mygo-data/<id>`（歌单缓存、登录后拉的数据）。
pub fn purge_plugin_data(id: &str) {
    if id.trim().is_empty() {
        return;
    }
    let path = storage_path();
    let mut map: serde_json::Map<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let prefixes = [
        format!("mygo/{id}/"),
        format!("mygo/{id}"),
        format!("{id}/"),
        format!("musicfree/{id}/"),
    ];
    let before = map.len();
    map.retain(|k, _| !prefixes.iter().any(|p| k.starts_with(p.as_str())));
    if map.len() != before {
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&map).unwrap_or_default());
    }

    let data_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(audcore::APP_NAME)
        .join("mygo-data")
        .join(id);
    if data_dir.is_dir() {
        let _ = std::fs::remove_dir_all(&data_dir);
    }
}

pub(crate) fn storage_access(key: &str, value: Option<&str>) -> String {
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

// ------------------------------------------------------------------ 统一门面

/// 插件给的一条播放地址：`url` + 随请求带的 headers。
#[derive(Clone, Debug, Default)]
pub struct MediaSource {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// 插件返回的 headers 可能是对象（`{"Referer": "..."}`），也可能是数组
/// （`[{name, value}]`），两种都收。
fn collect_headers(value: &Value) -> Vec<(String, String)> {
    let Some(headers) = value.get("headers") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(map) = headers.as_object() {
        for (key, value) in map {
            if let Some(text) = value.as_str() {
                out.push((key.clone(), text.to_string()));
            }
        }
    } else if let Some(list) = headers.as_array() {
        for item in list {
            let key = item["name"]
                .as_str()
                .or(item["key"].as_str())
                .unwrap_or_default();
            let value = item["value"].as_str().unwrap_or_default();
            if !key.is_empty() {
                out.push((key.to_string(), value.to_string()));
            }
        }
    }
    out
}

/// 首页里的一段（一个横排）。两种插件标准都归一到这里。
#[derive(Clone, Debug, Default)]
pub struct HomeSection {
    pub title: String,
    /// 可以直接播放的歌曲（已归一成 MusicFree 的字段）
    pub tracks: Vec<Value>,
    /// 歌单 / 专辑 / 艺人这类卡片，暂只展示标题
    pub cards: Vec<Value>,
}

/// 首页头部信息（MyGo 的 Browse 带标题与简介，MusicFree 只有若干段）
#[derive(Clone, Debug, Default)]
pub struct HomePage {
    pub title: String,
    pub subtitle: String,
    pub sections: Vec<HomeSection>,
}

/// 两种插件标准，界面上统一成一份列表。
#[derive(Clone)]
pub enum OnlinePlugin {
    /// MusicFree：单个 `.js` 文件
    MusicFree(Plugin),
    /// MyGo：一个目录 + `plugin.json`
    Mygo(MygoPlugin),
}

impl OnlinePlugin {
    /// 界面上显示的名字，带标准后缀。
    pub fn name(&self) -> String {
        match self {
            OnlinePlugin::MusicFree(p) => format!("{} · MusicFree", p.info.platform),
            OnlinePlugin::Mygo(p) => p.display_name(),
        }
    }

    pub fn version(&self) -> &str {
        match self {
            OnlinePlugin::MusicFree(p) => &p.info.version,
            OnlinePlugin::Mygo(p) => &p.info.version,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            OnlinePlugin::MusicFree(p) => p.info.error.as_deref(),
            OnlinePlugin::Mygo(p) => p.info.error.as_deref(),
        }
    }

    /// 两种标准的"能做什么"，用统一名字回答。
    pub fn supports(&self, method: &str) -> bool {
        match self {
            OnlinePlugin::MusicFree(p) => p.info.supports(method),
            OnlinePlugin::Mygo(p) => match method {
                "search" => p.info.supports("search"),
                "getMediaSource" => p.info.supports("stream"),
                "getLyric" => p.info.supports("lyrics"),
                other => p.info.supports(other),
            },
        }
    }

    /// 搜索，返回统一形状的条目数组。
    pub fn search_tracks(&self, query: &str, page: u32) -> anyhow::Result<Vec<Value>> {
        match self {
            OnlinePlugin::MusicFree(p) => {
                let out = p.search(query, page, "music")?;
                Ok(out["data"].as_array().cloned().unwrap_or_default())
            }
            OnlinePlugin::Mygo(p) => p.search_tracks(query, ((page.max(1) - 1) as u64) * 20),
        }
    }

    /// 取播放地址。
    pub fn media_url(&self, item: &Value, quality: &str) -> anyhow::Result<String> {
        Ok(self.media(item, quality)?.url)
    }

    /// 取播放地址 + 插件要求随请求带的 headers（Referer / User-Agent / Cookie 等）。
    pub fn media(&self, item: &Value, quality: &str) -> anyhow::Result<MediaSource> {
        match self {
            OnlinePlugin::MusicFree(p) => {
                let v = p.media_source(item, quality)?;
                let pick = |x: &Value| {
                    x.as_str()
                        .filter(|s| s.starts_with("http"))
                        .map(|s| s.to_string())
                        .or_else(|| {
                            x["url"]
                                .as_str()
                                .filter(|s| s.starts_with("http"))
                                .map(|s| s.to_string())
                        })
                };
                let url = pick(&v)
                    .or_else(|| pick(&v["data"]))
                    .ok_or_else(|| anyhow::anyhow!("插件没有返回可播放的地址"))?;
                let mut headers: Vec<(String, String)> = collect_headers(&v)
                    .into_iter()
                    .chain(collect_headers(&v["data"]))
                    .collect();
                if !headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
                {
                    headers.push((
                        "User-Agent".into(),
                        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36".into(),
                    ));
                }
                Ok(MediaSource { url, headers })
            }
            OnlinePlugin::Mygo(p) => {
                let v = p.stream(item, quality)?;
                let url = v["url"]
                    .as_str()
                    .filter(|s| s.starts_with("http"))
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("stream 没有返回 url"))?;
                let mut headers: Vec<(String, String)> = collect_headers(&v);
                if !headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
                {
                    headers.push((
                        "User-Agent".into(),
                        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36".into(),
                    ));
                }
                Ok(MediaSource { url, headers })
            }
        }
    }

    /// 歌词（界面暂未接面板，先留接口）。
    pub fn lyric(&self, item: &Value) -> anyhow::Result<Value> {
        match self {
            OnlinePlugin::MusicFree(p) => p.lyric(item),
            OnlinePlugin::Mygo(p) => p.lyrics(item),
        }
    }

    /// 这个插件有没有首页接口。
    pub fn supports_home(&self) -> bool {
        match self {
            OnlinePlugin::Mygo(plugin) => plugin.info.supports("home"),
            OnlinePlugin::MusicFree(plugin) => plugin.info.supports("getTopLists"),
        }
    }

    /// 首页：MyGo 调 `home()` 拿 Browse；MusicFree 用 `getTopLists()` 当「推荐歌单」。
    pub fn home(&self) -> anyhow::Result<HomePage> {
        match self {
            OnlinePlugin::Mygo(plugin) => {
                if !plugin.info.supports("home") {
                    return Ok(HomePage::default());
                }
                let value = plugin.call("home", Value::Null)?;
                Ok(mygo_home_page(&value))
            }
            OnlinePlugin::MusicFree(plugin) => {
                if !plugin.info.supports("getTopLists") {
                    return Ok(HomePage::default());
                }
                let value = plugin.call("getTopLists", &[])?;
                Ok(musicfree_home_page(&value))
            }
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            OnlinePlugin::MusicFree(p) => &p.info.path,
            OnlinePlugin::Mygo(p) => &p.info.path,
        }
    }

    /// 简短的方法列表，给命令行探针用。
    pub fn methods_brief(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for m in ["search", "getMediaSource", "getLyric"] {
            if self.supports(m) {
                out.push(match m {
                    "getMediaSource" => "media",
                    "getLyric" => "lyric",
                    other => other,
                });
            }
        }
        out
    }

    /// MyGo 插件才有：设置项与读写。
    pub fn as_mygo(&self) -> Option<&MygoPlugin> {
        match self {
            OnlinePlugin::Mygo(p) => Some(p),
            _ => None,
        }
    }
}

/// MyGo 的 `home()` 返回 Browse：`{ title, subtitle, sections: [{ title, tracks, playlists, albums }] }`
fn mygo_home_page(value: &Value) -> HomePage {
    let mut page = HomePage {
        title: value["title"].as_str().unwrap_or("").to_string(),
        subtitle: value["subtitle"].as_str().unwrap_or("").to_string(),
        sections: Vec::new(),
    };
    let Some(sections) = value["sections"].as_array() else {
        return page;
    };
    for section in sections {
        let title = section["title"].as_str().unwrap_or("").to_string();
        let tracks = section["tracks"]
            .as_array()
            .map(|tracks| tracks.iter().map(mygo::track_item).collect())
            .unwrap_or_default();
        let mut cards = Vec::new();
        if let Some(playlists) = section["playlists"].as_array() {
            for playlist in playlists {
                cards.push(serde_json::json!({
                    "title": playlist["name"].as_str().or(playlist["title"].as_str()).unwrap_or("歌单"),
                    "cover": playlist["imageUrl"].as_str().or(playlist["coverUrl"].as_str()).unwrap_or(""),
                    "id": playlist["id"].clone(),
                }));
            }
        }
        if let Some(albums) = section["albums"].as_array() {
            for album in albums {
                cards.push(serde_json::json!({
                    "title": album["name"].as_str().unwrap_or("专辑"),
                    "cover": album["imageUrl"].as_str().unwrap_or(""),
                    "id": album["id"].clone(),
                }));
            }
        }
        page.sections.push(HomeSection { title, tracks, cards });
    }
    page
}

/// MusicFree 的 `getTopLists()`：`[{ title, data: [{ id, title, coverImg }] }]`
fn musicfree_home_page(value: &Value) -> HomePage {
    let lists = value
        .as_array()
        .cloned()
        .or_else(|| value["data"].as_array().cloned())
        .unwrap_or_default();
    let mut page = HomePage::default();
    for list in lists {
        let title = list["title"].as_str().unwrap_or("推荐").to_string();
        let cards: Vec<Value> = list["data"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        serde_json::json!({
                            "title": item["title"].as_str().unwrap_or("歌单"),
                            "cover": item["coverImg"].as_str().or(item["artwork"].as_str()).unwrap_or(""),
                            "id": item["id"].clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if cards.is_empty() {
            continue;
        }
        page.sections.push(HomeSection { title, tracks: Vec::new(), cards });
    }
    page
}

/// 扫描候选：根目录的 `.js`、含 `plugin.json` 的子目录。
fn candidates(dir: &Path) -> Vec<Candidate> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("js") {
            out.push(Candidate::MusicFree(path));
        } else if path.is_dir() && path.join("plugin.json").is_file() {
            out.push(Candidate::Mygo(path));
        }
    }
    out
}

#[derive(Clone)]
enum Candidate {
    MusicFree(PathBuf),
    Mygo(PathBuf),
}

impl Candidate {
    fn path(&self) -> &Path {
        match self {
            Candidate::MusicFree(p) | Candidate::Mygo(p) => p,
        }
    }
}

/// 后台扫描插件目录，返回一个 receiver；拿到结果后 UI 再替换列表。
///
/// 每个插件在独立线程里加载，超过 4 秒没出来就当作「卡死」跳过 ——
/// 插件顶层写死循环也不会把整个窗口拖成「无响应」。
pub fn scan_async(dir: PathBuf) -> Receiver<Vec<OnlinePlugin>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
        let cache = cache::load_all();
        let mut updates: HashMap<String, cache::Entry> = HashMap::new();
        let mut out = Vec::new();

        for cand in candidates(&dir) {
            let snapshot = cache.clone();
            let (tx2, rx2) = std::sync::mpsc::channel();
            let worker = cand.clone();
            // 混淆过的大插件（如 QQ Music 0.3.0 的 dist）解析时递归很深，
            // 默认 2MB 线程栈会直接 overflow，这里给足 16MB。
            std::thread::Builder::new()
                .stack_size(16 * 1024 * 1024)
                .spawn(move || {
                let (plugin, update) = match worker {
                    Candidate::MusicFree(path) => {
                        let (plugin, update) = Plugin::load_using(&path, &snapshot);
                        (OnlinePlugin::MusicFree(plugin), update)
                    }
                    Candidate::Mygo(path) => {
                        let (plugin, update) = MygoPlugin::load_using(&path, &snapshot);
                        (OnlinePlugin::Mygo(plugin), update)
                    }
                };
                let _ = tx2.send((plugin, update));
            })
            .expect("spawn plugin scan worker");

            match rx2.recv_timeout(Duration::from_secs(4)) {
                Ok((plugin, update)) => {
                    if let Some((key, entry)) = update {
                        updates.insert(key, entry);
                    }
                    out.push(plugin);
                }
                Err(_) => {
                    let path = cand.path();
                    log::warn!("插件加载超时: {}", path.display());
                    let error = "扫描超时：插件顶层代码跑了超过 4 秒（可能死循环），已跳过".to_string();
                    out.push(match cand {
                        Candidate::MusicFree(p) => OnlinePlugin::MusicFree(Plugin::synthetic(&p, error)),
                        Candidate::Mygo(p) => OnlinePlugin::Mygo(MygoPlugin::synthetic(&p, error)),
                    });
                }
            }
        }

        let mut merged = cache;
        for (k, v) in updates {
            merged.insert(k, v);
        }
        cache::save(&merged);

        out.sort_by_key(|p| p.name());
        let _ = tx.send(out);
    })
        .expect("spawn plugin scanner");
    rx
}

/// 扫描目录：根目录的 `.js` 当 MusicFree 插件，含 `plugin.json` 的子目录当 MyGo 插件。
pub fn discover_all(dir: &Path) -> Vec<OnlinePlugin> {
    let mut out: Vec<OnlinePlugin> = discover(dir).into_iter().map(OnlinePlugin::MusicFree).collect();
    out.extend(discover_mygo(dir).into_iter().map(OnlinePlugin::Mygo));
    out
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
    fn home_page_normalizes_mygo() {
        let browse = serde_json::json!({
            "title": "首页",
            "subtitle": "插件提供的",
            "sections": [{
                "title": "推荐",
                "tracks": [{ "id": "1", "name": "歌", "artists": [{ "name": "人" }], "duration": 200 }],
                "playlists": [{ "id": "p1", "name": "歌单", "imageUrl": "" }]
            }]
        });
        let page = mygo_home_page(&browse);
        assert_eq!(page.title, "首页");
        assert_eq!(page.sections.len(), 1);
        assert_eq!(page.sections[0].title, "推荐");
        assert_eq!(page.sections[0].tracks[0]["title"], "歌");
        assert_eq!(page.sections[0].tracks[0]["artist"], "人");
        assert_eq!(page.sections[0].cards[0]["title"], "歌单");
    }

    #[test]
    fn home_page_normalizes_musicfree() {
        let lists = serde_json::json!([{
            "title": "榜单",
            "data": [{ "id": "t1", "title": "热歌榜", "coverImg": "" }]
        }]);
        let page = musicfree_home_page(&lists);
        assert_eq!(page.sections.len(), 1);
        assert_eq!(page.sections[0].title, "榜单");
        assert_eq!(page.sections[0].cards[0]["title"], "热歌榜");
    }

    #[test]
    fn static_meta_reads_without_executing() {
        // platform 直接写在源码里，扫描时不应该真的跑插件
        let meta = inspect(MOCK).unwrap();
        assert_eq!(meta.platform, "MockFM");
        assert_eq!(meta.version, "1.2.3");
        assert!(meta.methods.contains(&"search".to_string()));
    }

    #[test]
    fn scan_does_not_execute_plugin_code() {
        // 顶层写死循环也必须秒回：扫描只做语法检查，不执行
        let dir = std::env::temp_dir().join(format!("audplugins-scan-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("loop.js"),
            "while (true) {}\nmodule.exports = { platform: \"Loop\", version: \"1\" };",
        )
        .unwrap();
        let start = std::time::Instant::now();
        let plugins = scan_async(dir.clone()).recv().unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
        assert_eq!(plugins.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_syntax_is_reported_in_chinese() {
        let err = friendly_error(
            "SyntaxError: formal parameter `z` declared in lexically declared names at line 139, col 12 (native)",
        );
        assert!(err.contains("第 139 行"));
        assert!(err.contains("重复声明"));
    }

    const ABORT_PLUGIN: &str = r#"
module.exports = {
  platform: "AbortFM",
  version: "0.1.0",
  async search() {
    const controller = new AbortController();
    if (typeof AbortSignal.timeout !== "undefined") { /* MyGo 才有时基 */ }
    let fired = false;
    controller.signal.addEventListener("abort", () => { fired = true; });
    controller.abort();
    return { isEnd: true, data: [{ id: "1", title: (controller.signal.aborted && fired) ? "aborted" : "bad" }] };
  },
  async getMediaSource() { return { url: "http://example.invalid/a.mp3" }; },
};
"#;

    #[test]
    fn abort_controller_in_polyfill() {
        let out = call_method(ABORT_PLUGIN, "search", &[]).unwrap();
        assert_eq!(out["data"][0]["title"], Value::String("aborted".into()));
    }

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

    const NETEASE: &str = r#"
const CryptoJs = require("crypto-js");
const bigInt = require("big-integer");
function AES(a, b) {
  var c = CryptoJs.enc.Utf8.parse(b),
    d = CryptoJs.enc.Utf8.parse("0102030405060708"),
    e = CryptoJs.enc.Utf8.parse(a),
    f = CryptoJs.AES.encrypt(e, c, { iv: d, mode: CryptoJs.mode.CBC });
  return f.toString();
}
function Rsa(text) {
  text = text.split("").reverse().join("");
  const d = "010001";
  const e = "00e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b725152b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbda92557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe4875d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7";
  const hexText = text.split("").map(function (_) { return _.charCodeAt(0).toString(16); }).join("");
  const res = bigInt(hexText, 16).modPow(bigInt(d, 16), bigInt(e, 16)).toString(16);
  return Array(256 - res.length).fill("0").join("").concat(res);
}
module.exports = {
  platform: "NetEaseAes",
  version: "0.0.0",
  async search() {
    return { isEnd: true, data: [{ id: "1", title: AES('{"csrf_token":""}', "0CoJUm6Qyw8W8jud"), rsa: Rsa("abcdefghijklmnop") }] };
  },
};
"#;

    #[test]
    fn aes_and_rsa_like_netease_plugin() {
        let out = call_method(NETEASE, "search", &[Value::String("x".into()), Value::from(1), Value::String("music".into())])
            .unwrap();
        // 与 `openssl enc -aes-128-cbc` 的结果对齐
        assert_eq!(
            out["data"][0]["title"],
            Value::from("eHhjXckqrtZkqcwCalCMx0QuU6Lj9L7Wxouw1iMCnB4=")
        );
        assert_eq!(
            out["data"][0]["rsa"],
            Value::from("d15a1683c992095d0c234c19966605c5c5964911268bbeda8cb8d08d834913e59d53b32358903a121b5fca784c1f5ae44951fd02524df58ecc98e52cc7cf8689b42c2e93ddf05b0592512d87f5960467e2f086c018849d76014d323500e30f13ef4cafbb0cf5a66731a3f1776c75ca35d0062dac70a3e33245afabcf47938487")
        );
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

#[cfg(test)]
mod meta_tests {
    use super::*;

    #[test]
    fn static_meta_reads_top_lists() {
        let code = r#"
module.exports = {
  platform: "X",
  version: "1.0",
  async getTopLists() { return []; },
  async search(q) { return { data: [] }; },
};
"#;
        let meta = static_meta(code).unwrap();
        assert!(meta.methods.contains(&"getTopLists".to_string()), "{:?}", meta.methods);
        assert!(meta.methods.contains(&"search".to_string()));
    }

#[cfg(test)]
mod header_tests {
    use super::*;

    #[test]
    fn headers_are_collected_from_map_and_array() {
        let map = serde_json::json!({ "url": "https://x/y.mp3", "headers": { "Referer": "https://b23.tv", "User-Agent": "ua" } });
        let list = collect_headers(&map);
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|(k, v)| k == "Referer" && v == "https://b23.tv"));

        let array = serde_json::json!({ "headers": [{ "name": "X-Test", "value": "1" }] });
        let list = collect_headers(&array);
        assert_eq!(list[0].0, "X-Test");
    }

    #[test]
    fn fetch_sends_plugin_headers() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        // 本地起一个服务：只有带 Referer 的请求才返回 200
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let Ok(mut stream) = stream else { continue };
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf);
                let text = String::from_utf8_lossy(&buf).to_lowercase();
                let ok = text.contains("referer: https://b23.tv");
                let response = if ok {
                    "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nDATA"
                } else {
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let url = format!("http://127.0.0.1:{port}/stream.mp3");
        let without = fetch_to_cache_with(&url, &[]);
        assert!(without.is_err(), "没有 Referer 应该被拒绝");

        let headers = vec![("Referer".to_string(), "https://b23.tv".to_string())];
        let with = fetch_to_cache_with(&url, &headers).expect("带 Referer 应该成功");
        assert!(with.metadata().map(|m| m.len() > 0).unwrap_or(false));
        let _ = std::fs::remove_file(with);
    }
}
}
pub mod install;

#[cfg(test)]
mod spawn_tests {
    use super::*;
    use boa_engine::Source;

    #[test]
    fn glob_match_patterns() {
        assert!(glob_match("bin/qishui-helper-*", "bin/qishui-helper-windows-amd64.exe"));
        assert!(glob_match("ffmpeg", "ffmpeg"));
        assert!(!glob_match("ffmpeg", "ffprobe"));
        assert!(glob_match("*helper*", "qishui-helper"));
    }

    #[test]
    fn spawn_requires_declared_program() {
        let dir = std::env::temp_dir().join(format!("audplugins-spawn-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        set_process_policy(dir.clone(), vec!["helper.sh".to_string()]);
        let mut ctx = new_context();
        let deny = ctx.eval(Source::from_bytes(
            r#"__spawn(JSON.stringify({command: "rm", args: []}))"#,
        ));
        assert!(deny.is_err(), "没声明的程序必须被拒绝");
        set_process_policy(std::path::PathBuf::new(), Vec::new());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spawn_pipes_roundtrip() {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!("audplugins-pipe-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let helper = dir.join("helper.sh");
            std::fs::write(&helper, "#!/bin/sh\nread line\necho \"{\\\"ok\\\":true,\\\"echo\\\":$line}\"\n").unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
            set_process_policy(dir.clone(), vec!["helper.sh".to_string()]);
            let mut ctx = new_context();
            let reply = ctx
                .eval(Source::from_bytes(
                    r#"
                    var id = __spawn(JSON.stringify({command: "helper.sh", args: []}));
                    __spawnWrite(id, "ping\n");
                    var line = __spawnReadLine(id, 0);
                    __spawnKill(id);
                    line;
                    "#,
                ))
                .expect("spawn pipeline")
                .to_string(&mut ctx)
                .unwrap()
                .to_std_string_escaped();
            assert!(reply.contains("ping"), "reply = {reply}");
            set_process_policy(std::path::PathBuf::new(), Vec::new());
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
