#!/usr/bin/env python3
"""静态校验 .github/workflows/build.yml（只用标准库）。

检查的都是本项目真实踩过的坑：

1. 重复的顶层 job 键 —— GitHub 判定整个 workflow 非法，结果是 0 个 job。
   这个失败模式极难从 UI 看出来，必须机械检测。
2. TAB 字符 —— YAML 禁用。
3. needs: 引用不存在的 job。
4. 每个 job 必须有 runs-on 与 steps。
5. CI 构建矩阵与 Rust 的 target_triple() 必须完全一致，
   否则 abook install 下载 404。

用法: python3 tools/check_workflow.py [workflow] [main.rs]
退出码: 0 通过 / 1 有问题
"""
import re
import sys


def load(path):
    with open(path, encoding="utf-8") as f:
        return f.read().split("\n")


def top_jobs(lines):
    out, in_jobs = [], False
    for i, l in enumerate(lines, 1):
        if re.match(r"^jobs:\s*$", l):
            in_jobs = True
            continue
        if in_jobs:
            if l and not l.startswith(" "):
                break
            m = re.match(r"^  ([A-Za-z_][A-Za-z0-9_-]*):\s*$", l)
            if m:
                out.append((i, m.group(1)))
    return out


def ci_targets(lines):
    txt = "\n".join(lines)
    return set(re.findall(r"^\s*-\s*target:\s*([A-Za-z0-9_.-]+)\s*$", txt, re.M))


def rust_targets(main_rs):
    with open(main_rs, encoding="utf-8") as f:
        txt = f.read()
    m = re.search(r"fn target_triple\(.*?\n\}", txt, re.S)
    if not m:
        return set()
    return set(re.findall(r'=>\s*"([a-z0-9_-]+)"', m.group(0)))


def main():
    wf = sys.argv[1] if len(sys.argv) > 1 else ".github/workflows/build.yml"
    rs = sys.argv[2] if len(sys.argv) > 2 else "src/main.rs"
    lines = load(wf)
    text = "\n".join(lines)
    problems = []

    jobs = top_jobs(lines)
    names = [n for _, n in jobs]
    dupes = {n for n in names if names.count(n) > 1}
    if dupes:
        for n in sorted(dupes):
            rows = [i for i, x in jobs if x == n]
            problems.append(
                "重复的顶层 job 键 `%s` 出现在行 %s —— 会让整个 workflow 变成 0 个 job"
                % (n, rows)
            )
    if not names:
        problems.append("没有解析到任何 job")

    for i, l in enumerate(lines, 1):
        if "\t" in l:
            problems.append("第 %d 行含 TAB，YAML 不允许" % i)
            break

    for m in re.finditer(r"needs:\s*\[([^\]]+)\]", text):
        for r in [x.strip() for x in m.group(1).split(",")]:
            if r and r not in names:
                problems.append("needs 引用不存在的 job: %s" % r)
    for m in re.finditer(r"^\s*needs:\s*([A-Za-z0-9_-]+)\s*$", text, re.M):
        if m.group(1) not in names:
            problems.append("needs 引用不存在的 job: %s" % m.group(1))

    for idx, (ln, name) in enumerate(jobs):
        end = jobs[idx + 1][0] - 1 if idx + 1 < len(jobs) else len(lines)
        body = "\n".join(lines[ln:end])
        if "runs-on:" not in body:
            problems.append("job `%s` 缺少 runs-on" % name)
        if "steps:" not in body:
            problems.append("job `%s` 缺少 steps" % name)

    built = ci_targets(lines)
    declared = rust_targets(rs)
    for t in sorted(declared - built):
        problems.append("Rust 声明了但 CI 不构建（install 会 404）: %s" % t)
    for t in sorted(built - declared):
        problems.append("CI 构建了但 Rust 未映射（产物无人下载）: %s" % t)

    print("job 数 %d: %s" % (len(names), ", ".join(names)))
    print("CI 构建目标 %d 个，Rust 映射 %d 个" % (len(built), len(declared)))
    if problems:
        print("\n发现 %d 个问题:" % len(problems))
        for p in problems:
            print("  -", p)
        return 1
    print("\n校验通过")
    return 0


if __name__ == "__main__":
    sys.exit(main())
