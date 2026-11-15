//! Android 平台胶水：SAF（系统文件选择器）与 Activity 结果回调。
//!
//! - Java 侧：`android/java/io/github/hhym/audaciousrs/MainActivity.java` 继承
//!   `android.app.NativeActivity`，把 `onActivityResult` 转给这里的 JNI 方法；
//! - Rust 侧：[`open_audio_picker`] 用 JNI 拉起 `ACTION_OPEN_DOCUMENT`，
//!   回调里把选中的内容 URI 通过 `ContentResolver` 读到 App 私有目录
//!   （`files/imported/`），再把落地路径塞进通道，由 UI 每帧取走。
//!
//! 之所以要拷一份：SAF 给的是 `content://` URI，不是文件路径，symphonia 要能随机读的
//! `File`；替身 fd 又会随进程/授权生命周期失效，落盘最省心。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, JNIEnv};
use jni::{JNIEnv as Env, JavaVM};

/// 拉起系统文件选择器的 requestCode；Java 侧原样带回。
const REQ_PICK_AUDIO: jint = 0x51AF;
/// `Activity.RESULT_OK`
const RESULT_OK: jint = -1;

static APP: OnceLock<android_activity::AndroidApp> = OnceLock::new();
static PICKED: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

fn picked() -> &'static Mutex<Vec<PathBuf>> {
    PICKED.get_or_init(|| Mutex::new(Vec::new()))
}

/// 记住 `AndroidApp`，后续 JNI 调用要靠它拿 JavaVM / Activity。
pub fn install(app: &android_activity::AndroidApp) {
    let _ = APP.set(app.clone());
}

/// UI 每帧调用：取走这次选择器落地的文件路径（取完即清空）。
pub fn take_picked() -> Vec<PathBuf> {
    let mut list = picked().lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *list)
}

/// 打开系统文件选择器（多选音频）。没有 Activity 上下文时返回 false。
pub fn open_audio_picker() -> bool {
    let Some(app) = APP.get() else {
        log::warn!("没有 AndroidApp 上下文，无法打开 SAF 选择器");
        return false;
    };

    match start_picker(app) {
        Ok(()) => true,
        Err(err) => {
            log::error!("打开 SAF 选择器失败：{err}");
            false
        }
    }
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

fn start_picker(app: &android_activity::AndroidApp) -> jni::errors::Result<()> {
    with_activity(app, |env, activity| {
        let intent = env.new_object("android/content/Intent", "()V", &[])?;
        let action = env.new_string("android.intent.action.OPEN_DOCUMENT")?;
        env.call_method(
            &intent,
            "setAction",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&JObject::from(action))],
        )?;
        let mime = env.new_string("audio/*")?;
        env.call_method(
            &intent,
            "setType",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&JObject::from(mime))],
        )?;
        // 允许一次选多首
        let extra_multi = env.new_string("android.intent.extra.ALLOW_MULTIPLE")?;
        env.call_method(
            &intent,
            "putExtra",
            "(Ljava/lang/String;Z)Landroid/content/Intent;",
            &[JValue::Object(&JObject::from(extra_multi)), JValue::Bool(1)],
        )?;
        env.call_method(
            activity,
            "startActivityForResult",
            "(Landroid/content/Intent;I)V",
            &[JValue::Object(&intent), JValue::Int(REQ_PICK_AUDIO)],
        )?;
        Ok(())
    })
}

/// Java 侧 `MainActivity.nativeOnActivityResult` 的实现。
///
/// 静态 native 方法：第二个参数是 `JClass`；回调跑在 UI 线程，
/// 只做「读流 → 落盘 → 塞通道」，重活留给主循环。
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_io_github_hhym_audaciousrs_MainActivity_nativeOnActivityResult(
    mut env: Env,
    _class: JClass,
    request_code: jint,
    result_code: jint,
    data: JObject,
) {
    if request_code != REQ_PICK_AUDIO || result_code != RESULT_OK || data.is_null() {
        return;
    }
    match import_uris(&mut env, &data) {
        Ok(paths) => {
            if !paths.is_empty() {
                log::info!("SAF 选中 {} 个文件", paths.len());
                let mut list = picked().lock().unwrap_or_else(|e| e.into_inner());
                list.extend(paths);
            }
        }
        Err(err) => log::error!("导入 SAF 文件失败：{err}"),
    }
}

fn import_uris(env: &mut Env, data: &JObject) -> anyhow::Result<Vec<PathBuf>> {
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

    let activity = unsafe {
        JObject::from_raw(
            APP.get()
                .map(|a| a.activity_as_ptr() as jni::sys::jobject)
                .unwrap_or(std::ptr::null_mut()),
        )
    };
    if activity.is_null() {
        return Ok(Vec::new());
    }
    let resolver = env
        .call_method(
            &activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()?;

    let dir = audcore::config::data_dir().join("imported");
    let _ = std::fs::create_dir_all(&dir);

    let mut out = Vec::new();
    for uri in uris {
        match copy_uri(env, &resolver, &uri, &dir) {
            Ok(path) => out.push(path),
            Err(err) => log::warn!("跳过某个文件：{err}"),
        }
    }
    Ok(out)
}

/// 把一个 `content://` 读进私有目录，返回落地路径。
fn copy_uri(
    env: &mut Env,
    resolver: &JObject,
    uri: &JObject,
    dir: &std::path::Path,
) -> anyhow::Result<PathBuf> {
    let name = display_name(env, resolver, uri).unwrap_or_else(|| "imported".to_string());
    let ext = extension_for(env, resolver, uri);
    let safe_name = sanitize(&name, ext);
    let mut dest = dir.join(&safe_name);
    let mut n = 1;
    while dest.exists() {
        dest = dir.join(format!("{n}-{safe_name}"));
        n += 1;
    }

    let stream = env
        .call_method(
            resolver,
            "openInputStream",
            "(Landroid/net/Uri;)Ljava/io/InputStream;",
            &[JValue::Object(uri)],
        )?
        .l()?;
    if stream.is_null() {
        anyhow::bail!("openInputStream 返回 null");
    }

    let mut file = std::fs::File::create(&dest)?;
    let buf = env.new_byte_array(64 * 1024)?;
    loop {
        let read = env
            .call_method(&stream, "read", "([B)I", &[JValue::Object(&buf)])?
            .i()?;
        if read <= 0 {
            break;
        }
        let bytes = env.convert_byte_array(&buf)?;
        std::io::Write::write_all(&mut file, &bytes[..read as usize])?;
    }
    let _ = env.call_method(&stream, "close", "()V", &[]);
    log::info!("导入 → {}", dest.display());
    Ok(dest)
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
        let column = env
            .new_string("_display_name")
            .ok()?;
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
    env.get_string(&text).ok().map(|s| s.to_string_lossy().into_owned())
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
        .map(|(_, e)| !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or(false);
    if has_ext {
        base.to_string()
    } else {
        format!("{base}.{ext}")
    }
}
