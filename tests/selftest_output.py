#!/usr/bin/env python3
"""selftest 的完整输出与截断管道必须得到同一组统计。"""
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path


SUMMARY = re.compile(r"自检\s+(\d+)\s+通过\s+/\s+(\d+)\s+失败(?:\s+/\s+(\d+)\s+跳过)?")


def summary(text):
    matches = SUMMARY.findall(text)
    assert matches, f"找不到 selftest 汇总:\n{text}"
    passed, failed, skipped = matches[-1]
    return int(passed), int(failed), int(skipped or 0)


def main(binary):
    binary = str(Path(binary).resolve())
    with tempfile.TemporaryDirectory(prefix="abook-selftest-output-") as directory:
        init = subprocess.run(
            [binary, "memory", "init", "--root", directory],
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=False,
        )
        assert init.returncode == 0, (init.returncode, init.stdout, init.stderr)

        direct = subprocess.run(
            [binary, "selftest", "--root", directory],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=180,
            check=False,
        )
        assert direct.returncode == 0, (direct.returncode, direct.stdout, direct.stderr)
        direct_summary = summary(direct.stdout + direct.stderr)

        output_file = os.path.join(directory, "selftest.txt")
        with open(output_file, "w", encoding="utf-8") as handle:
            redirected = subprocess.run(
                [binary, "selftest", "--root", directory],
                stdout=handle,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                timeout=180,
                check=False,
            )
        redirected_text = Path(output_file).read_text(encoding="utf-8") + redirected.stderr
        assert redirected.returncode == 0, (redirected.returncode, redirected_text)
        redirected_summary = summary(redirected_text)

        # 让 shell 真实建立 binary -> tail 的管道；不能用 Python 截取来替代
        # 用户报的调用形态。selftest 内部的子进程输出不得被这个外部管道误判。
        command = 'exec "$1" selftest --root "$2" | tail -3'
        piped = subprocess.run(
            ["sh", "-c", command, "selftest-pipe", binary, directory],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=180,
            check=False,
        )
        piped_text = piped.stdout + piped.stderr
        assert piped.returncode == 0, (piped.returncode, piped_text)
        piped_summary = summary(piped_text)

        assert direct_summary == redirected_summary == piped_summary, (
            direct_summary,
            redirected_summary,
            piped_summary,
        )
        assert piped_summary[1] == 0, piped_summary
        print({
            "status": "PASS",
            "direct": direct_summary,
            "redirected": redirected_summary,
            "piped_tail": piped_summary,
        })


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "./target/release/abook")
