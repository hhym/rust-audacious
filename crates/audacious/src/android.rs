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
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
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

/// 更新「正在播放」通知（标题 / 艺人 / 是否播放中）。
pub fn update_notification(title: &str, artist: &str, playing: bool) -> bool {
    let Some(app) = APP.get() else { return false };
    let result = with_activity(app, |env, activity| {
        let t = env.new_string(title)?;
        let a = env.new_string(artist)?;
        env.call_method(
            activity,
            "updateNotification",
            "(Ljava/lang/String;Ljava/lang/String;Z)V",
            &[
                JValue::Object(&JObject::from(t)),
                JValue::Object(&JObject::from(a)),
                JValue::Bool(playing as u8),
            ],
        )?;
        Ok(())
    });
    match result {
        Ok(()) => true,
        Err(err) => {
            log::warn!("更新通知失败：{err}");
            false
        }
    }
}

/// 把「正在播放」写进 MediaSession（锁屏、耳机线控、蓝牙按键都读它）。
pub fn update_media_session(title: &str, artist: &str, playing: bool, pos: f64, len: f64) {
    let Some(app) = APP.get() else { return };
    let result = with_activity(app, |env, activity| {
        let t = env.new_string(title)?;
        let a = env.new_string(artist)?;
        env.call_method(
            activity,
            "updateMediaSession",
            "(Ljava/lang/String;Ljava/lang/String;ZJJ)V",
            &[
                JValue::Object(&JObject::from(t)),
                JValue::Object(&JObject::from(a)),
                JValue::Bool(playing as u8),
                JValue::Long((pos * 1000.0) as i64),
                JValue::Long((len * 1000.0) as i64),
            ],
        )?;
        Ok(())
    });
    if let Err(err) = result {
        log::warn!("更新 MediaSession 失败：{err}");
    }
}

/// 锁屏 / 线控的播放控制回调：4 播放 / 5 暂停 / 6 停止。
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_io_github_hhym_audaciousrs_MainActivity_nativeOnMediaAction(
    _env: Env,
    _class: JClass,
    action: jint,
) {
    let Some(shared) = SHARED.get() else { return };
    match action {
        4 => {
            shared.send(Cmd::Play);
        }
        5 => {
            shared.send(Cmd::Pause);
        }
        6 => {
            shared.send(Cmd::Stop);
        }
        _ => {}
    }
}

/// 锁屏进度条拖动（秒）。
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_io_github_hhym_audaciousrs_MainActivity_nativeOnMediaSeek(
    _env: Env,
    _class: JClass,
    seconds: jni::sys::jdouble,
) {
    if let Some(shared) = SHARED.get() {
        if seconds >= 0.0 {
            shared.send(Cmd::Seek(seconds));
        }
    }
}

/// 状态栏 / 导航栏内边距（物理像素），由 Java 的 `OnApplyWindowInsetsListener` 上报。
static INSET_TOP: AtomicU32 = AtomicU32::new(0);
static INSET_BOTTOM: AtomicU32 = AtomicU32::new(0);

/// 顶部内边距（egui 点）。桌面版恒为 0。
pub fn inset_top_pt(pixels_per_point: f32) -> f32 {
    let px = INSET_TOP.load(std::sync::atomic::Ordering::Relaxed) as f32;
    if pixels_per_point > 0.0 {
        px / pixels_per_point
    } else {
        px
    }
}

/// 底部内边距（egui 点）。
pub fn inset_bottom_pt(pixels_per_point: f32) -> f32 {
    let px = INSET_BOTTOM.load(std::sync::atomic::Ordering::Relaxed) as f32;
    if pixels_per_point > 0.0 {
        px / pixels_per_point
    } else {
        px
    }
}

/// Java 侧 `MainActivity.nativeSetInsets` 的实现。
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_io_github_hhym_audaciousrs_MainActivity_nativeSetInsets(
    _env: Env,
    _class: JClass,
    top: jint,
    bottom: jint,
) {
    INSET_TOP.store(top.max(0) as u32, std::sync::atomic::Ordering::Relaxed);
    INSET_BOTTOM.store(bottom.max(0) as u32, std::sync::atomic::Ordering::Relaxed);
}

/// 软键盘事件：Java 用隐形 EditText 接住 IME 输入，再经由这里交给 egui。
pub enum ImeEvent {
    Text(String),
    Backspace,
    Enter,
}

static IME_EVENTS: OnceLock<Mutex<Vec<ImeEvent>>> = OnceLock::new();

fn ime_queue() -> &'static Mutex<Vec<ImeEvent>> {
    IME_EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 取走这一帧攒下的软键盘事件。
pub fn take_ime_events() -> Vec<ImeEvent> {
    ime_queue()
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// 抬起 / 收起软键盘（egui 的文本框拿到焦点时抬）。
pub fn ime_show(show: bool) {
    let Some(app) = APP.get() else { return };
    let _ = with_activity(app, |env, activity| {
        env.call_method(
            activity,
            "imeShow",
            "(Z)V",
            &[JValue::Bool(if show { 1 } else { 0 })],
        )?;
        Ok(())
    });
}

/// Java 侧 `nativeImeText`：一段（可能含多字的）文本。
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_io_github_hhym_audaciousrs_MainActivity_nativeImeText(
    mut env: Env,
    _class: JClass,
    text: JString,
) {
    let Ok(raw) = env.get_string(&text) else { return };
    let s = String::from(raw);
    if s.is_empty() {
        return;
    }
    if let Ok(mut q) = ime_queue().lock() {
        q.push(ImeEvent::Text(s));
    }
}

/// Java 侧 `nativeImeKey`：8 退格，13 回车。
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_io_github_hhym_audaciousrs_MainActivity_nativeImeKey(
    _env: Env,
    _class: JClass,
    code: jint,
) {
    let ev = if code == 13 {
        ImeEvent::Enter
    } else {
        ImeEvent::Backspace
    };
    if let Ok(mut q) = ime_queue().lock() {
        q.push(ev);
    }
}

/// 机型串（厂商 / 品牌 / 型号 / 设备代号，小写合并），用来判断默认界面。
pub fn device_model() -> String {
    let Some(app) = APP.get() else { return String::new() };
    with_activity(app, |env, _activity| {
        let mut parts: Vec<String> = Vec::new();
        if let Ok(cls) = env.find_class("android/os/Build") {
            for field in ["MANUFACTURER", "BRAND", "MODEL", "DEVICE", "PRODUCT"] {
                if let Ok(v) = env.get_static_field(&cls, field, "Ljava/lang/String;") {
                    if let Ok(obj) = v.l() {
                        let jstr = JString::from(obj);
                        let text = env.get_string(&jstr).map(String::from).ok();
                        if let Some(text) = text {
                            parts.push(text);
                        }
                    }
                }
            }
        }
        Ok(parts.join(" ").to_lowercase())
    })
    .unwrap_or_default()
}

/// 是不是墨水屏机器。没有可靠的系统 API，用厂商/型号关键词兜底：
/// 认不出来就当彩屏，走图标版界面（这才是绝大多数设备）。
pub fn is_eink_device(model: &str) -> bool {
    const KEYS: [&str; 12] = [
        "boox", "onyx", "ireader", "kindle", "kobo", "remarkable", "mudita", "yotaphone",
        "eink", "e-ink", "hlte", "sc801a",
    ];
    let m = model.to_ascii_lowercase();
    KEYS.iter().any(|k| m.contains(k))
}

/// 停止播放时把通知收掉。
pub fn clear_notification() {
    let Some(app) = APP.get() else { return };
    if let Err(err) = call_activity(app, "clearNotification") {
        log::warn!("清除通知失败：{err}");
    }
}

/// Java 侧 `MainActivity.nativeOnNotificationAction` 的实现：
/// 1 切换播放 / 2 下一首 / 3 上一首。
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_io_github_hhym_audaciousrs_MainActivity_nativeOnNotificationAction(
    _env: Env,
    _class: JClass,
    action: jint,
) {
    let Some(shared) = SHARED.get() else { return };
    match action {
        1 => {
            shared.send(Cmd::Toggle);
        }
        2 => {
            shared.send(Cmd::Next);
        }
        3 => {
            shared.send(Cmd::Prev);
        }
        _ => {}
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

/// 用 MediaStore 扫一遍本地音乐（设备上没有 DocumentsUI 时的主路径）。
pub fn scan_audio_paths() -> Vec<PathBuf> {
    let Some(app) = APP.get() else {
        return Vec::new();
    };
    let res = with_activity(app, |env, activity| {
        let cls = env.get_object_class(activity)?;
        let arr = env
            .call_static_method(cls, "scanAudioPaths", "()[Ljava/lang/String;", &[])?
            .l()?;
        let arr = unsafe { jni::objects::JObjectArray::from_raw(arr.into_raw()) };
        let len = env.get_array_length(&arr)? as usize;
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            let obj = env.get_object_array_element(&arr, i as jint)?;
            let s: String = env.get_string(&JString::from(obj))?.into();
            if !s.is_empty() {
                out.push(PathBuf::from(s));
            }
        }
        Ok(out)
    });
    match res {
        Ok(mut v) => {
            v.sort();
            v.dedup();
            log::info!("MediaStore 扫到 {} 个音频文件", v.len());
            v
        }
        Err(err) => {
            log::error!("扫描媒体库失败：{err:?}");
            Vec::new()
        }
    }
}

/// 打开系统文件选择器（多选音频）。没有 Activity 上下文时返回 false。
pub fn open_audio_picker() -> bool {
    launch_picker(false)
}

/// 外部存储里用户最可能放插件的位置。内置浏览器从这里起步，
/// 免得落在应用沙箱（空目录）里，看起来像「打不开」。
pub fn storage_root() -> PathBuf {
    for p in ["/storage/emulated/0/Download", "/storage/emulated/0", "/sdcard"] {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return path;
        }
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// 插件选择：`zip = true` 挑 .zip 压缩包，`false` 挑插件目录。
/// 返回 false 表示这台机器没有文档选择器，调用方改用内置浏览器。
pub fn open_plugin_pick(zip: bool) -> bool {
    let Some(app) = APP.get() else {
        return false;
    };
    let res = with_activity(app, |env, activity| {
        let cls = env.get_object_class(activity)?;
        env.call_static_method(
            cls,
            "openPluginPick",
            "(I)Z",
            &[JValue::Int(if zip { 0 } else { 1 })],
        )?
        .z()
    });
    matches!(res, Ok(true))
}

/// Java 侧 `MainActivity.nativePluginReady`：系统选择器收回的路径（zip 已拷到缓存目录，
/// 目录是 SAF tree 转出来的真实路径）。
static PLUGIN_READY: Mutex<Option<String>> = Mutex::new(None);

#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_io_github_hhym_audaciousrs_MainActivity_nativePluginReady(
    mut env: Env,
    _class: JClass,
    path: JString,
) {
    if let Ok(s) = env.get_string(&path) {
        let text: String = s.into();
        log::info!("插件选择结果：{text}");
        if let Ok(mut slot) = PLUGIN_READY.lock() {
            *slot = Some(text);
        }
    }
}

/// UI 每帧查一次，拿到就装。
pub fn take_plugin_ready() -> Option<String> {
    PLUGIN_READY.lock().ok().and_then(|mut g| g.take())
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
        Ok(true) => true,
        Ok(false) => {
            log::warn!("这台机器没有文档选择器（DocumentsUI）");
            false
        }
        Err(err) => {
            log::error!("打开 SAF 选择器失败：{err:?}");
            if let Ok(vm) = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _) } {
                if let Ok(mut env) = vm.attach_current_thread() {
                    if env.exception_check().unwrap_or(false) {
                        let _ = env.exception_describe();
                        let _ = env.exception_clear();
                    }
                }
            }
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
    // egui 渲染线程不在 VM 里 attach 过，get_env 会返回 JNI_EDETACHED，
    // 之后每次调用都只会得到「Java exception was thrown」。显式 attach 一次。
    let mut guard = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    f(&mut guard, &activity)
}

fn start_picker(app: &android_activity::AndroidApp, skin: bool) -> jni::errors::Result<bool> {
    with_activity(app, |env, activity| {
        // 原生线程直接 startActivityForResult 会抛异常（不是 UI 线程），
        // 统一交给 Java 侧切到主线程再拉起选择器。
        let mime = env.new_string(if skin { "*/*" } else { "audio/*" })?;
        // 用 Activity 实例取类对象，避免 native 线程里 FindClass 落到系统
        // class loader 上找不到本应用的类。
        let cls = env.get_object_class(activity)?;
        let launched = env
            .call_static_method(
            cls,
            "openPicker",
            "(ILjava/lang/String;Z)Z",
            &[
                JValue::Int(if skin { REQ_PICK_SKIN } else { REQ_PICK_AUDIO }),
                JValue::Object(&JObject::from(mime)),
                JValue::Bool(if skin { 0 } else { 1 }),
            ],
        )?
        .z()?;
        Ok(launched)
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
