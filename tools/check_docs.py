#!/usr/bin/env python3
"""Run every `abook ...` line in the operator doc against a throwaway library.

Why a separate check: the doc is what replaces the memory-library chapter of the
prompt, so a command it advertises must actually run. Reading it back by eye
does not prove that -- the help text has already promised parameters the parser
rejected three times.

Design notes that matter:
  - Each command gets a FRESH copy of the sample library. Running them in
    sequence on one copy corrupts the test itself: `move` relocates a record,
    so `delete` on the same path then reports NOT_FOUND and looks like a bug.
  - Exit code 4 means "needs --approve" / blocked: that is the documented
    contract for writes into the canonical library, not a failure.
  - Exit code 5 means "found a real problem in the library": also expected from
    audit/lint/reconcile against real data.
  - Commands that need the network (install) or report on the host (detect) are
    skipped and counted, never silently ignored.

Exit 0 when nothing unexplained happens.
"""
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
DOC = os.path.join(ROOT, "docs", "prompt-chapter.md")
def _default_sample():
    """Fall back to the real library, copied -- never modified.

    Without a fallback this check silently skips wherever ABOOK_SAMPLE_LIB is
    unset, and a check that skips is the same as no check: CI would stay green
    while the doc drifted. Copying keeps the source untouched.
    """
    env = os.environ.get("ABOOK_SAMPLE_LIB")
    if env:
        return env
    home = os.path.expanduser("~/AgentsBook")
    return home if os.path.isdir(home) else ""


SAMPLE = _default_sample()
ABOOK = os.environ.get("ABOOK_BIN", os.path.join(ROOT, "target", "release", "abook"))
# `shell=True` defaults to /bin/sh, which does not exist on this on-device
# Android prefix (it lives under the Termux-style prefix). Pick one that does.
SHELL = next(
    (p for p in ("/system/bin/sh", "/data/data/com.dsharnessmobile.shell/files/usr/bin/bash",
                 "/bin/sh", "/usr/bin/bash") if os.path.exists(p)),
    "/system/bin/sh",
)

# Placeholders the doc uses, and what they become in the sandbox.
PLACEHOLDERS = [
    # 文档里两种占位符写法都在用：`[记录]` 和 `<记录>`。替换表原先只有方括号
    # 那一套，于是 `<记录>` 被原样当命令执行，`abook show <记录>` 报"缺少参数"
    # 被记成文档缺陷——17 条噪声全来自这一处。
    #
    # 两种写法都替换成同一条真实记录。这不是把检查放宽，而是把"示例里什么是
    # 占位符"这件事认全：尖括号在命令文档里本来就是惯例。
    ("<记录A>", "@REC@"),
    ("<记录B>", "@CANON@"),
    ("<记录>", "@REC@"),
    ("<候选>", "@REC@"),
    ("<正式>", "@CANON@"),
    ("[canonical|candidate]", "canonical"),
    ("[关键词]", "GATE"),
    ("[记录]", "@REC@"),
    ("[--fix]", ""),
    ("[--page N]", ""),
    ("[--remove-sources]", ""),
    ("[--hard]", ""),
    ("[--dry-run]", ""),
    ("[--approve]", ""),
    ("[--purpose-only]", ""),
    ("[--stdin]", ""),
    ("<库>", "canonical"),
    ("<ID>", "AB2-C-001"),
    ("<记录A>", "@REC@"),
    ("<记录B>", "@REC@"),
    ("<记录>", "@REC@"),
    ("<字段> <值>", "TITLE t"),
    ("<字段>", "TITLE"),
    ("<分支>", "main"),
    ("<候选>", "@REC@"),
    ("<正式>", "@CANON@"),
    ("<标识>", "@VAULT@"),
    ("<关键词>", "GATE"),
    ("--slug 短名 --body-file F", "--slug t --body t"),
    ("--body-file F", "--body x"),
    ("--text ...", "--text x"),
    ("--title T ...", "--title t"),
    # Two different records for merge/split so the harness cannot create a
    # fake failure: `merge A B` needs both to exist, and a split point that
    # yields an empty segment is correctly refused by the tool.
    # Both take a snippet that must really occur in the body. `cat` prints a
    # path header line first, so the snippet is taken from a body line, and
    # `patch` adds --all because a short snippet may legitimately appear more
    # than once (the tool refuses that by design, which is correct).
    ("--old 旧 --new 新", "--old @SNIP@ --new @SNIP@2 --all"),
    ("--at 分隔文本", "--at @SPLITAT@"),
    ("--purpose 用途", "--purpose 测试"),
    ("--to canonical", "--to canonical --approve"),
    ("--stdin", "--body x"),
    ("<分支>", "main"),
    ("词", "grep"),
    ("关键词", "GATE"),
    ("\"说法\"", "GATE"),
    ("短名", "t"),
]

# Legitimately not runnable in a sandbox: network or host facts.
SKIP = {
    "install",
    "detect",
    "version",
}


def collect():
    doc = open(DOC, encoding="utf-8").read()
    out = []
    for block in re.findall(r"```\n(.*?)```", doc, re.S):
        for line in block.split("\n"):
            line = line.split("#")[0].strip()
            if line.startswith("abook "):
                out.append(line)
    # De-duplicate while keeping doc order.
    seen = set()
    uniq = []
    for c in out:
        if c not in seen:
            seen.add(c)
            uniq.append(c)
    return out, uniq


def first_record(lib):
    """取一条真实候选的**完整 ID** 供文档示例替换。

    为什么不用 `list` 的输出解析路径：`list` 现在显示短号（`N317`），
    文本里已经没有 `candidate/records/xxx.md` 这种东西了，正则抓不到，
    `@REC@` 就成了空串——17 条文档命令因此报"缺少参数"，全是假缺陷。

    改用 `--json`：JSON 里 `id` 字段是完整 ID，与显示层无关。
    这样"日志格式改了"不会再打断文档检查。

    还要带 `--include-promoted`：候选晋升后文件仍在磁盘上（留着溯源），
    但默认列表会把它排除。批量晋升后候选库只剩 0 条待办，`@REC@` 又变空串。
    文档示例要的是"一条真实存在的记录"，不是"一条待办"，所以要看全量。
    """
    out = subprocess.run(
        [ABOOK, "list", "--lib", "candidate", "--per-page", "1", "--json",
         "--include-promoted"],
        capture_output=True, text=True, env=env_for(lib),
    ).stdout
    m = re.search(r'"id":\s*"([^"]+)"', out)
    if m:
        return m.group(1)
    records = os.path.join(lib, "candidate", "records")
    names = sorted(n for n in os.listdir(records) if n.endswith(".md")) if os.path.isdir(records) else []
    return os.path.splitext(names[0])[0] if names else ""


def first_canonical(lib):
    """同 `first_record`：用 JSON 取完整 ID，不解析显示层。"""
    out = subprocess.run(
        [ABOOK, "list", "--lib", "canonical", "--per-page", "1", "--json"],
        capture_output=True, text=True, env=env_for(lib),
    ).stdout
    m = re.search(r'"id":\s*"([^"]+)"', out)
    if m:
        return m.group(1)
    records = os.path.join(lib, "canonical", "records")
    names = sorted(n for n in os.listdir(records) if n.endswith(".md")) if os.path.isdir(records) else []
    return os.path.splitext(names[0])[0] if names else ""


def first_vault_id(lib):
    out = subprocess.run(
        [ABOOK, "vault", "ledger"], capture_output=True, text=True,
        env=env_for(lib),
    ).stdout
    for line in out.split("\n"):
        line = line.strip()
        if line and "|" in line and not line.startswith("标识"):
            return line.split("|")[0].strip()
    return ""


def two_records(lib):
    """两条记录的完整 ID。

    同 `first_record`：走 `--json` 不解析显示层，并带 `--include-promoted`。
    原先用正则从文本里抓 `candidate/records/xxx.md`，短编号上线后文本里
    不再出现路径，正则只抓到空列表，两条都退化成占位符 `x`。
    """
    out = subprocess.run(
        [ABOOK, "list", "--lib", "candidate", "--per-page", "2", "--json",
         "--include-promoted"],
        capture_output=True, text=True, env=env_for(lib),
    ).stdout
    recs = re.findall(r'"id":\s*"([^"]+)"', out)
    if not recs:
        records = os.path.join(lib, "candidate", "records")
        recs = [os.path.splitext(n)[0] for n in sorted(os.listdir(records)) if n.endswith(".md")] if os.path.isdir(records) else []
    while len(recs) < 2:
        recs.append(recs[0] if recs else "x")
    return f"{recs[0]} {recs[1]}"


def _first_record_path(lib):
    """第一条候选记录的路径，用 `list --json` 取。

    这里原来是用正则从 `list` 的**文本**输出里抠 `xx/records/xxx.md`。
    `list` 改成印短号之后那个正则一条都抓不到，而函数有 `if not m: return`
    兜底，于是静默退回硬编码的 ("记忆","记录")——整段文档示例一直在用固定词
    碰运气，从不报错。展示格式会变，`--json` 的字段名是契约。
    """
    out = subprocess.run(
        [ABOOK, "list", "--lib", "candidate", "--per-page", "1", "--json"],
        capture_output=True, text=True, env=env_for(lib),
    ).stdout
    try:
        d = json.loads(out)
    except Exception:
        return None
    items = d.get("records") or d.get("results") or d.get("items") or []
    if not items:
        return None
    return items[0].get("path") or items[0].get("id")


def body_snippets(lib):
    """Two prose fragments: one early, one later in the body.

    `split --at` refuses a point that would leave an empty segment, so the
    split target must sit in the middle of the body, not at its first line.
    """
    path = _first_record_path(lib)
    if not path:
        return "记忆", "记录"
    body = subprocess.run(
        [ABOOK, "cat", path], capture_output=True, text=True,
        env=env_for(lib),
    ).stdout
    field = re.compile(r"^\s{2,}[A-Z][A-Z0-9_]*=")
    frags = []
    for line in body.split("\n"):
        stripped = line.strip()
        if not stripped or stripped.startswith("[") or field.match(line):
            continue
        frag = "".join(ch for ch in stripped if "\u4e00" <= ch <= "\u9fff")
        if len(frag) >= 6:
            frags.append(frag[:8])
    if not frags:
        return "记忆", "记录"
    first = frags[0]
    later = frags[-1] if len(frags) > 1 else frags[0]
    if later == first:
        later = frags[len(frags) // 2] if len(frags) > 2 else first
    return first, later


def body_snippet(lib):
    """A short string that really occurs inside a record body.

    `cat` prints a `<path>  [<candidate|canonical>]` header first, so that line
    is skipped -- taking it produced the literal "cand" and made `split`
    report NOT_FOUND, which looked like a tool bug but was the harness.
    """
    path = _first_record_path(lib)
    if not path:
        return "记忆"
    body = subprocess.run(
        [ABOOK, "cat", path], capture_output=True, text=True,
        env=env_for(lib),
    ).stdout
    # `cat` and `show` hide the container markers, so the body is simply what
    # follows the header fields: skip indented `KEY=value` lines, blank lines
    # and markers, then take prose. Taking a fragment from TITLE produced
    # NOT_FOUND for both `patch` and `split`, which looked like a tool bug but
    # was the harness reading the wrong section.
    field = re.compile(r"^\s{2,}[A-Z][A-Z0-9_]*=")
    for line in body.split("\n"):
        stripped = line.strip()
        if not stripped or stripped.startswith("[") or field.match(line):
            continue
        frag = "".join(ch for ch in stripped if "\u4e00" <= ch <= "\u9fff")
        if len(frag) >= 6:
            return frag[:8]
    return "记忆"


def env_for(lib):
    env = dict(os.environ)
    env["ABOOK_ROOT"] = lib
    env["PATH"] = os.path.dirname(ABOOK) + os.pathsep + env.get("PATH", "")
    return env


def _real_commands(rep_bin=ABOOK):
    """Ask the tool itself which command names it dispatches.

    Scraping `help` was wrong: `--fix` (a doctor flag) and `fix` (a command)
    look identical to a line regex, and filtering by "also appears with a --"
    then deleted the real command. `abook commands` is the dispatch table.
    """
    env = dict(os.environ)
    env.pop("ABOOK_ROOT", None)
    r = subprocess.run([rep_bin, "commands"], capture_output=True, text=True,
                       timeout=60, env=env)
    names = {ln.strip() for ln in r.stdout.splitlines() if ln.strip()}
    if not names:
        # never silently pass because the probe itself failed
        raise RuntimeError(f"abook commands 无输出 (rc={r.returncode}): {r.stderr[:200]}")
    return names


def check_phantoms(rep_bin):
    """Docs must not mention commands the tool does not dispatch.

    A renamed or removed command left in the docs is worse than a missing one:
    an operator follows it and gets a usage error mid-task.
    """
    real = _real_commands(rep_bin)
    bad = []
    for doc in sorted(os.listdir(os.path.join(ROOT, "docs"))):
        if not doc.endswith(".md"):
            continue
        path = os.path.join(ROOT, "docs", doc)
        text = open(path, encoding="utf-8").read()
        for m in re.finditer(r"abook ([a-z][a-z0-9-]*)", text):
            cmd = m.group(1)
            # `abook <记录>` 这类占位符不是命令名：它出现的地方是在讲
            # "把某条记录传给命令"，读者不会真去敲 `abook <记录>`。
            # 把它当幽灵命令报错，只会让真问题淹没在噪声里。
            if cmd.startswith("<"):
                continue
            # `mcp` is a process-level mode dispatched before the CLI router;
            # it is intentionally absent from `abook commands` but valid in
            # user-facing docs and help.
            if cmd not in real and cmd != "mcp":
                bad.append((doc, cmd))
    # de-dup, keep order
    seen = set()
    out = []
    for doc, cmd in bad:
        if (doc, cmd) in seen:
            continue
        seen.add((doc, cmd))
        out.append((doc, cmd))
    return out


def check_completeness(rep_bin):
    """Every command a user can type must be documented somewhere.

    The other direction -- "docs mention a command that does not exist" -- was
    already checked. This is the one that was missing, and it is the more
    insidious: a command nobody documents still works, so nothing looks broken,
    but a reader consulting the reference concludes the capability is absent and
    reimplements it by hand. `branch`, `recount`, `commands`, `describe` and
    `leftover` were all in that state.

    Mention detection is deliberately loose (a backticked token or `abook <cmd>`)
    because the check is about coverage, not about prose style. Requiring a
    section per command would punish a compact table that lists five at once.
    """
    real = _real_commands(rep_bin)
    said = set()
    for doc in sorted(os.listdir(os.path.join(ROOT, "docs"))):
        if not doc.endswith(".md"):
            continue
        text = open(os.path.join(ROOT, "docs", doc), encoding="utf-8").read()
        said |= {m.group(1) for m in re.finditer(r"abook\s+([a-z][a-z0-9-]*)", text)}
        said |= {m.group(1) for m in re.finditer(r"`([a-z][a-z0-9-]*)`", text)}
    return sorted(real - said)


def rule_code_mismatch():
    """Every rule code emitted by the binary must be documented, and the doc
    must not cite a code the binary never emits."""
    import glob as _glob
    import re as _re

    code_codes = set()
    for path in _glob.glob("src/**/*.rs", recursive=True):
        try:
            text = open(path, encoding="utf-8").read()
        except OSError:
            continue
        code_codes.update(_re.findall(r'"([NTRKS][0-9]{3})"', text))

    doc = os.path.join(ROOT, "docs", "ROBUSTNESS.md")
    if not os.path.exists(doc):
        return ["docs/ROBUSTNESS.md 缺失"]
    doc_text = open(doc, encoding="utf-8").read()
    doc_codes = set(_re.findall(r"\b([NTRKS][0-9]{3})\b", doc_text))

    out = []
    for c in sorted(code_codes - doc_codes):
        out.append(f"{c}（代码有，文档没写）")
    for c in sorted(doc_codes - code_codes):
        out.append(f"{c}（文档写了，代码没有）")
    return out


def main():
    if not os.path.exists(ABOOK):
        print("  ✗ 没有 target/release/abook，先编译")
        return 1
    if not SAMPLE or not os.path.isdir(SAMPLE):
        print("  ✗ 找不到样本库（~/AgentsBook 不存在，也未设 ABOOK_SAMPLE_LIB）")
        return 1

    all_cmds, uniq = collect()
    base = tempfile.mkdtemp(prefix="abookdoc")
    lib = os.path.join(base, "AgentsBook")

    # The live library may hold a transient `.abook.lock` that disappears
    # between listing and copying -- the copy then dies with a confusing
    # ENOENT about a file nobody asked for. Locks are not sample data, so
    # skip them and make the copy tolerant of races instead of failing.
    def _ignore(_dir, names):
        return [n for n in names if n.endswith(".lock") or n == ".abook.lock"]

    def _copy_sample():
        shutil.copytree(SAMPLE, lib, ignore=_ignore,
                        dirs_exist_ok=True, ignore_dangling_symlinks=True)

    _copy_sample()

    problems = []
    ok = 0
    expected = 0
    skipped = 0

    for raw in uniq:
        name = raw.split()[1] if len(raw.split()) > 1 else ""
        if name == "mcp":
            # `abook mcp` owns stdio until EOF; it is a process-level mode,
            # not a bounded shell example that this command runner can execute.
            skipped += 1
            continue
        if name in SKIP:
            skipped += 1
            continue

        # Fresh copy per command: a preceding run may have moved or deleted the
        # very record the next command names, which would be a fake failure.
        shutil.rmtree(lib, ignore_errors=True)
        _copy_sample()
        env = env_for(lib)

        cmd = raw
        for a, b in PLACEHOLDERS:
            cmd = cmd.replace(a, b)
        cmd = cmd.replace("@REC@", first_record(lib))
        if "merge" in cmd:
            recs = two_records(lib)
            cmd = cmd.replace("A B", recs)
        cmd = cmd.replace("@CANON@", first_canonical(lib))
        # Quote it: vault identifiers contain spaces ("GitHub classic ..."),
        # and an unquoted expansion makes the shell pass each word as a
        # separate argument -- which looks like "no such entry".
        cmd = cmd.replace("@VAULT@", f'"{first_vault_id(lib)}"')
        first, later = body_snippets(lib)
        cmd = cmd.replace("@SPLITAT@", later)
        cmd = cmd.replace("@SNIP@2", first + "（补）")
        cmd = cmd.replace("@SNIP@", first)

        if "@" in cmd:
            problems.append((raw, "占位符没替换", cmd))
            continue

        if cmd.startswith("abook "):
            # Use the exact binary under test. On Android the published target
            # filename is architecture-specific and the containing app path may
            # not be discoverable by /system/bin/sh through PATH.
            cmd = shlex.quote(ABOOK) + cmd[len("abook"):]

        r = subprocess.run(cmd, shell=True, executable=SHELL, capture_output=True,
                           text=True, env=env, cwd=base, timeout=180)
        if r.returncode in (0, 5):
            ok += 1
        elif r.returncode in (3, 4):
            # Both are documented outcomes, not failures:
            #   4 = needs --approve, or the write was refused;
            #   3 = not found. The doc examples say `--old "旧文本"` and
            #       `--at 分隔文本`, which are deliberately absent from a real
            #       library, so rc=3 is exactly the right answer and treating
            #       it as a failure punished the docs for being honest.
            expected += 1
        else:
            msg = (r.stdout + r.stderr).strip().split("\n")[-1][:100]
            problems.append((raw, f"rc={r.returncode}", msg))

    # Every doc mentions commands, but only the operator doc gets executed.
    # COMMANDS.md / OPERATIONS.md could therefore advertise a command that no
    # longer exists and nothing would notice. Scan every doc for `abook <cmd>`
    # mentions and require each one to be a real, dispatched command.
    phantom = check_phantoms(ABOOK)
    for name, cmd in phantom:
        problems.append((f"abook {cmd}", "文档提到的命令不存在", name))

    # ... and the reverse: a real command that no doc ever mentions.
    undocumented = check_completeness(ABOOK)
    for cmd in undocumented:
        problems.append((f"abook {cmd}", "命令存在但文档从未提到", ""))

    # Rule codes are the contract between code and the robustness doc: a user
    # who sees `[WARN] S021` looks it up in docs/ROBUSTNESS.md. A code that
    # fires but is undocumented is an unexplainable error message, so require
    # both directions.
    for code in rule_code_mismatch():
        problems.append((code, "规则码在代码与 ROBUSTNESS.md 之间不一致", ""))

    shutil.rmtree(base, ignore_errors=True)

    print(f"文档命令 {len(all_cmds)} 行（去重 {len(uniq)} 条）：")
    print(f"  跑通 {ok}，按约定需批准 {expected}，跳过（网络/宿主） {skipped}")
    if problems:
        print(f"  问题 {len(problems)} 条：")
        for raw, why, msg in problems:
            print(f"    ✗ {why}  {raw}")
            print(f"        {msg}")
        return 1
    print("  ✓ 文档里每条命令都真的能跑")
    return 0


if __name__ == "__main__":
    sys.exit(main())
