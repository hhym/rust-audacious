//! Android 平台胶水：SAF（系统文件选择器）与 Activity 结果回调。
//!
//! - Java 侧：`android/java/io/github/hhym/audaciousrs/MainActivity.java` 继承
//!   `android.app.NativeActivity`，把 `onActivityResult` 转给这里的 JNI 方法；
//! - Rust 侧：[`open_audio_picker`] 用 JNI 拉起 `ACTION_OPEN_DOCUMENT`，
//!   回调里只做「查文件名 + detachFd」，真正的读盘拷进 `files/imported/`
//!   交给一个后台线程，UI 线程不会被几十 MB 的整轨卡住。
//!
//! 为什么拷一份：SAF 给的是 `content://` URI，不是文件路径，symphonia 要能随机读的
//! `File`；替身 fd 又会随进程/授权生命周期失效，落盘最省心。

use std::os::unix::io::FromRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::engine::{Cmd, Shared, Status};

use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, JNIEnv};
use jni::{JNIEnv as Env, JavaVM};

/// 拉起系统文件选择器的 requestCode；Java 侧原样带回。
const REQ_PICK_AUDIO: jint = 0x51AF;
const REQ_PICK_SKIN: jint = 0x51B0;
/// `Activity.RESULT_OK`
const RESULT_OK: jint = -1;

/// 回调 → 后台导入线程的一个待拷文件。
struct Job {
    fd: i32,
    name: String,
    size: u64,
}

static APP: OnceLock<android_activity::AndroidApp> = OnceLock::new();
static SHARED: OnceLock<Shared> = OnceLock::new();
/// 当前是否握着音频焦点（避免每帧重复申请）
static FOCUS_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 是不是因为焦点丢失才暂停的（只有这种暂停才自动恢复）
static AUTO_PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static JOBS: OnceLock<Sender<Job>> = OnceLock::new();
static PICKED: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
static STATUS: OnceLock<Mutex<String>> = OnceLock::new();
static BYTES: AtomicU64 = AtomicU64::new(0);
static TOTAL: AtomicU64 = AtomicU64::new(0);
/// 皮肤导入完成，等 UI 重新加载
static SKIN_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn jobs() -> Option<&'static Sender<Job>> {
    JOBS.get()
}

fn picked() -> &'static Mutex<Vec<PathBuf>> {
    PICKED.get_or_init(|| Mutex::new(Vec::new()))
}

fn status_slot() -> &'static Mutex<String> {
    STATUS.get_or_init(|| Mutex::new(String::new()))
}

fn set_status(text: impl Into<String>) {
    *status_slot().lock().unwrap_or_else(|e| e.into_inner()) = text.into();
}

/// UI 每帧调用：取走这次选择器落地的文件路径（取完即清空）。
pub fn take_picked() -> Vec<PathBuf> {
    let mut list = picked().lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *list)
}

/// UI 每帧调用：取走最新状态提示（导入进度/结果），取完即清空。
pub fn take_status() -> Option<String> {
    let mut slot = status_slot().lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_empty() {
        None
    } else {
        Some(std::mem::take(&mut *slot))
    }
}

/// 记住 `AndroidApp`，后续 JNI 调用要靠它拿 JavaVM / Activity。
pub fn install(app: &android_activity::AndroidApp) {
    let _ = APP.set(app.clone());
}

/// 记住引擎句柄，音频焦点回调直接往引擎发命令。
pub fn install_shared(shared: Shared) {
    let _ = SHARED.set(shared);
}

/// 每帧调一次：播放中申请焦点，停止时释放。
pub fn maintain_focus(playing: bool, stopped: bool) {
    use std::sync::atomic::Ordering;
    let Some(app) = APP.get() else { return };

    if playing && !FOCUS_HELD.swap(true, Ordering::Relaxed) {
        if let Err(err) = call_activity(app, "requestAudioFocus") {
            log::warn!("申请音频焦点失败：{err}");
            FOCUS_HELD.store(false, Ordering::Relaxed);
        }
    } else if stopped && FOCUS_HELD.swap(false, Ordering::Relaxed) {
        let _ = call_activity(app, "abandonAudioFocus");
        AUTO_PAUSED.store(false, Ordering::Relaxed);
    }
}

fn call_activity(app: &android_activity::AndroidApp, method: &str) -> jni::errors::Result<()> {
    with_activity(app, |env, activity| {
        env.call_method(activity, method, "()V", &[])?;
        Ok(())
    })
}

/// 起后台导入线程；必须在 `install` 之后、UI 起来之前调一次。
pub fn start_importer() {
    if JOBS.get().is_some() {
        return;
    }
    let (tx, rx) = channel::<Job>();
    if JOBS.set(tx).is_ok() {
        let spawned = std::thread::Builder::new()
            .name("saf-import".into())
            .spawn(move || {
                for job in rx {
                    let dest = import_dir().join(&job.name);
                    match copy_fd(job.fd, &dest, job.size) {
                        Ok(path) => {
                            picked()
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(path.clone());
                            log::info!("导入 → {}", path.display());
                            set_status(format!("已导入 {}", job.name));
                        }
                        Err(err) => {
                            log::error!("导入 {} 失败：{err}", job.name);
                            set_status(format!("导入失败：{err}"));
                        }
                    }
                }
            });
        if spawned.is_err() {
            log::error!("导入线程起不来");
        }
    }
}

fn import_dir() -> PathBuf {
    let dir = audcore::config::data_dir().join("imported");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 打开系统文件选择器（多选音频）。没有 Activity 上下文时返回 false。
pub fn open_audio_picker() -> bool {
    launch_picker(false)
}

/// 打开系统文件选择器（单选 .wsz 皮肤）。
pub fn open_skin_picker() -> bool {
    launch_picker(true)
}

fn launch_picker(skin: bool) -> bool {
    let Some(app) = APP.get() else {
        log::warn!("没有 AndroidApp 上下文，无法打开 SAF 选择器");
        return false;
    };

    match start_picker(app, skin) {
        Ok(()) => true,
        Err(err) => {
            log::error!("打开 SAF 选择器失败：{err}");
            false
        }
    }
}

/// 皮肤是否已经导入完成（UI 每帧查一次，导完就重新加载）
pub fn take_skin_ready() -> bool {
    SKIN_READY.swap(false, std::sync::atomic::Ordering::Relaxed)
}

fn with_activity<T>(
    app: &android_activity::AndroidApp,
    f: impl FnOnce(&mut Env, &JObject) -> jni::errors::Result<T>,
) -> jni::errors::Result<T> {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _) }?;
    let mut env = vm.get_env()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    f(&mut env, &activity)
}

fn start_picker(app: &android_activity::AndroidApp, skin: bool) -> jni::errors::Result<()> {
    with_activity(app, |env, activity| {
        let intent = env.new_object("android/content/Intent", "()V", &[])?;
        let action = env.new_string("android.intent.action.OPEN_DOCUMENT")?;
        env.call_method(
            &intent,
            "setAction",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&JObject::from(action))],
        )?;
        // 皮肤是 zip 换扩展名，只能按 */* 让它出现在选择器里
        let mime = env.new_string(if skin { "*/*" } else { "audio/*" })?;
        env.call_method(
            &intent,
            "setType",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&JObject::from(mime))],
        )?;
        if !skin {
            // 音频允许多选；皮肤一次一个
            let extra_multi = env.new_string("android.intent.extra.ALLOW_MULTIPLE")?;
            env.call_method(
                &intent,
                "putExtra",
                "(Ljava/lang/String;Z)Landroid/content/Intent;",
                &[JValue::Object(&JObject::from(extra_multi)), JValue::Bool(1)],
            )?;
        }
        env.call_method(
            activity,
            "startActivityForResult",
            "(Landroid/content/Intent;I)V",
            &[JValue::Object(&intent), JValue::Int(if skin { REQ_PICK_SKIN } else { REQ_PICK_AUDIO })],
        )?;
        Ok(())
    })
}

/// Java 侧 `MainActivity.nativeOnActivityResult` 的实现。
///
/// 静态 native 方法：第二个参数是 `JClass`。回调跑在 UI 线程，因此这里只做
/// 查名字 / `openFileDescriptor` / `detachFd` 这些毫秒级动作，读盘交给后台线程。
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_io_github_hhym_audaciousrs_MainActivity_nativeOnActivityResult(
    mut env: Env,
    _class: JClass,
    request_code: jint,
    result_code: jint,
    data: JObject,
) {
    if result_code != RESULT_OK || data.is_null() {
        return;
    }
    if request_code == REQ_PICK_SKIN {
        match install_skin(&mut env, &data) {
            Ok(()) => {
                SKIN_READY.store(true, Ordering::Relaxed);
                set_status("皮肤已导入，正在应用…");
                log::info!("WSZ 皮肤已复制到私有目录");
            }
            Err(err) => {
                set_status("皮肤导入失败");
                log::error!("导入皮肤失败：{err}");
            }
        }
        return;
    }
    if request_code != REQ_PICK_AUDIO {
        return;
    }
    match enqueue_uris(&mut env, &data) {
        Ok(0) => log::warn!("SAF 没有返回可用文件"),
        Ok(n) => {
            set_status(format!("正在导入 {n} 首…"));
            log::info!("SAF 选中 {n} 个文件并已交给导入线程");
        }
        Err(err) => log::error!("处理 SAF 结果失败：{err}"),
    }
}

/// Java 侧 `MainActivity.nativeOnAudioFocusChange` 的实现。
///
/// AUDIOFOCUS_LOSS(-1) / LOSS_TRANSIENT(-2)：抢占就暂停，并记住是「被抢」导致的；
/// GAIN(1) / GAIN_TRANSIENT(2)：只有被抢暂停过才自动续播，用户自己按的暂停不会被打断；
/// CAN_DUCK(-3) 先不处理（不降音量，保持原样）。
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_io_github_hhym_audaciousrs_MainActivity_nativeOnAudioFocusChange(
    _env: Env,
    _class: JClass,
    change: jint,
) {
    use std::sync::atomic::Ordering;
    let Some(shared) = SHARED.get() else { return };
    let status = shared
        .state
        .lock()
        .map(|s| s.status)
        .unwrap_or(Status::Stopped);

    match change {
        -1 | -2 => {
            if matches!(status, Status::Playing) {
                log::info!("音频焦点被抢占（{change}），暂停");
                shared.send(Cmd::Pause);
                AUTO_PAUSED.store(true, Ordering::Relaxed);
            }
        }
        1 | 2 => {
            if AUTO_PAUSED.swap(false, Ordering::Relaxed) {
                log::info!("音频焦点恢复，续播");
                shared.send(Cmd::Play);
            }
        }
        _ => log::debug!("音频焦点变化 {change}，忽略"),
    }
}

fn enqueue_uris(env: &mut Env, data: &JObject) -> anyhow::Result<usize> {
    let mut uris: Vec<JObject> = Vec::new();

    // 多选走 ClipData，单选走 getData()
    if let Ok(clip) = env.call_method(data, "getClipData", "()Landroid/content/ClipData;", &[]) {
        let clip = clip.l()?;
        if !clip.is_null() {
            let count = env.call_method(&clip, "getItemCount", "()I", &[])?.i()?;
            for i in 0..count {
                let item = env
                    .call_method(
                        &clip,
                        "getItemAt",
                        "(I)Landroid/content/ClipData$Item;",
                        &[JValue::Int(i)],
                    )?
                    .l()?;
                let uri = env
                    .call_method(&item, "getUri", "()Landroid/net/Uri;", &[])?
                    .l()?;
                if !uri.is_null() {
                    uris.push(uri);
                }
            }
        }
    }

    if uris.is_empty() {
        let uri = env
            .call_method(data, "getData", "()Landroid/net/Uri;", &[])?
            .l()?;
        if !uri.is_null() {
            uris.push(uri);
        }
    }

    let Some(app) = APP.get() else {
        anyhow::bail!("AndroidApp 还没装好");
    };
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    if activity.is_null() {
        anyhow::bail!("拿不到 Activity");
    }
    let resolver = env
        .call_method(
            &activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()?;

    let Some(sender) = jobs() else {
        anyhow::bail!("导入线程没起来");
    };

    let mut queued = 0usize;
    let mut total = 0u64;
    for uri in uris {
        let name = display_name(env, &resolver, &uri).unwrap_or_else(|| "imported".to_string());
        let ext = extension_for(env, &resolver, &uri);
        let safe_name = unique_name(&import_dir(), &sanitize(&name, ext));

        let mode = env.new_string("r")?;
        let pfd = env
            .call_method(
                &resolver,
                "openFileDescriptor",
                "(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
                &[JValue::Object(&uri), JValue::Object(&JObject::from(mode))],
            )?
            .l()?;
        if pfd.is_null() {
            log::warn!("openFileDescriptor 返回 null，跳过这个文件");
            continue;
        }
        let size = env
            .call_method(&pfd, "getStatSize", "()J", &[])?
            .j()
            .unwrap_or(-1);
        let size = if size > 0 { size as u64 } else { 0 };
        // detach 后 fd 归 Rust 所有，Java 侧不再关它
        let fd = env.call_method(&pfd, "detachFd", "()I", &[])?.i()?;
        if fd < 0 {
            log::warn!("detachFd 失败，跳过这个文件");
            continue;
        }

        total += size;
        let job = Job {
            fd,
            name: safe_name,
            size,
        };
        if sender.send(job).is_err() {
            anyhow::bail!("导入线程已退出");
        }
        queued += 1;
    }

    BYTES.store(0, Ordering::Relaxed);
    TOTAL.store(total, Ordering::Relaxed);
    Ok(queued)
}

/// 把选中的 .wsz 复制成 `skins/current.wsz`。
fn install_skin(env: &mut Env, data: &JObject) -> anyhow::Result<()> {
    let uri = env
        .call_method(data, "getData", "()Landroid/net/Uri;", &[])?
        .l()?;
    if uri.is_null() {
        anyhow::bail!("选择器没返回 URI");
    }
    let Some(app) = APP.get() else {
        anyhow::bail!("AndroidApp 还没装好");
    };
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    let resolver = env
        .call_method(
            &activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()?;

    let mode = env.new_string("r")?;
    let pfd = env
        .call_method(
            &resolver,
            "openFileDescriptor",
            "(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
            &[JValue::Object(&uri), JValue::Object(&JObject::from(mode))],
        )?
        .l()?;
    if pfd.is_null() {
        anyhow::bail!("openFileDescriptor 返回 null");
    }
    let fd = env.call_method(&pfd, "detachFd", "()I", &[])?.i()?;
    if fd < 0 {
        anyhow::bail!("detachFd 失败");
    }

    let dest = crate::skin::current_skin_path();
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 先写临时文件再改名，避免加载到写了一半的包
    let tmp = dest.with_extension("wsz.tmp");
    copy_fd(fd, &tmp, 0)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(())
}

/// 把 detach 出来的 fd 读进私有目录。跑在后台线程，顺便刷新进度。
fn copy_fd(fd: i32, dest: &std::path::Path, _size: u64) -> std::io::Result<PathBuf> {
    use std::io::{Read, Write};

    let mut src = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut out = std::fs::File::create(dest)?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut last = Instant::now();
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        let done = BYTES.fetch_add(n as u64, Ordering::Relaxed) + n as u64;

        // 进度提示每 150ms 刷一次，避免每读一块就抢一次锁
        if last.elapsed() >= Duration::from_millis(150) {
            last = Instant::now();
            let total = TOTAL.load(Ordering::Relaxed);
            if total > 0 {
                let pct = (done.min(total) * 100 / total) as u32;
                set_status(format!("正在导入… {pct}%"));
            }
        }
    }
    Ok(dest.to_path_buf())
}

/// 同名文件不覆盖：`2-xxx.mp3`。
fn unique_name(dir: &std::path::Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "imported".into());
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut n = 2;
    loop {
        let candidate = if ext.is_empty() {
            format!("{n}-{stem}")
        } else {
            format!("{n}-{stem}.{ext}")
        };
        if !dir.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
    }
}

/// `OpenableColumns.DISPLAY_NAME`
fn display_name(env: &mut Env, resolver: &JObject, uri: &JObject) -> Option<String> {
    let cursor = env
        .call_method(
            resolver,
            "query",
            "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
            &[
                JValue::Object(uri),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
            ],
        )
        .ok()?
        .l()
        .ok()?;
    if cursor.is_null() {
        return None;
    }
    let moved = env
        .call_method(&cursor, "moveToFirst", "()Z", &[])
        .ok()?
        .z()
        .ok()?;
    let name = if moved {
        let column = env.new_string("_display_name").ok()?;
        let col = env
            .call_method(
                &cursor,
                "getColumnIndex",
                "(Ljava/lang/String;)I",
                &[JValue::Object(&JObject::from(column))],
            )
            .ok()?
            .i()
            .ok()?;
        env.call_method(
            &cursor,
            "getString",
            "(I)Ljava/lang/String;",
            &[JValue::Int(col)],
        )
        .ok()
        .and_then(|v| v.l().ok())
        .and_then(|s| read_string(env, s))
    } else {
        None
    };
    let _ = env.call_method(&cursor, "close", "()V", &[]);
    name
}

fn read_string(env: &mut Env, obj: JObject) -> Option<String> {
    let text = JString::from(obj);
    env.get_string(&text)
        .ok()
        .map(|s| s.to_string_lossy().into_owned())
}

fn extension_for(env: &mut Env, resolver: &JObject, uri: &JObject) -> &'static str {
    let mime = env
        .call_method(
            resolver,
            "getType",
            "(Landroid/net/Uri;)Ljava/lang/String;",
            &[JValue::Object(uri)],
        )
        .ok()
        .and_then(|v| v.l().ok())
        .and_then(|s| read_string(env, s))
        .unwrap_or_default();
    match mime.as_str() {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/ogg" | "audio/vorbis" => "ogg",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/aac" => "aac",
        "audio/opus" => "opus",
        _ => "mp3",
    }
}

/// 只留文件名，去掉路径分隔符；没有像样扩展名就按 MIME 补一个。
fn sanitize(name: &str, ext: &str) -> String {
    let base: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    let base = base.trim();
    let base = if base.is_empty() { "imported" } else { base };
    let has_ext = base
        .rsplit_once('.')
        .map(|(_, e)| {
            !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or(false);
    if has_ext {
        base.to_string()
    } else {
        format!("{base}.{ext}")
    }
}
