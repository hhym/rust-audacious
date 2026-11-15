#!/usr/bin/env bash
# 安卓构建入口（分支 android 专用）。桌面打包仍走 package-windows.sh。
#
# 不走 cargo-apk（它在 workspace 根清单上解析失败），整条链路只用 SDK / NDK 自带工具：
#   cargo build → aapt2 compile/link → 塞 native 库 → zipalign → apksigner
#
# 依赖：
#   rustup target add aarch64-linux-android
#   ANDROID_HOME  装好 platforms/android-34、build-tools/34.0.0、ndk/26.x 的 SDK
#   ANDROID_NDK_HOME  可选，缺省取 $ANDROID_HOME/ndk 下版本号最大的那个
#   keytool（JDK 17 自带）
#
# 用法：./scripts/build-android.sh [--debug]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${ANDROID_TARGET:-aarch64-linux-android}"
API_LEVEL="${ANDROID_API_LEVEL:-26}"
PROFILE="release"
CARGO_PROFILE_FLAG="--release"
if [[ "${1:-}" == "--debug" ]]; then
    PROFILE="debug"
    CARGO_PROFILE_FLAG=""
fi

need() { command -v "$1" >/dev/null 2>&1 || { echo "缺少 $1，$2"; exit 1; }; }

need rustup "先装 rustup"
rustup target list --installed | grep -qx "$TARGET" || {
    echo "未安装 target $TARGET，执行：rustup target add $TARGET"
    exit 1
}
[[ -n "${ANDROID_HOME:-}" ]] || { echo "请先设 ANDROID_HOME"; exit 1; }

NDK="${ANDROID_NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)}"
[[ -n "$NDK" && -d "$NDK" ]] || { echo "在 $ANDROID_HOME/ndk 下没找到 NDK"; exit 1; }

BUILD_TOOLS="$ANDROID_HOME/build-tools/${BUILD_TOOLS_VERSION:-34.0.0}"
[[ -d "$BUILD_TOOLS" ]] || BUILD_TOOLS="$(ls -d "$ANDROID_HOME"/build-tools/* 2>/dev/null | sort -V | tail -1)"
PLATFORM="$ANDROID_HOME/platforms/android-${ANDROID_PLATFORM:-34}"
[[ -f "$PLATFORM/android.jar" ]] || PLATFORM="$(ls -d "$ANDROID_HOME"/platforms/android-* 2>/dev/null | sort -V | tail -1)"

for t in aapt2 zipalign apksigner; do need "$BUILD_TOOLS/$t" "SDK build-tools 里没有 $t"; done

TC="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
[[ -d "$TC" ]] || { echo "NDK 工具链不在 $TC"; exit 1; }
export CC_aarch64_linux_android="$TC/${TARGET}${API_LEVEL}-clang"
export CXX_aarch64_linux_android="$TC/${TARGET}${API_LEVEL}-clang++"
export AR_aarch64_linux_android="$TC/llvm-ar"
export RANLIB_aarch64_linux_android="$TC/llvm-ranlib"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
# Android 15 起 16KB 页设备要求 .so 的 LOAD 段按 16KB 对齐（NDK r26 默认还是 4KB）
export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384"

echo "==> 构建 $TARGET（$PROFILE）"
(cd "$ROOT" && cargo build $CARGO_PROFILE_FLAG --target "$TARGET" -p audacious-rs)

SO="$ROOT/target/$TARGET/$PROFILE/libaudacious_rs.so"
[[ -f "$SO" ]] || { echo "没找到 $SO"; exit 1; }

WORK="$ROOT/target/android-apk"
rm -rf "$WORK" && mkdir -p "$WORK/lib/arm64-v8a"

echo "==> 剥离符号并放入 lib/arm64-v8a"
"$TC/llvm-strip" --strip-unneeded -o "$WORK/lib/arm64-v8a/libaudacious_rs.so" "$SO"
# oboe（cpal 的 Android 后端）用共享 C++ 运行库
LIBCXX="$NDK/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
[[ -f "$LIBCXX" ]] && cp "$LIBCXX" "$WORK/lib/arm64-v8a/" && "$TC/llvm-strip" --strip-unneeded "$WORK/lib/arm64-v8a/libc++_shared.so"

echo "==> aapt2 编译资源"
"$BUILD_TOOLS/aapt2" compile --dir "$ROOT/android/res" -o "$WORK/res.zip"
"$BUILD_TOOLS/aapt2" link -o "$WORK/base.apk" \
    -I "$PLATFORM/android.jar" \
    --manifest "$ROOT/android/AndroidManifest.xml" \
    -R "$WORK/res.zip" \
    --auto-add-overlay

echo "==> 塞入 native 库"
(cd "$WORK" && zip -q -r base.apk lib)

echo "==> zipalign + 签名"
"$BUILD_TOOLS/zipalign" -f -p 4 "$WORK/base.apk" "$WORK/aligned.apk"

KS="${ANDROID_DEBUG_KEYSTORE:-$HOME/.android/debug.keystore}"
if [[ ! -f "$KS" ]]; then
    mkdir -p "$(dirname "$KS")"
    keytool -genkeypair -keystore "$KS" -storepass android -alias androiddebugkey \
        -keypass android -keyalg RSA -keysize 2048 -validity 10000 \
        -dname "CN=Android Debug,O=Android,C=US" >/dev/null 2>&1
fi
"$BUILD_TOOLS/apksigner" sign --ks "$KS" --ks-pass pass:android --key-pass pass:android \
    --min-sdk-version "$API_LEVEL" --out "$WORK/audacious-rs.apk" "$WORK/aligned.apk"
"$BUILD_TOOLS/apksigner" verify --print-certs "$WORK/audacious-rs.apk" | head -3

VER="$(grep -m1 '^version = ' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
OUT="$ROOT/dist/rust-audacious-${VER}-android-arm64.apk"
cp "$WORK/audacious-rs.apk" "$OUT"
echo "==> APK: $OUT"
ls -lh "$OUT"
sha256sum "$OUT"
