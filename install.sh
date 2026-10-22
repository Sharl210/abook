#!/bin/sh
# abook 引导安装脚本。
#
# 用途：在一台还没装 abook 的机器上，一条命令装好并能裸名调用。
# 它会自己识别平台与架构，下载对应产物，放进 PATH。
#
#   curl -fsSL https://raw.githubusercontent.com/Sharl210/abook/main/install.sh | sh
#
# 可选环境变量：
#   ABOOK_BASE_URL  自定义下载基址（默认用本仓库 latest release）
#   ABOOK_BIN_DIR   安装目录（默认先去已有的可写 PATH 目录，否则 ~/.local/bin）

set -eu

REPO="Sharl210/abook"
BASE="${ABOOK_BASE_URL:-https://github.com/${REPO}/releases/latest/download}"

say() { printf '%s\n' "$*"; }
die() { printf '错误: %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- 识别平台
os=$(uname -s 2>/dev/null || echo unknown)
arch=$(uname -m 2>/dev/null || echo unknown)

case "$os" in
  Linux)
    # Android 的 uname -s 也是 Linux，靠 /system/build.prop 或 getprop 区分
    if [ -f /system/build.prop ] || command -v getprop >/dev/null 2>&1; then
      os=android
    fi
    ;;
  Darwin) os=macos ;;
  MINGW*|MSYS*|CYGWIN*) os=windows ;;
  *) ;;
esac

case "$arch" in
  x86_64|amd64) arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  armv7l|armv7|arm) arch=arm ;;
  i386|i486|i586|i686) arch=x86 ;;
  riscv64) arch=riscv64 ;;
  loongarch64|loongarch) arch=loongarch64 ;;
  *) die "无法识别的架构: $arch" ;;
esac

if [ "$os" = android ]; then
  abi=$(getprop ro.product.cpu.abi 2>/dev/null || echo "")
  case "$abi" in
    arm64-v8a) arch=aarch64 ;;
    armeabi-v7a|armeabi) arch=arm ;;
    x86_64) arch=x86_64 ;;
    x86) arch=x86 ;;
  esac
fi

case "$os/$arch" in
  android/aarch64)    target=aarch64-linux-android ;;
  android/arm)        target=armv7-linux-androideabi ;;
  android/x86_64)     target=x86_64-linux-android ;;
  android/x86)        target=i686-linux-android ;;
  linux/aarch64)      target=aarch64-unknown-linux-musl ;;
  linux/x86_64)       target=x86_64-unknown-linux-musl ;;
  linux/arm)          target=armv7-unknown-linux-musleabihf ;;
  linux/x86)          target=i686-unknown-linux-musl ;;
  linux/riscv64)      target=riscv64gc-unknown-linux-musl ;;
  linux/loongarch64)  target=loongarch64-unknown-linux-gnu ;;
  windows/x86_64)     target=x86_64-pc-windows-gnu.exe ;;
  windows/x86)        target=i686-pc-windows-gnu.exe ;;
  windows/aarch64)    target=aarch64-pc-windows-gnullvm.exe ;;
  macos/x86_64)       target=x86_64-apple-darwin ;;
  macos/aarch64)      target=aarch64-apple-darwin ;;
  *) die "不支持的平台组合: $os/$arch" ;;
esac

[ "$os" = windows ] && name="abook-$target" || name="abook-$target"

say "平台 $os/$arch  →  $name"
say "来源 $BASE/$name"

# ---------------------------------------------------------------- 选安装目录
pick_dir() {
  if [ -n "${ABOOK_BIN_DIR:-}" ]; then printf '%s' "$ABOOK_BIN_DIR"; return; fi
  # 优先复用已在 PATH 里且可写的目录，避免要求用户改 PATH
  old_ifs=$IFS; IFS=:
  for d in $PATH; do
    case "$d" in
      /data/data/*/files/usr/bin|*/usr/local/bin|*/bin)
        if [ -w "$d" ] 2>/dev/null; then IFS=$old_ifs; printf '%s' "$d"; return; fi ;;
    esac
  done
  IFS=$old_ifs
  printf '%s' "${HOME:-/tmp}/.local/bin"
}

BIN_DIR=$(pick_dir)
mkdir -p "$BIN_DIR" 2>/dev/null || die "无法创建目录 $BIN_DIR"

# ---------------------------------------------------------------- 下载
tmp=$(mktemp "${TMPDIR:-/tmp}/abook.XXXXXX" 2>/dev/null || echo "/tmp/abook.$$")
cleanup() { rm -f "$tmp" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

say "下载到 $tmp"
if command -v curl >/dev/null 2>&1; then
  curl -fL --retry 5 --retry-all-errors --retry-delay 3 -o "$tmp" "$BASE/$name" \
    || die "下载失败。请检查网络，或用 ABOOK_BASE_URL 指定其它来源。"
elif command -v wget >/dev/null 2>&1; then
  wget -O "$tmp" "$BASE/$name" || die "下载失败"
else
  die "找不到 curl 或 wget"
fi

dest="$BIN_DIR/$name"
case "$name" in
  abook-*.exe) final="$BIN_DIR/abook.exe" ;;
  *)           final="$BIN_DIR/abook" ;;
esac
mv "$tmp" "$final" || die "无法写入 $final"
chmod +x "$final" 2>/dev/null || true

say "已安装 $final"

# ---------------------------------------------------------------- 验证
if "$final" version >/dev/null 2>&1; then
  say "版本 $("$final" version 2>/dev/null | head -1)"
fi

case ":$PATH:" in
  *":$BIN_DIR:"*)
    say "PATH 已包含 $BIN_DIR，可直接运行: abook detect" ;;
  *)
    say ""
    say "注意：$BIN_DIR 不在 PATH 里。请把下面一行加入 shell 配置："
    say "  export PATH=\"$BIN_DIR:\$PATH\""
    say "然后重开终端，运行: abook detect" ;;
esac
