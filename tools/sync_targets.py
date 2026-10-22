#!/usr/bin/env python3
"""从 workflow 提取 CI 构建目标，重写 src/selftest.rs 里的 CI_TARGETS 常量。

内嵌常量让 selftest 能在任意机器上运行（不需要仓库文件），但常量会漂移。
本脚本做单向同步，退出码非 0 表示已过期。

用法:
  python3 tools/sync_targets.py          # 检查，过期则退出 1
  python3 tools/sync_targets.py --write  # 直接更新 selftest.rs
"""
import re
import sys

WF = ".github/workflows/build.yml"
RS = "src/selftest.rs"


def ci_targets():
    with open(WF, encoding="utf-8") as f:
        txt = f.read()
    return sorted(set(re.findall(r"^\s*-\s*target:\s*([A-Za-z0-9_.-]+)\s*$", txt, re.M)))


def embedded():
    with open(RS, encoding="utf-8") as f:
        txt = f.read()
    m = re.search(r"const CI_TARGETS: &\[&str\] = &\[(.*?)\];", txt, re.S)
    if not m:
        return []
    return sorted(re.findall(r'"([^"]+)"', m.group(1)))


def main():
    want = ci_targets()
    have = embedded()
    if want == have:
        print("CI_TARGETS 与 workflow 一致（%d 项）" % len(want))
        return 0
    missing = set(want) - set(have)
    extra = set(have) - set(want)
    if missing:
        print("selftest.rs 缺少:", ", ".join(sorted(missing)))
    if extra:
        print("selftest.rs 多余:", ", ".join(sorted(extra)))
    if "--write" not in sys.argv:
        print("\n运行 `python3 tools/sync_targets.py --write` 更新")
        return 1
    with open(RS, encoding="utf-8") as f:
        txt = f.read()
    body = "\n".join('        "%s",' % t for t in want)
    txt = re.sub(
        r"const CI_TARGETS: &\[&str\] = &\[.*?\];",
        "const CI_TARGETS: &[&str] = &[\n%s\n    ];" % body,
        txt,
        flags=re.S,
    )
    with open(RS, "w", encoding="utf-8") as f:
        f.write(txt)
    print("已更新 selftest.rs（%d 项）" % len(want))
    return 0


if __name__ == "__main__":
    sys.exit(main())
