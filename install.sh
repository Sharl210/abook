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

REPO="${ABOOK_REPO:-Sharl210/abook}"
BASE="${ABOOK_BASE_URL:-https://github.com/${REPO}/releases/latest/download}"
# Private repositories cannot be downloaded from github.com/releases/download at
# all -- that endpoint ignores an Authorization header and answers 404. The API
# asset endpoint is the only working path, and it needs a token.
TOKEN="${ABOOK_TOKEN:-${GITHUB_TOKEN:-${GH_TOKEN:-}}}"

say() { printf '%s\n' "$*"; }
die() { printf '错误: %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- 纯映射函数
# These two exist so the mappings can be tested without touching the network,
# the filesystem or the real machine: `--self-check` feeds them fixed inputs.
# Every value below was chosen after a real failure or a real near-miss.
norm_arch() {
  case "$1" in
    x86_64|amd64) arch=x86_64 ;;
    aarch64|arm64) arch=aarch64 ;;
    # armv8l is what `uname -m` prints on a 32-bit userland running on an ARMv8
    # chip (older Android, 32-bit distros); armv7a and armv8 show up in kernel
    # and proot strings. Missing these made the script die with
    # "无法识别的架构" on hardware that runs the armv7 build perfectly well.
    armv7l|armv7|armv7a|armv8l|armv8|arm) arch=arm ;;
    i386|i486|i586|i686) arch=x86 ;;
    riscv64) arch=riscv64 ;;
    loongarch64|loongarch) arch=loongarch64 ;;
    *) return 1 ;;
  esac
}
norm_os() {
  case "$1" in
    Linux) os=linux ;;
    # Android's uname -s is also Linux; the caller distinguishes by build.prop.
    Darwin) os=macos ;;
    MINGW*|MSYS*|CYGWIN*) os=windows ;;
    *) return 1 ;;
  esac
}
# os/arch -> release asset suffix. Must stay in sync with the CI matrix; the
# repo's check_workflow.py cross-checks that every name here is really built.
target_for() {
  case "$1/$2" in
    android/aarch64)    target=aarch64-linux-android ;;
    android/arm)        target=armv7-linux-androideabi ;;
    android/x86_64)     target=x86_64-linux-android ;;
    android/x86)        target=i686-linux-android ;;
    linux/aarch64)      target=aarch64-unknown-linux-musl ;;
    linux/x86_64)       target=x86_64-unknown-linux-musl ;;
    linux/arm)          target=armv7-unknown-linux-musleabihf ;;
    linux/x86)          target=i686-unknown-linux-musl ;;
    # gnu, not musl: the matrix builds riscv64gc-unknown-linux-gnu (Ubuntu
    # ships the cross gcc, and real riscv64 machines run glibc). The musl
    # flavour is no longer produced by the workflow.
    linux/riscv64)      target=riscv64gc-unknown-linux-gnu ;;
    linux/loongarch64)  target=loongarch64-unknown-linux-gnu ;;
    windows/x86_64)     target=x86_64-pc-windows-gnu.exe ;;
    windows/x86)        target=i686-pc-windows-gnu.exe ;;
    windows/aarch64)    target=aarch64-pc-windows-gnullvm.exe ;;
    macos/x86_64)       target=x86_64-apple-darwin ;;
    macos/aarch64)      target=aarch64-apple-darwin ;;
    *) return 1 ;;
  esac
}

self_check() {
  bad=0
  t() {  # t <kind> <input> <expected>
    case "$1" in
      arch) norm_arch "$2" || { printf '  FAIL arch %-14s 无法识别\n' "$2"; bad=$((bad+1)); return; } ;;
      os)   norm_os   "$2" || { printf '  FAIL os   %-14s 无法识别\n' "$2"; bad=$((bad+1)); return; } ;;
    esac
    got_var=$arch; [ "$1" = os ] && got_var=$os
    if [ "$got_var" != "$3" ]; then
      printf '  FAIL %-4s %-14s 得到 %s，应为 %s\n' "$1" "$2" "$got_var" "$3"
      bad=$((bad+1))
    else
      printf '  ok   %-4s %-14s -> %s\n' "$1" "$2" "$got_var"
    fi
  }
  t arch aarch64 aarch64
  t arch arm64 aarch64
  # The one that used to abort on 32-bit userlands.
  t arch armv8l arm
  t arch armv7l arm
  t arch armv7a arm
  t arch armv8 arm
  t arch x86_64 x86_64
  t arch amd64 x86_64
  t arch i686 x86
  t arch riscv64 riscv64
  t arch loongarch64 loongarch64
  t os Linux linux
  t os Darwin macos
  t os MINGW64_NT-10.0 windows
  t os MSYS_NT-10.0 windows
  # Every platform/arch pair must resolve to the EXPECTED asset name, not just
  # to something. Asserting only that a name came back let a wrong name pass:
  # switching riscv64 back to the pruned -musl flavour kept this check green
  # while `install` would 404. The expected value is written out per pair.
  tp() {  # tp <os/arch> <expected-suffix>
    if ! target_for "${1%%/*}" "${1#*/}"; then
      printf '  FAIL pair %-22s 无对应产物\n' "$1"
      bad=$((bad+1)); return
    fi
    if [ "$target" != "$2" ]; then
      printf '  FAIL pair %-22s 得到 %s，应为 %s\n' "$1" "$target" "$2"
      bad=$((bad+1)); return
    fi
    printf '  ok   pair %-22s -> abook-%s\n' "$1" "$target"
  }
  tp android/aarch64   aarch64-linux-android
  tp android/arm       armv7-linux-androideabi
  tp android/x86_64    x86_64-linux-android
  tp android/x86       i686-linux-android
  tp linux/aarch64     aarch64-unknown-linux-musl
  tp linux/x86_64      x86_64-unknown-linux-musl
  tp linux/arm         armv7-unknown-linux-musleabihf
  tp linux/x86         i686-unknown-linux-musl
  tp linux/riscv64     riscv64gc-unknown-linux-gnu
  tp linux/loongarch64 loongarch64-unknown-linux-gnu
  tp windows/x86_64    x86_64-pc-windows-gnu.exe
  tp windows/x86       i686-pc-windows-gnu.exe
  tp windows/aarch64   aarch64-pc-windows-gnullvm.exe
  tp macos/x86_64      x86_64-apple-darwin
  tp macos/aarch64     aarch64-apple-darwin
  if [ "$bad" -eq 0 ]; then
    say "install.sh 自检全部通过"
    return 0
  fi
  printf 'install.sh 自检 %d 项失败\n' "$bad" >&2
  return 1
}

# ---------------------------------------------------------------- 识别平台
# `--token` must be parsed BEFORE the download code reads $TOKEN. It was
# documented in the error message and in INSTALL.md while the flag itself did
# not exist: `--token xyz` was silently swallowed as a positional argument, the
# token stayed empty, the private-repo fallback was skipped entirely, and the
# user got "下载失败" with no way to tell why. A flag that is advertised but
# ignored is worse than a missing one, so both are handled here.
ARG_BIN=""
while [ $# -gt 0 ]; do
  case "$1" in
    --self-check) self_check; exit $? ;;
    --token)
      [ $# -ge 2 ] || die "--token 后面要跟凭据"
      TOKEN="$2"; shift 2 ;;
    --token=*) TOKEN="${1#--token=}"; shift ;;
    --bin-dir)
      [ $# -ge 2 ] || die "--bin-dir 后面要跟目录"
      ARG_BIN="$2"; shift 2 ;;
    --bin-dir=*) ARG_BIN="${1#--bin-dir=}"; shift ;;
    -h|--help)
      say "用法: install.sh [选项]"
      say "  --self-check          离线验证平台/架构/产物名映射，不联网、不改文件"
      say "  --token <凭据>        私有仓库下载凭据（也可用 ABOOK_TOKEN 环境变量）"
      say "  --bin-dir <目录>      指定安装目录（默认取 PATH 里第一个可写目录）"
      exit 0 ;;
    *) die "无法识别的参数: $1（用 --help 看用法）" ;;
  esac
done

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
  # armv8l is what `uname -m` prints on a 32-bit userland running on an ARMv8
  # chip (common on older Android and on 32-bit distros); armv7a and armv8
  # appear in kernel/proot strings. Without these the script died with
  # "无法识别的架构" on hardware that runs the armv7 build perfectly well.
  armv7l|armv7|armv7a|armv8l|armv8|arm) arch=arm ;;
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
  # gnu, not musl: the matrix builds riscv64gc-unknown-linux-gnu (Ubuntu ships
  # the cross gcc, and real riscv64 machines run glibc). The musl flavour was
  # never produced by the current workflow; it lingered on the release page as
  # a stale asset and is pruned, so asking for it here would 404 right after.
  linux/riscv64)      target=riscv64gc-unknown-linux-gnu ;;
  linux/loongarch64)  target=loongarch64-unknown-linux-gnu ;;
  windows/x86_64)     target=x86_64-pc-windows-gnu.exe ;;
  windows/x86)        target=i686-pc-windows-gnu.exe ;;
  windows/aarch64)    target=aarch64-pc-windows-gnullvm.exe ;;
  macos/x86_64)       target=x86_64-apple-darwin ;;
  macos/aarch64)      target=aarch64-apple-darwin ;;
  *) die "不支持的平台组合: $os/$arch" ;;
esac

# target already carries the .exe suffix on Windows, so the asset name is
# uniform: abook-<triple>[.exe]. Keeping the two branches distinct would invite
# someone to "simplify" one of them later and break the other platform.
name="abook-$target"

say "平台 $os/$arch  →  $name"
say "来源 $BASE/$name"

# ---------------------------------------------------------------- 选安装目录
pick_dir() {
  # Explicit choice wins. `--bin-dir` is parsed above but had no effect until
  # it was threaded through here: accepting a flag and ignoring it is the same
  # defect class as documenting one that does not exist.
  if [ -n "$ARG_BIN" ]; then printf '%s' "$ARG_BIN"; return; fi
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
# Never assume /tmp: on Android it is often unwritable and mktemp then fails
# silently or falls back to a path that cannot be created either.
tmpdir=""
for d in "$BIN_DIR" "${TMPDIR:-}" "${HOME:-}" "/data/local/tmp" "/tmp"; do
  [ -n "$d" ] || continue
  if [ -d "$d" ] && [ -w "$d" ]; then tmpdir="$d"; break; fi
done
[ -n "$tmpdir" ] || die "找不到可写临时目录"
tmp="$tmpdir/.abook-dl.$$"
cleanup() { rm -f "$tmp" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

# Probe whether the bytes we got are really an executable. An HTTP 200 is not
# proof: a captive portal or an HTML error page also arrives with a status.
is_binary() {
  [ -s "$1" ] || return 1
  head -c 4 "$1" | od -An -tx1 | tr -d ' \n' | grep -qiE '^(7f454c46|4d5a|cffaedfe|feedfacf|cafebabe)$'
}

say "下载到 $tmp"
ok=0
if command -v curl >/dev/null 2>&1; then
  if curl -fsSL --retry 5 --retry-all-errors --retry-delay 3 -o "$tmp" "$BASE/$name" 2>/dev/null \
     && is_binary "$tmp"; then
    ok=1
  fi
elif command -v wget >/dev/null 2>&1; then
  if wget -q -O "$tmp" "$BASE/$name" 2>/dev/null && is_binary "$tmp"; then ok=1; fi
else
  die "找不到 curl 或 wget"
fi

# Private repositories: github.com/releases/download answers 404 no matter what
# header you send, so fall back to the API asset endpoint which does honour it.
if [ "$ok" -ne 1 ] && [ -n "$TOKEN" ]; then
  say "直接下载不可用，改用 API（私有仓库路径）"
  if command -v curl >/dev/null 2>&1; then
    # Extract the asset id whose "name" equals $name. The whitespace around the
    # colon varies between minified and pretty-printed JSON, so match with ` *`
    # instead of assuming a single space. `|| true` matters: under `set -e` a
    # non-matching grep would abort the whole script with no explanation.
    rel=$(curl -fsSL \
            -H "Authorization: Bearer $TOKEN" \
            -H "Accept: application/vnd.github+json" \
            "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null) || rel=""
    # The release JSON is pretty-printed: `"id"` and `"name"` sit on
    # different lines, so they cannot be matched together in one line. Within
    # each asset object `id` always precedes `name`, so scan line by line and
    # emit the id remembered just before the wanted name.
    aid=$(printf '%s' "$rel" | awk -v want="$name" '
      {
        if (match($0, /"id": *[0-9]+/)) {
          seg = substr($0, RSTART, RLENGTH)
          gsub(/[^0-9]/, "", seg)
          cur = seg
        }
        if (match($0, /"name": *"[^"]*"/)) {
          seg = substr($0, RSTART, RLENGTH)
          sub(/.*"name": *"/, "", seg)
          sub(/"$/, "", seg)
          if (seg == want) { print cur; exit }
        }
      }') || aid=""
    if [ -n "$aid" ]; then
      if curl -fsSL --retry 3 -o "$tmp" \
           -H "Authorization: Bearer $TOKEN" \
           -H "Accept: application/octet-stream" \
           "https://api.github.com/repos/$REPO/releases/assets/$aid" 2>/dev/null \
         && is_binary "$tmp"; then
        ok=1
      fi
    fi
  fi
fi

if [ "$ok" -ne 1 ]; then
  rm -f "$tmp"
  die "下载失败或产物不是可执行文件。
  目标产物: $name
  试过来源: $BASE
做法:
  1) 私有仓库需设置 ABOOK_TOKEN（或 GITHUB_TOKEN）
  2) 或用 ABOOK_BASE_URL 指向你自己的镜像
  3) 或手动下载 $name 放进 $BIN_DIR"
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
