#!/usr/bin/env python3
"""Regenerate the KNOWN_PARAMS list in src/util.rs from actual usage.

The list used to be maintained by hand, and it repeatedly drifted: parameters
that the code really reads (`--at`, `--body-file`, `--supersedes`) were missing,
so a legitimate call was rejected as a typo. Generating it from the source
removes that whole failure mode.
"""
import re, sys, pathlib

root = pathlib.Path(__file__).resolve().parent.parent
u = root / "src" / "util.rs"
src = "".join((root / "src" / f).read_text() for f in sorted(p.name for p in (root/"src").glob("*.rs")))

text = u.read_text()

# names the code actually reads
used = {k for k in re.findall(r'a\.(?:get|num|req|has|all)\("([^"]+)"\)', src)
        if not any(c.isupper() for c in k)}
# keep whatever is already listed as well, so nothing regresses
m = re.search(r'pub const KNOWN_PARAMS: &\[&str\] = &\[(.*?)\];', text, re.S)
cur = set(re.findall(r'"([^"]+)"', m.group(1)))
# BOOLS entries are bare switches and live in their own table
bo = set(re.findall(r'"([^"]+)"',
         re.search(r'const BOOLS: &\[&str\] = &\[(.*?)\];', text, re.S).group(1)))

# Alias spellings are rewritten during parsing, so they must not be listed as
# real parameters: listing them would make the alias unreachable.
ALIASES = {'limit', 'size', 'per_page', 'perpage', 'count', 'n', 'offset', 'p'}
final = sorted((used | cur) - bo - ALIASES)
lines, line = [], '   '
for x in final:
    piece = ' "%s",' % x
    if len(line) + len(piece) > 78:
        lines.append(line); line = '   '
    line += piece
lines.append(line)
block = '\n'.join(lines)

new = text[:m.start(1)] + '\n' + block + '\n' + text[m.end(1):]
if new == text:
    print("参数表已是最新（%d 个）" % len(final))
    sys.exit(0)
u.write_text(new)
print("已更新参数表：%d 个（新增 %s）" % (
    len(final), ", ".join(sorted((used | cur) - cur - bo)) or "无"))
