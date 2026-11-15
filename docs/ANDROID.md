# Android 分支说明（android）

`android` 分支的任务：把 `audacious-rs` 搬到安卓手机上跑。桌面主线（`main`）保持不动，
安卓相关的改动只落在这个分支，成熟后再合回。

当前状态：**P0 代码就位、P2 布局可在桌面预览**。分支里已有目录解析、Android 启动环境、
APK 清单模板、构建脚本、手机布局；但还没在真机上跑过——这台开发机没有 Android SDK/NDK，
APK 编译验证要在有 SDK 的机器上做。

手机布局可以在桌面直接看：

```bash
AUDACIOUS_RS_DATA_DIR=~/.config cargo +stable run -p audacious-rs
# 窗口拉到 430×900，或 设置 → 手机布局 打勾
```

![手机布局预览](screenshot-mobile-layout.png)

## 一、代码层面已经铺好的东西

| 项 | 位置 | 说明 |
|---|---|---|
| 数据目录统一 | `audcore::config::data_dir()` | 原来 9 处各写一份 `dirs::config_dir().unwrap_or(".").join(APP_NAME)`；现在统一走这个函数。它先看环境变量 `AUDACIOUS_RS_DATA_DIR`，再看 `dirs::config_dir()`，最后退回当前目录。Android 上 `dirs` 拿不到路径，启动时把 App 私有目录塞进这个环境变量即可 |
| APK 清单模板 | `android/AndroidManifest.xml` | `NativeActivity` + `android.app.lib_name=audacious_rs`，已声明 INTERNET 与读媒体音频权限 |
| 名称资源 | `android/res/values/strings.xml` | App 名 |
| 构建脚本 | `scripts/build-android.sh` | 检查 target / NDK / cargo-apk，再 `cargo apk build --release`，最后打印 APK 路径与 sha256 |
| Android 启动环境 | `main.rs::init_android_env` | 设置 `AUDACIOUS_RS_DATA_DIR=/data/data/<pkg>/files`，日志改写到 `app.log`；`audtool` 的 TCP 服务在 Android 上不启动 |
| Android 依赖 | `crates/audacious/Cargo.toml` | 目标级依赖：`winit` 开 `android-native-activity`，`cpal` 开 `oboe-shared-stdcxx` |
| 手机布局 | `app.rs::mobile_nav_bar / mobile_command_bar / mobile_play_bar` | 侧栏变顶部可横滑导航（40px 高），菜单栏/工具栏/状态栏收成一条命令条，播放条换封面+三键+满宽进度；窄屏列表只留 序号/标题/时长 三列、行高 34px；`config.mobile_layout` 或窗口 <700px 自动生效 |

已实测（桌面 430×900 窗口）：导航切换、命令条按钮、播放条播放/暂停/切歌、进度点按 seek、
窄屏三列表格都正常；`cargo test` 50 用例全绿。

## 二、还要做的四步

### P0 能编译、能起窗口

1. `crates/audacious/Cargo.toml` 里给 Android 加依赖：
   ```toml
   [target.'cfg(target_os = "android")'.dependencies]
   android-activity = { version = "0.6", features = ["native-activity"] }
   ```
   `eframe` 在 Android 上要走 `android-activity` 后端（winit 的 `android-native-activity` feature）。
2. `main.rs` 在 Android 上先设置数据目录再启动 UI：
   ```rust
   #[cfg(target_os = "android")]
   std::env::set_var("AUDACIOUS_RS_DATA_DIR", android_data_dir());
   ```
   `android_data_dir()` 用 JNI 调 `Context.getFilesDir()`；拿不到就退回 `/data/data/io.github.hhym.audaciousrs/files`。
3. `Cargo.toml` 加 `[package.metadata.android]`（包名 `io.github.hhym.audaciousrs`、`apk_name`、
   `build_targets = ["aarch64-linux-android"]`、清单/资源路径指向 `android/`）。
4. 首次构建后在真机上确认：窗口起来、日志能看到「控制端口」那行。

### P1 音频能出声

- 现在输出走 `cpal`（Linux 是 ALSA）。Android 上 `cpal` 要用 oboe 后端：
  ```toml
  cpal = { version = "0.15", features = ["oboe-shared-stdcxx"] }
  ```
- 关掉 `audtool` 的 TCP 控制端口（Android 上没意义，还白白占端口）；`ipc.rs` 加 `cfg(not(target_os = "android"))`。
- 音频焦点：电话进来要暂停，接 `AAudioStream` 的焦点回调（后续做）。
- 文件选择：桌面用的 `rfd` 在 Android 上走 SAF（`ACTION_OPEN_DOCUMENT`），要换成 JNI 调 Intent，再用 `ContentResolver` 打开 fd 交给 symphonia 的 `MediaSourceStream` 读。

### P2 触屏界面

桌面的 1180×720 布局直接塞进手机不行，需要一套 `mobile` 布局分支：

- 侧栏改成抽屉（顶部一条汉堡按钮，点开覆盖 80% 宽）
- 列表行高从 30px 提到 44px，按钮最小可点面积 44×44
- 播放条固定在底部安全区（避开手势条，读 `WindowInsets`）
- 迷你卡片（5.15.0 那套深色圆角卡片）改成底部常驻条，宽度铺满
- 歌词页字号按屏幕宽度算，不用固定 28px

### P3 打包发布

- 用 `cargo apk` 出 `arm64-v8a` 单架构 APK，再用 `apksigner` 签一个自签证书
- GitHub Release 加一个 `rust-audacious-<ver>-android-arm64.apk`，跟桌面三件套并列
- 先在真机（Android 10+）验证：本地 MP3 播放、内嵌封面、歌词、网络音源插件（`boa` 那套 JS 宿主在 arm64 上要重跑一遍用例）

## 三、已知坑（先记下来）

1. **字体**：桌面版是运行时从系统找 CJK 字体，Android 上字体路径不同（`/system/fonts/NotoSansCJK-Regular.ttc`），找不到就会满屏方块——要在 `theme::cjk_candidates()` 里补 Android 路径。
2. **插件线程栈**：`scan_async` 用 16MB 栈跑插件扫描，Android 主线程栈小，必须在后台线程里跑，别顺手挪到主线程。
3. **boa 引擎**：纯 Rust，能编到 arm64；但 `reqwest` 在 Android 上要带 rustls（不能依赖系统 OpenSSL）。
4. **HTTP 白名单**：插件 fetch 的域名白名单逻辑不变，但 Android 上要先声明 INTERNET 权限（清单里已加）。
5. **存储权限**：Android 13 起读本地音乐要 `READ_MEDIA_AUDIO`，别再用老的 `READ_EXTERNAL_STORAGE`。

## 四、分支工作流

```bash
git checkout android
# 改代码 → 桌面 cargo test 仍然要绿（不破坏 main 的功能）
cargo +stable test
./scripts/build-android.sh          # 需要 NDK + cargo-apk
git push origin android
```

桌面发布照旧走 `main`：`./scripts/package-windows.sh` + `git archive` + `gh release create`。
安卓包等 P3 再加进 Release 资产。
