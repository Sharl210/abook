#!/usr/bin/env bash
# 在推送前检查 GitHub Actions 工作流里的内嵌脚本能不能通过语法检查。
#
# 为什么需要它：`python3 -c "..."` 和 `run: |` 里的 shell 在本地没有任何入口
# 会被执行，只有真的推上去跑 CI 才会暴露。v1.6.16 与 v1.6.17 连续两版都死在
# 一处两格缩进——`IndentationError: unexpected indent`，而前面 15 个架构的编译
# 全部成功。等发现时已经烧掉两次完整构建和一个 tag。
#
# 这个脚本做两件事：
#   1. 把每个 python3 -c "..." 块抽出来编译一遍（compile()，不执行）；
#   2. 把每个 run: | 块交给 `bash -n` 检查语法。
#
# 它不执行任何工作流逻辑，只做语法门——语法错误正是本地最不容易发现的那一类。
set -euo pipefail

WF="${1:-.github/workflows/build.yml}"
[ -f "$WF" ] || { echo "找不到工作流: $WF" >&2; exit 2; }

python3 - "$WF" <<'PY'
import re, subprocess, sys, tempfile, os

path = sys.argv[1]
src = open(path, encoding='utf-8').read()
lines = src.split('\n')
fail = 0

# ---- 1. python3 -c "..." 块 ----
# 块以 `python3 -c "` 开头，到与之配对的孤立 `"` 行结束。工作流里的写法就是
# 这样（缩进 + 单行引号），所以按行扫比按字符扫稳。
i = 0
n_py = 0
while i < len(lines):
    m = re.search(r'python3\s+-c\s+"\s*$', lines[i])
    if not m:
        i += 1
        continue
    indent = len(lines[i]) - len(lines[i].lstrip())
    body = []
    i += 1
    while i < len(lines) and lines[i].strip() != '"':
        # 工作流里的 python 代码带 YAML 的缩进，交给 python 前必须剥掉，
        # 否则每一行都会被判 unexpected indent —— 这正是被检查的那个 bug。
        ln = lines[i]
        body.append(ln[indent:] if ln[:indent].strip() == '' else ln.lstrip())
        i += 1
    code = '\n'.join(body) + '\n'
    n_py += 1
    try:
        compile(code, f'{path}:python3-c-{n_py}', 'exec')
    except SyntaxError as e:
        fail += 1
        print(f'FAIL  第 {n_py} 段 python3 -c: {e.msg} (偏移行 {e.lineno})')
        if e.text:
            print(f'      {e.text.rstrip()}')
    i += 1

# ---- 2. run: | 块交给 bash -n ----
n_sh = 0
for m in re.finditer(r'^\s*run:\s*\|\s*$', src, re.M):
    start = src.index('\n', m.end()) + 1
    block = []
    base = None
    for ln in src[start:].split('\n'):
        if ln.strip() == '':
            block.append('')
            continue
        ind = len(ln) - len(ln.lstrip())
        if base is None:
            base = ind
        if ind < base:
            break
        block.append(ln[base:])
    script = '\n'.join(block).rstrip() + '\n'
    n_sh += 1
    with tempfile.NamedTemporaryFile('w', suffix='.sh', delete=False) as f:
        f.write(script)
        tmp = f.name
    r = subprocess.run(['bash', '-n', tmp], capture_output=True, text=True)
    os.unlink(tmp)
    if r.returncode != 0:
        fail += 1
        print(f'FAIL  第 {n_sh} 段 run: 块 shell 语法错误')
        print('      ' + (r.stderr.strip().replace('\n', '\n      ')))

print(f'工作流语法检查：{n_py} 段 python、{n_sh} 段 shell，{fail} 处错误')
sys.exit(1 if fail else 0)
PY
