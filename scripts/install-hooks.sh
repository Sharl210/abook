#!/usr/bin/env bash
# 安装仓库的 git hook。`scripts/` 里的检查只有在被调用时才起作用，
# 而"记得手动跑"是不成立的——所以挂到 pre-commit 上。
set -euo pipefail
root=$(git rev-parse --show-toplevel)
hook="$root/.git/hooks/pre-commit"

cat > "$hook" <<'HOOK'
#!/usr/bin/env bash
# 提交前检查工作流语法。只查被暂存的工作流文件：无关提交不该被拖慢，
# 而一处失败的检查被全局触发时，人第一反应是绕过它。
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root"

changed=$(git diff --cached --name-only --diff-filter=ACM | grep -E '^\.github/workflows/.*\.ya?ml$' || true)
[ -n "$changed" ] || exit 0
[ -x scripts/check-workflow.sh ] || { echo "缺 scripts/check-workflow.sh，跳过工作流检查" >&2; exit 0; }

for f in $changed; do
  ./scripts/check-workflow.sh "$f"
done
HOOK
chmod +x "$hook"
echo "已安装 pre-commit: $hook"
