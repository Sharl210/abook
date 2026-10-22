#!/bin/sh
# 提交前预检：把"靠肉眼看不出来"的失败模式全部机械拦下。
#
# 真实踩过的坑都在这里：
#   - 重复的 YAML job 键 → 整个 workflow 0 个 job
#   - CI 目标与 Rust 映射漂移 → install 404
#   - selftest 内嵌清单过期
#   - 编译不过就推送
#
# 用法: sh tools/preflight.sh
set -eu
cd "$(dirname "$0")/.."

fail=0
step() { printf '\n=== %s ===\n' "$1"; }
bad()  { printf '  ✗ %s\n' "$1"; fail=1; }
ok()   { printf '  ✓ %s\n' "$1"; }

step "1. workflow 结构校验"
if python3 tools/check_workflow.py; then ok "workflow 结构合法"; else bad "workflow 结构有问题"; fi

step "2. CI 目标与 selftest 内嵌清单一致"
if python3 tools/sync_targets.py; then ok "目标清单一致"; else bad "目标清单过期，跑 tools/sync_targets.py --write"; fi

step "3. 编译"
if cargo build --release --offline 2>/dev/null; then
  ok "release 编译通过"
else
  bad "编译失败"
fi

step "4. 自检"
if ./target/release/abook selftest >/dev/null 2>&1; then
  N=$(./target/release/abook selftest 2>/dev/null | tail -1)
  ok "自检通过（$N）"
else
  bad "自检失败"
  ./target/release/abook selftest 2>&1 | grep FAIL | head -5
fi

step "5. 真库只读接管"
if ./target/release/abook state >/dev/null 2>&1; then
  ok "能读真实记忆库"
else
  bad "读库失败"
fi

# ---------------------------------------------------------------- 6. 参数登记完整性
# 未知参数会被直接拒绝，所以源码里用到却没登记的参数会让一个合法用法失效。
# 反过来，登记表靠手维护必然漂移，这里机械核对源码与登记表。
echo "=== 6. 参数登记完整性 ==="
# Regenerate first: the table is derived from source, never hand-edited.
python3 tools/gen_params.py >/dev/null 2>&1 || true
python3 - <<'PY'
import re, os, sys
src = ""
for f in sorted(os.listdir('src')):
    if f.endswith('.rs'):
        src += open('src/' + f).read()
# `a.` is not always Args: `at.get("COVERED_THROUGH")` reads an Atlas field.
# CLI parameter names are lowercase or hyphenated, so drop names containing an
# uppercase letter to exclude those struct-field reads.
used = {k for k in re.findall(r'a\.(?:get|num|req|has|all)\("([^"]+)"\)', src)
        if not any(ch.isupper() for ch in k)}
know = set(re.findall(r'"([^"]+)"',
         re.search(r'pub const KNOWN_PARAMS: &\[&str\] = &\[(.*?)\];', src, re.S).group(1)))
boolr = set(re.findall(r'"([^"]+)"',
          re.search(r'const BOOLS: &\[&str\] = &\[(.*?)\];', src, re.S).group(1)))
alias = {'limit','size','per_page','perpage','count','n','offset','p'}
missing = sorted(used - know - boolr - alias)
if missing:
    print("  ✗ 源码用到但未登记: " + ", ".join(missing))
    print("    未登记的参数会被拒绝，合法用法会失效")
    sys.exit(1)
print(f"  ✓ 参数登记完整（登记 {len(know)} 个取值参数 + {len(boolr)} 个开关）")
PY

if [ $? -ne 0 ]; then
  bad "参数登记不完整"
fi

printf '\n'
if [ "$fail" -eq 0 ]; then
  printf '预检全部通过\n'
  exit 0
else
  printf '预检未通过，先修再提交\n'
  exit 1
fi
