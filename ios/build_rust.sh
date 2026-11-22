#!/usr/bin/env bash
# iOS 宿主构建脚本：cargo 构建 Rust 可执行文件，并拷入 Xcode 产物目录。
# 由 Xcode（XcodeGen 生成的 post-compile 脚本阶段）调用。
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

if [ "${PLATFORM_NAME:-}" = "iphonesimulator" ]; then
  CARGO_TARGET="aarch64-apple-ios-sim"
else
  CARGO_TARGET="aarch64-apple-ios"
fi

# 仓库根定位：Xcode 下 SRCROOT 指向本目录（ios/），向上查找 Cargo.toml 得到仓库根；
# 本地手动执行时回退到脚本所在目录的上级。
REPO_ROOT="${SRCROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
while [ ! -f "$REPO_ROOT/Cargo.toml" ] && [ "$REPO_ROOT" != "/" ]; do
  REPO_ROOT="$(dirname "$REPO_ROOT")"
done

cd "$REPO_ROOT"
cargo build --release --bin zplayer-music --target "$CARGO_TARGET" --no-default-features --features mobile

mkdir -p "$(dirname "$TARGET_BUILD_DIR/$EXECUTABLE_PATH")"
cp "$REPO_ROOT/target/$CARGO_TARGET/release/zplayer-music" "$TARGET_BUILD_DIR/$EXECUTABLE_PATH"
