#!/bin/sh
# abook 引导安装脚本。
#
# 用途：在一台还没装 abook 的机器上，一条命令装好。
# 它会自己识别平台与架构，下载对应产物，放进**记忆库根目录**。
#
#   curl -fsSL https://raw.githubusercontent.com/Sharl210/abook/main/install.sh | sh
#
# 为什么装在库根而不是 PATH 目录：程序要回答"我的库在哪"。把二进制和
# Global.md / Index.md 放在同一层，这个问题就有了唯一答案，换 shell、换会话、
# 换调用方式都一样。装进 PATH 目录则多出一个"哪个 abook 被调到"的变量，还会
# 让安装动作伸手到用户 shell 配置里去。
#
# 可选环境变量：
#   ABOOK_BASE_URL  自定义下载基址（默认用本仓库 latest release）
#   ABOOK_BIN_DIR   安装目录（默认库根 ABOOK_ROOT，再默认 ~/AgentsBook）

set -eu

REPO="${ABOOK_REPO:-Sharl210/abook}"
# 两个官方固定地址，都不含版本号，发一次新版这里不用改：
#   releases/latest  跟着最新 release 走
#   raw/downloads    跟着 downloads 分支走（每次构建由 CI 覆盖）
REL_BASE="https://github.com/${REPO}/releases/latest/download"
RAW_BASE="https://raw.githubusercontent.com/${REPO}/downloads"
BASE="${ABOOK_BASE_URL:-$REL_BASE}"
# 镜像前缀：把官方整条 URL 原样拼在后面即可。国内直连 github.com 常年不稳，
# 这一串是按实测通过率排的，前面的先试。
MIRRORS="${ABOOK_MIRRORS:-https://gh-proxy.com/ https://ghfast.top/ https://ghproxy.net/}"
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
    windows/x86_64)     target=x86_64-pc-windows-gnullvm.exe ;;
    windows/x86)        target=i686-pc-windows-gnullvm.exe ;;
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
  tp windows/x86_64    x86_64-pc-windows-gnullvm.exe
  tp windows/x86       i686-pc-windows-gnullvm.exe
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
      say "  --bin-dir <目录>      指定安装目录（默认库根：~/AgentsBook）"
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
  windows/x86_64)     target=x86_64-pc-windows-gnullvm.exe ;;
  windows/x86)        target=i686-pc-windows-gnullvm.exe ;;
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
#
# 目标只有一个：记忆库根。以前这里会扫描 PATH、挑第一个可写的 bin 目录，
# 于是同一台机器上装在哪取决于启动脚本留下的 PATH，装完还得劝用户改 shell
# 配置。现在不再看 PATH —— 库根是一个确定的位置，装进去就完事。
pick_dir() {
  # Explicit choice wins. `--bin-dir` is parsed above but had no effect until
  # it was threaded through here: accepting a flag and ignoring it is the same
  # defect class as documenting one that does not exist.
  if [ -n "$ARG_BIN" ]; then printf '%s' "$ARG_BIN"; return; fi
  if [ -n "${ABOOK_BIN_DIR:-}" ]; then printf '%s' "$ABOOK_BIN_DIR"; return; fi
  # ABOOK_ROOT 只为老脚本保留；不设就用默认库根。
  if [ -n "${ABOOK_ROOT:-}" ]; then printf '%s' "$ABOOK_ROOT"; return; fi
  printf '%s' "${HOME:-/tmp}/AgentsBook"
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

# 逐源尝试。一个源失败不等于网络坏：国内直连 github.com 常被丢包，而镜像好使。
# 每个源都要过 is_binary —— 只有 HTTP 200 不算数，劫持页和错误页也是 200。
#
# --connect-timeout 6 是关键：被墙时表现为连接挂住而不是立刻拒绝，不设这个
# 上限就会卡满 --max-time，八个源挨下来要十几分钟，看起来像死机。
fetch_from() {
  _u="$1"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --connect-timeout 6 --max-time 180 \
         --retry 2 --retry-delay 1 --retry-all-errors \
         -o "$tmp" "$_u" 2>/dev/null && is_binary "$tmp"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -T 20 -t 2 -O "$tmp" "$_u" 2>/dev/null && is_binary "$tmp"
  else
    return 1
  fi
}

# 源清单：
#  - 显式给了 ABOOK_BASE_URL 就只认它（自建镜像/私有部署的用法）
#  - 否则官方 release → 三个镜像 → downloads 分支 → 分支的三个镜像
sources=""
if [ -n "${ABOOK_BASE_URL:-}" ]; then
  sources="$BASE/$name"
else
  # release 资产是平铺的 abook-<triple>[.exe]；downloads 分支里同一样东西
  # 放在 <triple>/abook[.exe]。两个形态不同，不能套同一个模板。
  dl_triple="${target}"
  case "$name" in *.exe) dl_suffix=".exe" ;; *) dl_suffix="" ;; esac
  DL_PATH="$dl_triple/abook$dl_suffix"

  sources="$REL_BASE/$name"
  for m in $MIRRORS; do sources="$sources $m$REL_BASE/$name"; done
  sources="$sources $RAW_BASE/$DL_PATH"
  for m in $MIRRORS; do sources="$sources $m$RAW_BASE/$DL_PATH"; done
  # jsDelivr 走自己的 CDN，和 gh 系镜像不是同一条线路；它只认仓库文件，
  # 所以只对 downloads 分支这一路有效。
  sources="$sources https://cdn.jsdelivr.net/gh/${REPO}@downloads/$DL_PATH"
fi

[ -n "$(command -v curl || command -v wget)" ] || die "找不到 curl 或 wget"

ok=0
n=0
for u in $sources; do
  n=$((n + 1))
  if fetch_from "$u"; then
    ok=1
    [ "$n" -gt 1 ] && say "第 $n 个源可用: $u"
    break
  fi
done

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

say ""
say "库根: $BIN_DIR"
say "调用: $final detect"
say ""
say "程序与库同目录，路径不依赖环境变量，也不需要改 PATH 或 shell 配置。"
say "想少打字就自己加个别名（指向上面这个绝对路径），别把库根塞进 PATH。" 
