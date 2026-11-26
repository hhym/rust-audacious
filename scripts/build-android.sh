#!/usr/bin/env bash
# 安卓构建入口（分支 android 专用）。桌面打包仍走 package-windows.sh。
#
# 不走 cargo-apk（它在 workspace 根清单上解析失败），整条链路只用 SDK / NDK 自带工具：
#   cargo build → aapt2 compile/link → 塞 native 库 → zipalign → apksigner
#
# 依赖：
#   rustup target add aarch64-linux-android（32 位再 add armv7-linux-androideabi）
#   ANDROID_HOME  装好 platforms/android-34、build-tools/34.0.0、ndk/26.x 的 SDK
#   ANDROID_NDK_HOME  可选，缺省取 $ANDROID_HOME/ndk 下版本号最大的那个
#   keytool（JDK 17 自带）
#
# 用法：./scripts/build-android.sh [--debug]
#       ANDROID_ABI=armeabi-v7a ./scripts/build-android.sh   # 32 位 ARM 机器
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# ABI 选择：arm64-v8a（默认）或 armeabi-v7a（老的 32 位机器，比如 8.1 的国产平板/车机）
ABI="${ANDROID_ABI:-arm64-v8a}"
case "$ABI" in
    arm64-v8a)
        TARGET="${ANDROID_TARGET:-aarch64-linux-android}"
        CLANG_PREFIX="aarch64-linux-android"
        LIBDIR="aarch64-linux-android"
        TRIPLE_ENV="AARCH64_LINUX_ANDROID"
        OUT_ARCH="arm64"
        ;;
    armeabi-v7a)
        TARGET="${ANDROID_TARGET:-armv7-linux-androideabi}"
        CLANG_PREFIX="armv7a-linux-androideabi"
        LIBDIR="arm-linux-androideabi"
        TRIPLE_ENV="ARMV7_LINUX_ANDROIDEABI"
        OUT_ARCH="armv7"
        ;;
    *)
        echo "不支持的 ANDROID_ABI：$ABI（可选 arm64-v8a / armeabi-v7a）"
        exit 1
        ;;
esac
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

for t in aapt2 zipalign apksigner d8; do need "$BUILD_TOOLS/$t" "SDK build-tools 里没有 $t"; done
need javac "装 JDK 17，并把 bin 放进 PATH"
need java "装 JDK 17，并把 bin 放进 PATH"

TC="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
[[ -d "$TC" ]] || { echo "NDK 工具链不在 $TC"; exit 1; }
CC_BIN="$TC/${CLANG_PREFIX}${API_LEVEL}-clang"
CXX_BIN="$TC/${CLANG_PREFIX}${API_LEVEL}-clang++"
export "CC_${TRIPLE_ENV,,}=$CC_BIN"
export "CXX_${TRIPLE_ENV,,}=$CXX_BIN"
export "AR_${TRIPLE_ENV,,}=$TC/llvm-ar"
export "RANLIB_${TRIPLE_ENV,,}=$TC/llvm-ranlib"
export "CARGO_TARGET_${TRIPLE_ENV}_LINKER=$CC_BIN"
# Android 15 起 16KB 页设备要求 .so 的 LOAD 段按 16KB 对齐（NDK r26 默认还是 4KB）
if [[ "$ABI" == "arm64-v8a" ]]; then
    export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384"
fi

echo "==> 构建 $TARGET / $ABI（$PROFILE）"
(cd "$ROOT" && cargo build $CARGO_PROFILE_FLAG --target "$TARGET" -p audacious-rs)

SO="$ROOT/target/$TARGET/$PROFILE/libaudacious_rs.so"
[[ -f "$SO" ]] || { echo "没找到 $SO"; exit 1; }

WORK="$ROOT/target/android-apk-$ABI"
rm -rf "$WORK" && mkdir -p "$WORK/lib/$ABI"

echo "==> 剥离符号并放入 lib/$ABI"
"$TC/llvm-strip" --strip-unneeded -o "$WORK/lib/$ABI/libaudacious_rs.so" "$SO"
# oboe（cpal 的 Android 后端）用共享 C++ 运行库
LIBCXX="$NDK/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/$LIBDIR/libc++_shared.so"
[[ -f "$LIBCXX" ]] && cp "$LIBCXX" "$WORK/lib/$ABI/" && "$TC/llvm-strip" --strip-unneeded "$WORK/lib/$ABI/libc++_shared.so"

echo "==> aapt2 编译资源"
"$BUILD_TOOLS/aapt2" compile --dir "$ROOT/android/res" -o "$WORK/res.zip"
"$BUILD_TOOLS/aapt2" link -o "$WORK/base.apk" \
    -I "$PLATFORM/android.jar" \
    --manifest "$ROOT/android/AndroidManifest.xml" \
    -R "$WORK/res.zip" \
    --auto-add-overlay

echo "==> 编译 Java 壳（MainActivity，转发 onActivityResult）"
rm -rf "$WORK/classes" "$WORK/dex"
mkdir -p "$WORK/classes" "$WORK/dex"
javac --release 8 -classpath "$PLATFORM/android.jar" -d "$WORK/classes" \
    $(find "$ROOT/android/java" -name '*.java')
"$BUILD_TOOLS/d8" --lib "$PLATFORM/android.jar" --min-api "$API_LEVEL" \
    --output "$WORK/dex" $(find "$WORK/classes" -name '*.class')

echo "==> 塞入 classes.dex 与 native 库"
cp "$WORK/dex/classes.dex" "$WORK/classes.dex"
(cd "$WORK" && zip -q -r base.apk classes.dex lib)

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
OUT="$ROOT/dist/rust-audacious-${VER}-android-${OUT_ARCH}.apk"
cp "$WORK/audacious-rs.apk" "$OUT"
echo "==> APK: $OUT"
ls -lh "$OUT"
sha256sum "$OUT"
