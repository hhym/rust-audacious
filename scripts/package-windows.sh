#!/usr/bin/env bash
# 交叉编译 Windows x64 运行包（在 Linux / macOS 上执行）
# 依赖：rustup target add x86_64-pc-windows-gnu，以及 mingw-w64 的 x86_64-w64-mingw32-gcc
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION=$(grep -m1 'version = ' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
TARGET=x86_64-pc-windows-gnu
NAME="rust-audacious-${VERSION}-windows-x64"

echo "==> 编译 $TARGET（release）"
cargo build --release --target "$TARGET"

echo "==> 组装 $NAME"
rm -rf "dist/$NAME" "dist/$NAME.zip"
mkdir -p "dist/$NAME"
cp "target/$TARGET/release/audacious-rs.exe" "dist/$NAME/"
cp "target/$TARGET/release/audtool.exe" "dist/$NAME/"
cp LICENSE "dist/$NAME/"
cp dist/README.txt "dist/$NAME/" 2>/dev/null || true
cp -r docs "dist/$NAME/"

echo "==> 打包"
(cd dist && zip -q -r "$NAME.zip" "$NAME")
sha256sum "dist/$NAME.zip" | tee "dist/$NAME.zip.sha256"
echo "完成： dist/$NAME.zip"
