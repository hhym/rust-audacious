#!/usr/bin/env bash
# 安卓构建入口（分支 android 专用）。桌面打包仍走 package-windows.sh。
# 依赖：rustup target add aarch64-linux-android、NDK r26+、cargo-apk 或 gradle 模板。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${ANDROID_TARGET:-aarch64-linux-android}"
APK_NAME="audacious-rs"

need() { command -v "$1" >/dev/null 2>&1 || { echo "缺少 $1，$2"; exit 1; }; }

need rustup "先装 rustup"
rustup target list --installed | grep -qx "$TARGET" || {
    echo "未安装 target $TARGET，执行：rustup target add $TARGET"
    exit 1
}
[[ -n "${ANDROID_NDK_HOME:-}" ]] || { echo "请先设 ANDROID_NDK_HOME"; exit 1; }
need cargo-apk "执行：cargo install cargo-apk"

echo "==> 构建 $TARGET（release）"
cargo apk build --release --target "$TARGET"

OUT="$ROOT/target/release/apk/$APK_NAME.apk"
if [[ -f "$OUT" ]]; then
    echo "==> APK: $OUT"
    sha256sum "$OUT"
else
    echo "没找到 $OUT，看 cargo apk 的输出目录"
    exit 1
fi
