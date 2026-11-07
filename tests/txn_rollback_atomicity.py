#!/usr/bin/env python3
"""Verify rejected stale rollbacks leave files, revision and journal unchanged."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def check(binary):
    binary = str(Path(binary).resolve())
    with tempfile.TemporaryDirectory(prefix="abook-rollback-atomicity-") as directory:
        root = Path(directory)

        def cli(*args, expected=0):
            result = subprocess.run(
                [binary, *args, "--root", directory, "--json"],
                capture_output=True, text=True, encoding="utf-8", timeout=60,
            )
            assert result.returncode == expected, (args, result.returncode, result.stdout, result.stderr)
            return json.loads(result.stdout)

        tokens = []
        for i in range(3):
            (root / f"record{i}.md").write_text(f"before {i}\n", encoding="utf-8")
            body = root / f"body{i}.txt"
            body.write_text(f"after {i}\nUnicode 😀\n", encoding="utf-8")
            preview = cli("txn", "preview", "--path", f"record{i}.md", "--body-file", str(body))
            tokens.append(preview["token"])
            cli("txn", "apply", "--preview-token", tokens[-1], "--expect-revision", str(preview["revision"]))

        def snapshot():
            return {p.name: p.read_bytes() for p in root.iterdir() if p.is_file()}

        before = snapshot()
        rejected = cli("txn", "rollback", "--preview-token", tokens[0], expected=5)
        assert rejected["error"]["tag"] == "INTEGRITY", rejected
        after = snapshot()
        changed = sorted(k for k in before.keys() | after.keys() if before.get(k) != after.get(k))
        assert not changed, f"rejected rollback modified: {changed}"
        cli("txn", "rollback", "--preview-token", tokens[-1])
        assert (root / "record2.md").read_text(encoding="utf-8") == "before 2\n"
        assert (root / "record0.md").read_text(encoding="utf-8") == "after 0\nUnicode 😀\n"
        assert (root / "LIBRARY_REVISION").read_text().strip() == "4"
        return {"status": "PASS", "binary": binary,
                "sha256": hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
                "stale_rollback_unchanged": True, "latest_rollback_restored": True}


if __name__ == "__main__":
    print(json.dumps(check(sys.argv[1]), ensure_ascii=False))
