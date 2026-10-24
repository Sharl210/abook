#!/bin/sh
# 校验 15 个架构的二进制是否**同源**。
#
# 为什么需要：CI 保证了「15 个都构建成功才发布、数量不足拒绝覆盖」，但它不保证
# 「15 个来自同一份源码」。某个平台完全可能在发布页上留着上一轮的旧二进制，而这
# 一轮其他 14 个重编成功——文件数是 15，看起来齐整，实际有一个平台在跑几个月前
# 的代码。提示词里的下载地址固定，用户拿到哪个版本全靠运气。
#
# build.rs 把所有 src/**/*.rs 的 FNV-1a 指纹（16 位小写十六进制）编进二进制，
# 与架构无关。所以「同源」可以被直接证明：每个文件里都必须出现同一个指纹串。
#
# 用法：
#   sh verify-release.sh            校验本机 target/<triple>/release 的产物
#   sh verify-release.sh <目录>     校验目录下 15 个产物
#
# 退出码：0 同源且齐全；1 不全、不同源或与工作树不符
set -u

EXPECTED_TARGETS="
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

DIR="${1:-}"

# 期望指纹取自本机能跑的那个二进制（下面还会用同一算法从源码重算核对，
# 所以「产物是否落后于源码」不需要靠 mtime 或 git 状态猜）。
EXPECT=""; PROBE=""
for probe in target/release/abook \
             target/aarch64-linux-android/release/abook \
             target/x86_64-linux-android/release/abook; do
  if [ -x "$probe" ]; then
    got=$("$probe" version 2>/dev/null | sed -n 's/.*src=\([0-9a-f]\{16\}\).*/\1/p')
    if [ -n "$got" ]; then EXPECT=$got; PROBE=$probe; break; fi
  fi
done
if [ -z "$EXPECT" ]; then
  echo "错误: 本机没有可执行的 abook，无法确定期望指纹" >&2
  echo "先在本机平台构建一次（aarch64-linux-android 或 host）再校验" >&2
  exit 1
fi
# 判断「产物是否落后于源码」只有一条可靠依据：把工作树的源文件指纹当场重算，
# 与产物报告的值比。git 状态不行——改了又重编过是完全正常的流程，那样会一直误报；
# mtime 更不行，touch 一下内容没变也会误报。这里直接照 build.rs 的算法重算一遍：
# FNV-1a over src/**/*.rs 的路径与字节，排序后累加。
#
# 这里确实复制了一份 build.rs 的算法，这是权衡后的选择而不是疏忽：不复制就得相信
# mtime（touch 会误报）或 git 状态（改了又重编也会误报），两者都会让这个脚本在正常
# 流程里持续喊狼来了，喊久了就没人看它的结论。代价是两份实现必须一起改——所以
# 一旦 build.rs 换了算法，这个脚本会立刻把所有产物报成「落后于源码」，是个响亮的
# 失败而不是安静的漏检。
recompute_fp() {
  find src -name '*.rs' -type f | LC_ALL=C sort | python3 -c '
import sys
h = 0xcbf29ce484222325
M = 0x100000001b3
for p in sys.stdin.read().split():
    for b in p.encode():
        h = ((h ^ b) * M) & 0xFFFFFFFFFFFFFFFF
    for b in open(p, "rb").read():
        h = ((h ^ b) * M) & 0xFFFFFFFFFFFFFFFF
print("%016x" % h)
'
}
TREE_FP=$(recompute_fp 2>/dev/null || true)
if [ -n "$TREE_FP" ] && [ "$TREE_FP" != "$EXPECT" ]; then
  echo "警告: 工作树源码指纹 $TREE_FP ≠ 探针产物 $EXPECT" >&2
  echo "探针产物落后于源码，先重编再校验，否则结论不可信。" >&2
  exit 1
fi
echo "期望指纹（来自 $PROBE）: $EXPECT"
echo

found=0; missing=0; differ=0
MISSING_LIST=""; DIFFER_LIST=""
for T in $EXPECTED_TARGETS; do
  f=""
  if [ -n "$DIR" ]; then
    for cand in "$DIR/abook-$T" "$DIR/abook-$T.exe" "$DIR/$T/abook" "$DIR/$T/abook.exe"; do
      [ -f "$cand" ] && { f="$cand"; break; }
    done
  else
    for cand in "target/$T/release/abook" "target/$T/release/abook.exe"; do
      [ -f "$cand" ] && { f="$cand"; break; }
    done
  fi
  if [ -z "$f" ]; then
    printf '%-32s 缺失\n' "$T"
    missing=$((missing+1)); MISSING_LIST="$MISSING_LIST $T"
    continue
  fi
  # 定点字节匹配，不执行目标文件——15 个里有 Windows/macOS/别家架构，本机跑不了，
  # 非要执行就等于永远验不了跨平台产物。
  if grep -aq -F "$EXPECT" "$f" 2>/dev/null; then
    printf '%-32s 同源\n' "$T"
    found=$((found+1))
  else
    printf '%-32s **不同源**\n' "$T"
    differ=$((differ+1)); DIFFER_LIST="$DIFFER_LIST $T"
  fi
done

echo
echo "同源 $found / 15    缺失 $missing    不同源 $differ"
if [ "$missing" -ne 0 ]; then
  echo "缺失:$MISSING_LIST"
  echo "结论: 产物不全，不能发布"
  exit 1
fi
if [ "$differ" -ne 0 ]; then
  echo "不同源:$DIFFER_LIST"
  echo "结论: **不是同一份源码编出来的**"
  echo "必须整体重编这 15 个，不能只补跑出问题的那几个——补跑出来的必然和其余的不同源。"
  exit 1
fi
echo "结论: 15 个产物全部同源，指纹 $EXPECT"
exit 0
