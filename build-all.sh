#!/bin/sh
# 一条命令产出全部 15 个目标架构。
#
# 为什么要有这个脚本：CI 已经覆盖 15 个 target，但本地改完代码后要等 CI 才知道
# 有没有架构被改坏，一次反馈要几分钟到几十分钟。而这个仓库同时要求「所有架构
# 一起维护」——只在本机 android/aarch64 上编译通过，就发了下载页，等于让另外
# 14 个架构的用户拿到旧二进制。脚本把「编译全部 + 校验单个都没漏」变成一条命令。
#
# 分两段：
#   本机能编的（android + 本机 host）→ 直接 cargo build
#   需要交叉工具链的 → 交给 CI（本地没有 NDK / llvm-mingw / 交叉 gcc）
#
# 退出码：0 全部成功；1 有目标失败；2 缺工具链（已明确列出哪些交给 CI）
set -u

TARGETS="
aarch64-linux-android
armv7-linux-androideabi
x86_64-linux-android
i686-linux-android
x86_64-unknown-linux-musl
aarch64-unknown-linux-musl
armv7-unknown-linux-musleabihf
i686-unknown-linux-musl
riscv64gc-unknown-linux-gnu
loongarch64-unknown-linux-gnu
x86_64-pc-windows-gnullvm
i686-pc-windows-gnullvm
aarch64-pc-windows-gnullvm
x86_64-apple-darwin
aarch64-apple-darwin
"

EXPECTED=15
ok=0; fail=0; skip=0
FAILED=""; SKIPPED=""

for T in $TARGETS; do
  printf '%-32s ' "$T"
  # 能不能编，看 rustc 有没有这个 target 的 std。没有就是本机缺工具链，
  # 不是代码坏了——必须区分开，否则会把「没装 NDK」误报成「编译失败」。
  SYSROOT=$(rustc --print sysroot 2>/dev/null)
  LIBDIR="$SYSROOT/lib/rustlib/$T/lib"
  # 只看目录在不在是错的：本机 armv7/x86_64/i686 的 target 目录都存在，
  # 但里面一个 rlib 都没有，rustc 随后报 "crate `std` required to be
  # available in rlib format, but was not found in this form" —— 那是空壳 std，
  # 不是编译失败。判据必须是「真的有 libstd rlib」。
  NRLIB=0
  if [ -d "$LIBDIR" ]; then
    NRLIB=$(ls "$LIBDIR"/libstd-*.rlib 2>/dev/null | wc -l)
  fi
  if [ "$NRLIB" -eq 0 ]; then
    echo "跳过（无有效 std，交给 CI）"
    skip=$((skip+1)); SKIPPED="$SKIPPED $T"
    continue
  fi
  if cargo build --release --target "$T" >target/ba_$$.log 2>&1; then
    BIN="target/$T/release/abook"
    case "$T" in *windows*) BIN="$BIN.exe" ;; esac
    if [ -f "$BIN" ]; then
      echo "OK $(stat -c%s "$BIN" 2>/dev/null || stat -f%z "$BIN") B"
      ok=$((ok+1))
    else
      echo "失败（无产物）"
      fail=$((fail+1)); FAILED="$FAILED $T"
    fi
  else
    echo "失败"
    tail -4 target/ba_$$.log | sed 's/^/    /'
    fail=$((fail+1)); FAILED="$FAILED $T"
  fi
  rm -f target/ba_$$.log
done

echo
echo "本机: $ok 成功 / $fail 失败 / $skip 跳过（合计 $((ok+fail+skip)) / $EXPECTED）"
[ -n "$FAILED" ]  && echo "失败:$FAILED"
[ -n "$SKIPPED" ] && echo "需 CI 构建:$SKIPPED"
[ "$((ok+fail+skip))" -ne "$EXPECTED" ] && { echo "目标数不对，检查清单是否漏项"; exit 1; }
[ "$fail" -gt 0 ] && exit 1
[ "$skip" -gt 0 ] && exit 2
exit 0
