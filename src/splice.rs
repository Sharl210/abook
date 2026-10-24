//! Byte-preserving write-back.
//!
//! Rewriting a whole file to change one field is not something a proxy tool may
//! do: it silently reformats everything it was not asked to touch. This library
//! packs several fields per line, so a full re-render turned
//!
//!     ID=...;PROJECT=...;TASK=...;SESSION=...
//!
//! into four separate lines. Writing a field back with an *identical* value
//! changed the bytes of 92 of 189 candidate records — a data-integrity defect
//! that no test would ever surface, because every reader still saw the right
//! values afterwards.
//!
//! So writes here work on the original bytes: locate the span of each field
//! that actually changed, splice only that span, and leave every other byte
//! alone. When the layout cannot be matched confidently, the caller falls back
//! to a full render rather than guessing.

use crate::parse;
use crate::rec::Lib;

/// The state the record was parsed from.
pub struct Base<'a> {
    pub fields: &'a [(String, String)],
    pub body: &'a str,
    pub promo: Option<&'a str>,
    pub attach: Option<&'a str>,
}

struct FieldLoc {
    key: String,
    key_start: usize,
    value_start: usize,
    value_end: usize,
    line_start: usize,
    line_end: usize,
}

#[derive(Default)]
struct Analysis {
    header_close_start: Option<usize>,
    body_open_end: Option<usize>,
    body_close_start: Option<usize>,
    promo_open_line_start: Option<usize>,
    promo_open_end: Option<usize>,
    promo_close_line_start: Option<usize>,
    promo_close_end: Option<usize>,
    attach_open_line_start: Option<usize>,
    attach_open_end: Option<usize>,
    attach_close_line_start: Option<usize>,
    attach_close_end: Option<usize>,
    spans: Vec<FieldLoc>,
}

fn analyse(raw: &str) -> Analysis {
    let mut a = Analysis::default();
    let mut in_header = false;
    let mut off = 0usize;
    let mut last: Option<usize> = None;
    for line in raw.split_inclusive('\n') {
        let start = off;
        let end = off + line.len();
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        let t = content.trim();
        if t == "[CANDIDATE_MEMORY]" || t == "[CANONICAL_MEMORY]" {
            if a.header_close_start.is_none() && !in_header {
                in_header = true;
            }
        } else if t == "[/CANDIDATE_MEMORY]" || t == "[/CANONICAL_MEMORY]" {
            if a.header_close_start.is_none() {
                a.header_close_start = Some(start);
                in_header = false;
            }
        } else if t == "[MEMORY_BODY]" {
            if a.body_open_end.is_none() {
                a.body_open_end = Some(end);
            }
            in_header = false;
        } else if t == "[/MEMORY_BODY]" {
            if a.body_close_start.is_none() {
                a.body_close_start = Some(start);
            }
        } else if t == "[PROMOTION]" {
            if a.promo_open_end.is_none() {
                a.promo_open_line_start = Some(start);
                a.promo_open_end = Some(end);
            }
        } else if t == "[/PROMOTION]" {
            if a.promo_close_end.is_none() {
                a.promo_close_line_start = Some(start);
                a.promo_close_end = Some(end);
            }
        } else if t == "[ATTACH]" {
            // 附件清单段与晋升段并列，识别方式相同。
            if a.attach_open_end.is_none() {
                a.attach_open_line_start = Some(start);
                a.attach_open_end = Some(end);
            }
        } else if t == "[/ATTACH]" {
            if a.attach_close_end.is_none() {
                a.attach_close_line_start = Some(start);
                a.attach_close_end = Some(end);
            }
        } else if in_header && !t.is_empty() {
            match parse::pairs_with_span(content) {
                Some(pairs) => {
                    for p in pairs {
                        a.spans.push(FieldLoc {
                            key: p.key,
                            key_start: start + p.key_start,
                            value_start: start + p.value_start,
                            value_end: start + p.value_end,
                            line_start: start,
                            line_end: end,
                        });
                    }
                    last = if a.spans.is_empty() {
                        None
                    } else {
                        Some(a.spans.len() - 1)
                    };
                }
                None => {
                    // A line inside the header that is not `KEY=value` continues
                    // the previous value. Extending the span keeps the value's
                    // real extent, so a replacement covers all of it.
                    if let Some(i) = last {
                        a.spans[i].value_end = start + content.trim_end().len();
                        a.spans[i].line_end = end;
                    }
                }
            }
        }
        off = end;
    }
    a
}


/// Why a splice was abandoned. Silent by default; set `ABOOK_SPLICE_DEBUG=1`
/// to see it. A fallback is always safe (the caller re-renders), but knowing
/// the reason is what turns "it fell back" into a fixable layout fact.
fn fail(why: &str) -> Option<String> {
    if std::env::var_os("ABOOK_SPLICE_DEBUG").is_some() {
        eprintln!("[splice] 回退整文件重渲染: {why}");
    }
    None
}

/// Rewrite `raw` so that it reflects `fields` / `body` / `promo`, touching
/// only the bytes that differ from `base`.
///
/// Returns `None` when the layout cannot be matched confidently, or when the
/// spliced result does not parse back into exactly the intended record. The
/// caller then falls back to a full render, so a layout surprise can never
/// produce a corrupt file.
pub fn write_back(
    raw: &str,
    base: Base<'_>,
    lib: Lib,
    fields: &[(String, String)],
    body: &str,
    promo: Option<&str>,
    attach: Option<&str>,
) -> Option<String> {
    if raw.is_empty() {
        return fail("空文件");
    }
    let an = analyse(raw);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();

    let base_of = |k: &str| -> Option<String> {
        base.fields
            .iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| v.clone())
    };
    let locs_of = |k: &str| -> Vec<usize> {
        an.spans
            .iter()
            .enumerate()
            .filter(|(_, s)| s.key == k)
            .map(|(i, _)| i)
            .collect()
    };

    // ---- fields that are new or changed
    for (k, v) in fields {
        match base_of(k) {
            Some(old) if old == *v => {}
            Some(_) => {
                // Span offsets point into `raw`; `edits` uses the same space.
                let idxs = locs_of(k);
                if idxs.is_empty() {
                    return fail(&format!("头部找不到字段 {k} 的位置"));
                }
                for i in idxs {
                    let s = &an.spans[i];
                    edits.push((
                        s.value_start,
                        // only the last duplicate keeps the trailing extent
                        s.value_end,
                        v.clone(),
                    ));
                }
            }
            None => {
                // A brand new field: give it its own line before the header
                // closes, so no existing line is disturbed.
                let at = match an.header_close_start {
                    Some(x) => x,
                    None => return fail("没有头部闭合标签，无法插入新字段"),
                };
                edits.push((at, at, format!("{k}={v}\n")));
            }
        }
    }

    // ---- fields that disappeared
    for (k, _) in base.fields {
        if fields.iter().any(|(kk, _)| kk == k) {
            continue;
        }
        let idxs = locs_of(k);
        if idxs.is_empty() {
            continue;
        }
        for i in idxs {
            let s = &an.spans[i];
            let line_content_end = raw[s.line_start..s.line_end]
                .trim_end_matches('\n')
                .trim_end_matches('\r')
                .len()
                + s.line_start;
            // Prefer eating the separator that follows the field; fall back to
            // the one that precedes it so `A=1;B=2` stays a single clean line.
            let (ds, de) = if raw[s.value_end..line_content_end]
                .trim_start()
                .starts_with(';')
            {
                let semi = s.value_end
                    + raw[s.value_end..line_content_end]
                        .find(';')
                        .unwrap_or(0);
                let mut e = semi + 1;
                while e < line_content_end && raw.as_bytes()[e] == b' ' {
                    e += 1;
                }
                (s.key_start, e.min(line_content_end))
            } else if raw[s.line_start..s.key_start].trim_end().ends_with(';') {
                let before = &raw[s.line_start..s.key_start];
                let semi = s.line_start + before.trim_end().len() - 1;
                (semi, s.value_end)
            } else {
                (s.key_start, s.value_end)
            };
            // If that leaves nothing on the line, drop the whole line.
            let rest_left = &raw[s.line_start..ds];
            let rest_right = &raw[de..line_content_end];
            if rest_left.trim().is_empty() && rest_right.trim().is_empty() {
                edits.push((s.line_start, s.line_end, String::new()));
            } else {
                edits.push((ds, de, String::new()));
            }
        }
    }

    // ---- body
    if body != base.body {
        let (open, close) = match (an.body_open_end, an.body_close_start) {
            (Some(o), Some(c)) => (o, c),
            _ => return fail("正文标签缺失"),
        };
        let mut text = body.to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        edits.push((open, close, text));
    }

    // ---- promotion block
    match (base.promo, promo) {
        (Some(old), Some(new)) if old == new => {}
        (Some(_), Some(new)) => {
            let open = an.promo_open_end?;
            let close = an.promo_close_line_start?;
            let mut text = new.trim_matches('\n').to_string();
            if !text.is_empty() {
                text.push('\n');
            }
            edits.push((open, close, text));
        }
        (None, Some(new)) => {
            let at = an.promo_close_end.or(an.body_close_end(raw))?;
            let mut text = String::from("[PROMOTION]\n");
            text.push_str(new.trim_matches('\n'));
            text.push_str("\n[/PROMOTION]\n");
            edits.push((at, at, text));
        }
        (Some(_), None) => {
            let s = an.promo_open_line_start?;
            let e = an.promo_close_end?;
            // swallow the closing line's newline too
            let e = raw[e..].starts_with('\n')
                .then_some(e + 1)
                .unwrap_or(e);
            edits.push((s, e, String::new()));
        }
        (None, None) => {}
    }

    // ---- attach block
    //
    // 和晋升段同构：改内容就地替换，首次出现就追加到文件末尾。追加点选在
    // 文件尾而不是正文之后——附件清单是目录页，位置固定在最后，读的人往下
    // 翻到底就能看到全部附件，不必先去列目录。
    match (base.attach, attach) {
        (Some(old), Some(new)) if old == new => {}
        (Some(_), Some(new)) => {
            let open = an.attach_open_end?;
            let close = an.attach_close_line_start?;
            let mut text = new.trim_matches('\n').to_string();
            if !text.is_empty() {
                text.push('\n');
            }
            edits.push((open, close, text));
        }
        (None, Some(new)) => {
            let at = raw.len();
            let mut text = String::new();
            if !raw.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("[ATTACH]\n");
            text.push_str(new.trim_matches('\n'));
            text.push_str("\n[/ATTACH]\n");
            edits.push((at, at, text));
        }
        (Some(_), None) => {
            let s0 = an.attach_open_line_start?;
            let e0 = an.attach_close_end?;
            let e0 = raw[e0..].starts_with('\n').then_some(e0 + 1).unwrap_or(e0);
            edits.push((s0, e0, String::new()));
        }
        (None, None) => {}
    }

    if edits.is_empty() {
        return Some(raw.to_string());
    }

    // Apply forward, copying the untouched stretches verbatim between edits.
    // A zero-width insert (s == e) is ordered before a replacement starting at
    // the same offset so it lands in front of it.
    edits.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    let mut out = String::with_capacity(raw.len() + 256);
    let mut cursor = 0usize;
    for (s, e, text) in edits {
        if s < cursor || s > e || e > raw.len() {
            return fail(&format!(
                "编辑区间越界或重叠 (游标 {cursor}, 区间 {s}..{e}, 文长 {})",
                raw.len()
            ));
        }
        out.push_str(&raw[cursor..s]);
        out.push_str(&text);
        cursor = e;
    }
    out.push_str(&raw[cursor..]);

    // Prove the splice means what it should before letting it reach the disk.
    let check = parse::parse(&out, Some(lib));
    if check.has_error() {
        if std::env::var_os("ABOOK_SPLICE_DEBUG").is_some() {
            let msgs: Vec<String> = check.errors().iter().map(|d| d.show()).collect();
            eprintln!("[splice] 解析错误: {}", msgs.join(" | "));
            let mut path = std::env::temp_dir();
            path.push("abook_splice_bad.txt");
            let _ = std::fs::write(&path, &out);
            eprintln!("[splice] 拼接结果已存 {}", path.display());
        }
        return fail("拼接结果解析出错");
    }
    // The parser normalises the body by stripping its own leading/trailing
    // newlines, so compare content rather than the wrapper's newline habit:
    // otherwise a caller that passes a trailing newline is judged inconsistent
    // and the whole write falls back to a full re-render, which is exactly the
    // reformatting this module exists to avoid.
    if check.body.trim_matches('\n') != body.trim_matches('\n') {
        return fail(&format!(
            "拼接后正文不一致 (期望 {} 字节, 实得 {} 字节)",
            body.len(),
            check.body.len()
        ));
    }
    for (k, v) in fields {
        let got = check
            .fields
            .iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, vv)| vv.clone());
        if got.as_deref() != Some(v.as_str()) {
            return fail(&format!("拼接后字段 {k} 的值不符 (期望 {:?}, 实得 {:?})", v, got));
        }
    }
    if check.fields.len() != fields.len() {
        return fail(&format!(
            "拼接后字段数不同 (期望 {}, 实得 {}): 期望键 {:?} / 实得键 {:?}",
            fields.len(),
            check.fields.len(),
            fields.iter().map(|(k,_)| k.as_str()).collect::<Vec<_>>(),
            check.fields.iter().map(|(k,_)| k.as_str()).collect::<Vec<_>>()
        ));
    }
    if check.promotion.as_deref().map(|s| s.trim_matches('\n'))
        != promo.map(|s| s.trim_matches('\n'))
    {
        return fail("晋升段不一致");
    }
    Some(out)
}

impl Analysis {
    fn body_close_end(&self, raw: &str) -> Option<usize> {
        // Append point when there is no promotion block yet: right after the
        // body's closing tag line.
        let s = self.body_close_start?;
        let rest = &raw[s..];
        let line_end = rest.find('\n').map(|i| s + i + 1).unwrap_or(raw.len());
        Some(line_end)
    }
}
