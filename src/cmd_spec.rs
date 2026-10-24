//! `abook spec` — serve long-form rule sets on demand.
//!
//! The prompt keeps one entry line per spec; the text lives here. Two reasons:
//! size (these seven blocks are a quarter of the prompt) and fidelity (a spec
//! read from the tool is the spec, not a paraphrase of it).
//!
//! `abook spec` with no argument prints the index, which is what a caller
//! needs when it knows a rule exists but not which one covers the case.

use crate::spec_text::{SPECS, SUMMARIES};
use crate::util::*;

pub fn cmd_spec(a: &Args) -> R<i32> {
    let want = a.pos.first().cloned().unwrap_or_default();

    if want.is_empty() || want == "index" {
        if a.has("json") {
            let rows = SUMMARIES
                .iter()
                .map(|(n, d)| {
                    let bytes = SPECS
                        .iter()
                        .find(|(k, _)| k == n)
                        .map(|(_, b)| b.len())
                        .unwrap_or(0);
                    format!("{{\"name\":{},\"summary\":{},\"bytes\":{}}}", jstr(n), jstr(d), bytes)
                })
                .collect::<Vec<_>>()
                .join(",");
            crate::say_line!("{{\"specs\":[{}]}}", rows);
            return Ok(0);
        }
        let total: usize = SUMMARIES
            .iter()
            .filter_map(|(n, _)| SPECS.iter().find(|(k, _)| k == n).map(|(_, b)| b.len()))
            .sum();
        crate::say_line!("规范目录（合计 {} B，用 `abook spec <名>` 取全文）", total);
        for (n, d) in SUMMARIES {
            let bytes = SPECS
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, b)| b.len())
                .unwrap_or(0);
            crate::say_line!("  {n:<14} {bytes:>6} B  {d}");
        }
        crate::say_line!("");
        crate::say_line!("规则文本与提示词中曾经内联的版本逐字相同，未经改写或摘要。");
        return Ok(0);
    }

    if want == "list" || want == "commands" {
        for (n, _) in SUMMARIES {
            crate::say_line!("{n}");
        }
        return Ok(0);
    }

    match SPECS.iter().find(|(n, _)| *n == want.as_str()) {
        Some((_, body)) => {
            // `--json` has to be JSON here too. Printing the prose while
            // accepting the flag is the silently-ignored-flag defect this
            // tool has been rooting out everywhere else: a caller asked for
            // machine-readable output and got markdown, then parsed nothing.
            if a.has("json") {
                crate::say_line!(
                    "{{\"name\":{},\"bytes\":{},\"text\":{}}}",
                    jstr(&want),
                    body.len(),
                    jstr(body)
                );
                return Ok(0);
            }
            for line in body.lines() {
                crate::say_line!("{line}");
            }
            Ok(0)
        }
        None => usage(format!(
            "没有名为 `{want}` 的规范；运行 `abook spec` 看全部 {} 个",
            SUMMARIES.len()
        )),
    }
}
