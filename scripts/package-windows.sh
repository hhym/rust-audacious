#!/usr/bin/env bash
# 交叉编译 Windows x64 运行包（在 Linux / macOS 上执行）
# 依赖：rustup target add x86_64-pc-windows-gnu，以及 mingw-w64 的 x86_64-w64-mingw32-gcc
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION=$(grep -m1 'version = ' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
TARGET=x86_64-pc-windows-gnu
NAME="rust-audacious-${VERSION}-windows-x64"

# 有的依赖（stacker）带 C 代码，交叉编译要用 mingw 的 clang 当 CC
if [ -z "${CC_x86_64_pc_windows_gnu:-}" ] && command -v x86_64-w64-mingw32-clang >/dev/null 2>&1; then
  export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-clang
  export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
fi

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

# 可选：Mesa 软件 OpenGL 兜底（老显卡用）
MESA_DIR="${MESA_DIR:-$HOME/.cache/mesa-dist-win}"
if [ -f "$MESA_DIR/opengl32.dll" ] && [ -f "$MESA_DIR/libgallium_wgl.dll" ]; then
  mkdir -p "dist/$NAME/mesa"
  cp "$MESA_DIR/opengl32.dll" "$MESA_DIR/libgallium_wgl.dll" "dist/$NAME/mesa/"
  cp packaging/mesa-README.txt "dist/$NAME/mesa/README.txt"
  cp packaging/use-software-gl.bat packaging/restore-hardware-gl.bat "dist/$NAME/"
else
  echo "!! 找不到 $MESA_DIR/opengl32.dll，跳过 Mesa 软件渲染目录"
fi

# 可选：FFmpeg 解码插件（APE/WMA/DSD 等格式）
FFMPEG_DIR="${FFMPEG_DIR:-$HOME/.cache/ffmpeg-win}"
if [ -f "$FFMPEG_DIR/ffmpeg.exe" ]; then
  mkdir -p "dist/$NAME/ffmpeg"
  cp "$FFMPEG_DIR/ffmpeg.exe" "dist/$NAME/ffmpeg/"
  cp packaging/ffmpeg-README.txt "dist/$NAME/ffmpeg/README.txt"
else
  echo "!! 找不到 $FFMPEG_DIR/ffmpeg.exe，跳过 FFmpeg 解码插件"
fi

echo "==> 打包"
(cd dist && zip -q -r "$NAME.zip" "$NAME")
sha256sum "dist/$NAME.zip" | tee "dist/$NAME.zip.sha256"
echo "完成： dist/$NAME.zip"
