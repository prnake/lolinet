#!/bin/sh
# 编 lolinet：拉 easytier/build.env 指定的 EasyTier，按顺序打 EASYTIER_PATCHES 和 LOLINET_PATCHES，
# 把 lolinet/src 放进 easytier 包当第三个 bin（和 easytier-core / easytier-cli 同一份依赖和 Cargo.lock）。
# 用法: sh lolinet/build.sh            本机目标；TARGET=x86_64-apple-darwin FEATURES=jemalloc 交叉编
# 产物: .build/out/lolinet-<macos|linux>-<aarch64|x86_64>.tar.gz（根目录 lolinet + BUILD-INFO，和 easytier 包同一种格式）
set -eu
cd "$(dirname "$0")/.."
ROOT=$(pwd)
. easytier/build.env
SRC=${LOLINET_SRC:-$ROOT/.build/easytier}
TARGET=${TARGET:-}
# 和 easytier.yml 一致：aarch64 用 mimalloc，x86_64 用 jemalloc
case "${TARGET:-$(uname -m)}" in *aarch64*|*arm64*) ARCH=aarch64; FEATURES=${FEATURES:-mimalloc} ;; *) ARCH=x86_64; FEATURES=${FEATURES:-jemalloc} ;; esac
case "${TARGET:-$(uname -s)}" in *darwin*|*Darwin*) OS=macos ;; *) OS=linux ;; esac

[ -d "$SRC/.git" ] || git clone -q --depth 1 --branch "$EASYTIER_REF" https://github.com/EasyTier/EasyTier.git "$SRC"
[ "$(git -C "$SRC" rev-parse HEAD)" = "$EASYTIER_SHA" ] || { echo "$SRC 不在 $EASYTIER_SHA（$EASYTIER_REF）"; exit 1; }
# 每次从干净的上游打补丁；target/ 留着做增量编译
git -C "$SRC" checkout -q -- . && git -C "$SRC" clean -qfd -e target
for p in $EASYTIER_PATCHES $LOLINET_PATCHES; do git -C "$SRC" apply "$ROOT/$p"; done
command rm -rf "$SRC/easytier/src/lolinet" && command cp -R lolinet/src "$SRC/easytier/src/lolinet"

VERSION=$(git describe --always --dirty 2>/dev/null || echo dev)
(cd "$SRC" && LOLINET_VERSION=$VERSION cargo build --release --locked ${TARGET:+--target "$TARGET"} \
  -p easytier --features "$FEATURES" --bin lolinet)

B=$SRC/target/${TARGET:+$TARGET/}release/lolinet
PKG=$ROOT/.build/pkg; OUT=$ROOT/.build/out
command rm -rf "$PKG" && mkdir -p "$PKG" "$OUT"
command cp -f "$B" "$PKG/lolinet"
{
  echo "lolinet $VERSION"
  echo "easytier $EASYTIER_REF ($EASYTIER_SHA)"
  echo "patches: $EASYTIER_PATCHES $LOLINET_PATCHES"
  echo "target: ${TARGET:-native $(uname -m)} features: $FEATURES"
} > "$PKG/BUILD-INFO"
COPYFILE_DISABLE=1 tar -czf "$OUT/lolinet-$OS-$ARCH.tar.gz" -C "$PKG" lolinet BUILD-INFO
ls -la "$B"; shasum -a 256 "$OUT/lolinet-$OS-$ARCH.tar.gz"
