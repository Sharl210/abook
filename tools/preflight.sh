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

printf '\n'
if [ "$fail" -eq 0 ]; then
  printf '预检全部通过\n'
  exit 0
else
  printf '预检未通过，先修再提交\n'
  exit 1
fi
