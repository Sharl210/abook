//! Tolerant record parsing.
//!
//! Design stance: the parser must not assume the writer got the format right.
//! Models do write `READ-WHEN=`, forget the closing tag, mix in full-width
//! punctuation, or pack four fields onto one line. None of that may cause a
//! silent misread.
//!
//! Every deviation from canonical form is recorded as a `Deviation` carrying
//! the rule id, the original bytes, and what the parser did about it. Callers
//! can therefore tell "this record was clean" from "this record was repaired
//! on the fly". `--strict` turns every deviation into a hard error, so the
//! tolerance is inspectable rather than invisible.

use crate::rec::Lib;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sev {
    /// The record cannot be understood; parsing fails.
    Error,
    /// Parsed, but something was suspicious enough that repair is advised.
    Warn,
    /// The parser normalised it silently-safe; the text is recorded.
    Fixed,
}

impl Sev {
    pub fn label(self) -> &'static str {
        match self {
            Sev::Error => "ERROR",
            Sev::Warn => "WARN",
            // "FIXED" read as "the file was changed", and `check` promised it
            // only reports. These deviations are normalised in memory during
            // parsing; the file on disk is untouched. Name it for what it is.
            Sev::Fixed => "NORMALIZED",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Deviation {
    pub rule: &'static str,
    pub sev: Sev,
    pub line: usize,
    pub original: String,
    pub action: String,
}

impl Deviation {
    pub fn show(&self) -> String {
        let where_ = if self.line > 0 {
            format!("行{}", self.line)
        } else {
            "-".to_string()
        };
        format!(
            "[{}] {} {} :: {} → {}",
            self.sev.label(),
            self.rule,
            where_,
            crate::util::truncate_chars(&self.original, 90),
            self.action
        )
    }
}

// ---------------------------------------------------------------- aliases

/// Structural key aliases. Anything a model plausibly writes maps here.
const ALIASES: &[(&[&str], &str)] = &[
    (&["READ_WHEN", "READWHEN", "WHEN_TO_READ", "WHEN"], "READ_WHEN"),
    (&["NOT_FOR", "NOTFOR", "NOT_APPLICABLE", "EXCLUDE"], "NOT_FOR"),
    (&["NEXT_VALIDATION", "NEXTVALIDATION", "VALIDATE_NEXT"], "NEXT_VALIDATION"),
    (&["RELATED_IDS", "RELATEDIDS", "RELATED", "REFS"], "RELATED_IDS"),
    (&["PREVIOUS_ID", "PREVIOUSID", "PREV"], "PREVIOUS_ID"),
    (&["PROMOTED_FROM", "PROMOTEDFROM"], "PROMOTED_FROM"),
    (&["PROMOTED_AT", "PROMOTEDAT"], "PROMOTED_AT"),
    (&["PROMOTED_TO", "PROMOTEDTO"], "PROMOTED_TO"),
    (&["SUPERSEDES", "SUPERSEDE", "REPLACES"], "SUPERSEDES"),
    (&["CONCEPTS", "CONCEPT", "TOPICS"], "CONCEPTS"),
    (&["ENTITIES", "ENTITY"], "ENTITIES"),
    (&["SEQUENCE", "SEQ", "ORDER"], "SEQUENCE"),
    (&["SUMMARY", "ABSTRACT", "DESC"], "SUMMARY"),
    (&["BRIEF", "SHORT", "ONELINE"], "BRIEF"),
    (&["MATURITY", "MATURE"], "MATURITY"),
    (&["AUTHORITY", "AUTH"], "AUTHORITY"),
    (&["EVIDENCE", "PROOF"], "EVIDENCE"),
    (&["UNCERTAINTY", "UNKNOWN"], "UNCERTAINTY"),
    (&["SOURCE", "SRC", "ORIGIN"], "SOURCE"),
    (&["STATUS", "STATE"], "STATUS"),
    (&["SECTIONS", "SECTION"], "SECTIONS"),
];

/// Fold one raw key spelling to its canonical name.
/// Returns (canonical, was_alias).
pub fn canon_key(raw: &str) -> (String, bool) {
    let s = raw.trim();
    // normalise separators a model might use inside a key
    let up: String = s
        .chars()
        .map(|c| match c {
            '-' | ' ' | '.' | '·' => '_',
            c => c.to_ascii_uppercase(),
        })
        .collect();
    let up = up.trim_matches('_').to_string();
    let up = {
        let mut t = up.clone();
        while t.contains("__") {
            t = t.replace("__", "_");
        }
        t
    };
    for (spellings, canon) in ALIASES {
        if spellings.contains(&up.as_str()) {
            return (canon.to_string(), up != *canon);
        }
    }
    (up, false)
}

/// Read one `KEY` + separator at `start_byte`, from the ORIGINAL line.
/// Returns (canonical key, byte offset where the value begins).
/// Works on raw bytes so the value is never re-cased or punctuation-folded.
fn key_end_at(line: &str, start_byte: usize) -> Option<(String, usize)> {
    if start_byte >= line.len() {
        return None;
    }
    let rest = &line[start_byte..];
    let lead = rest.len() - rest.trim_start().len();
    let s = &rest[lead..];
    if s.is_empty() {
        return None;
    }
    // A key starts with an ASCII letter.
    //
    // Chinese keys such as `注意=` do exist, but they appear inside the body,
    // never in a header. Accepting CJK here made two prose lines parse as
    // fields and, worse, let `;`-separated prose invent new keys:
    //   EVIDENCE=...不变;围栏18=18;符号定义265=265无丢失...
    // turned one value into three bogus fields (`围栏18`, `符号定义265`,
    // `悬空引用63`) and truncated the real EVIDENCE at the first `;`.
    // Across the live library the header holds 11,558 key lines and every one
    // of them is `^[A-Z][A-Z0-9_]*=`; the only four non-ASCII starts were
    // orphan continuation lines. Requiring ASCII keeps the real keys and lets
    // those four be recognised as continuations (recorded as K004) instead of
    // silently minted as fields.
    let first = s.chars().next().unwrap();
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let mut end = 0usize;
    let mut sep_len = 0usize;
    for (i, c) in s.char_indices() {
        if c == '=' {
            end = i;
            sep_len = 1;
            break;
        }
        if c == '\u{ff1d}' || c == '\u{2261}' {
            end = i;
            sep_len = c.len_utf8();
            break;
        }
        let ok = c.is_ascii_alphanumeric() || c == '_' || c == '-';
        if !ok {
            return None;
        }
        if i > 64 {
            return None;
        }
    }
    if sep_len == 0 || end == 0 {
        return None;
    }
    let keyraw = s[..end].trim();
    if keyraw.is_empty() {
        return None;
    }
    let (canon, _) = canon_key(keyraw);
    if canon.is_empty() {
        return None;
    }
    Some((canon, start_byte + lead + end + sep_len))
}

/// Split one line into (key, value) pairs, keeping values byte-exact.
/// A `;` only separates when what follows really starts another key, so a
/// SUMMARY full of Chinese semicolons survives untouched.
fn extract_pairs(line: &str) -> Option<Vec<(String, String)>> {
    let (mut k, mut vstart) = key_end_at(line, 0)?;
    let mut out: Vec<(String, String)> = Vec::new();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 200 {
            break;
        }
        let mut cut: Option<(usize, usize)> = None;
        // Only the HALF-WIDTH semicolon separates fields. The full-width one
        // occurs 2836 times in this library and never once as a separator.
        for (i, c) in line[vstart..].char_indices() {
            if c == ';' {
                let after = vstart + i + c.len_utf8();
                // The key must be one this library actually uses, not merely
                // something shaped like a key. Without that gate, a URL inside
                // a value splits the value: `.../mt6990.git;a=shortlog;h=refs/
                // heads/x` used to cut SUMMARY at 44 chars and invent two
                // fields named `A` and `H`. Across all 960 records this gate
                // leaves 6371 real separators untouched and stops exactly the
                // 4 bogus ones.
                if let Some((kk, _)) = key_end_at(line, after) {
                    if crate::rec::is_known_key(&kk) {
                        cut = Some((vstart + i, after));
                        break;
                    }
                }
            }
        }
        match cut {
            Some((c, after)) => {
                out.push((k.clone(), line[vstart..c].trim().to_string()));
                match key_end_at(line, after) {
                    Some((kk, vs)) => {
                        k = kk;
                        vstart = vs;
                    }
                    None => break,
                }
            }
            None => {
                // A trailing `;` with nothing after it is a dangling separator,
                // not part of the value. Half-width `;` never separates fields
                // unless a valid key follows, so stripping is safe here; the
                // caller records the change as a deviation.
                let raw_tail = &line[vstart..];
                let trimmed = raw_tail.trim();
                let val = match trimmed.strip_suffix(';') {
                    Some(head) => head.trim_end().to_string(),
                    None => trimmed.to_string(),
                };
                out.push((k, val));
                break;
            }
        }
    }
    Some(out)
}

/// One field on one physical line, with the byte ranges needed to rewrite it
/// without touching anything else.
///
/// `key_start..value_start` covers `KEY=`, and `value_start..value_end` is the
/// value itself (already trimmed of surrounding spaces so replacements keep the
/// original spacing). Offsets are relative to the line passed in.
pub struct PairSpan {
    pub key: String,
    pub key_start: usize,
    pub value_start: usize,
    pub value_end: usize,
}

/// Same splitting rule as [`extract_pairs`], but keeping byte offsets.
///
/// The tool must be able to change one field without rewriting its neighbours:
/// this library packs `ID=...;PROJECT=...;TASK=...;SESSION=...` on a single
/// line, so a whole-file re-render silently expands one line into four.
pub fn pairs_with_span(line: &str) -> Option<Vec<PairSpan>> {
    let (mut k, mut vstart) = key_end_at(line, 0)?;
    let key_start0 = line.len() - line.trim_start().len();
    let mut key_start = key_start0;
    let mut out: Vec<PairSpan> = Vec::new();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 500 {
            break;
        }
        let mut cut: Option<(usize, usize)> = None;
        for (i, c) in line[vstart.min(line.len())..].char_indices() {
            if c == ';' {
                let after = vstart + i + c.len_utf8();
                if key_end_at(line, after).is_some() {
                    cut = Some((vstart + i, after));
                    break;
                }
            }
        }
        match cut {
            Some((c, after)) => {
                out.push(span_of(&k, line, key_start, vstart, c));
                match key_end_at(line, after) {
                    Some((kk, vs)) => {
                        // the next key starts just after the separator, possibly
                        // after spaces
                        let rest = &line[after..];
                        let lead = rest.len() - rest.trim_start().len();
                        key_start = after + lead;
                        k = kk;
                        vstart = vs;
                    }
                    None => break,
                }
            }
            None => {
                let mut vend = line.trim_end().len();
                if vend > vstart && line[..vend].ends_with(';') {
                    vend -= 1;
                }
                out.push(span_of(&k, line, key_start, vstart, vend));
                break;
            }
        }
    }
    Some(out)
}

fn span_of(key: &str, line: &str, key_start: usize, vstart: usize, vend: usize) -> PairSpan {
    let inner = &line[vstart.min(vend)..vend];
    let lead = inner.len() - inner.trim_start().len();
    let trail = inner.len() - inner.trim_end().len();
    PairSpan {
        key: key.to_string(),
        key_start,
        value_start: (vstart + lead).min(vend),
        value_end: vend - trail,
    }
}

// ---------------------------------------------------------------- normalise

/// Byte-level clean-up that cannot change meaning inside prose.
pub fn normalize(src: &str, devs: &mut Vec<Deviation>) -> String {
    let mut s = src.to_string();
    if s.starts_with('\u{feff}') {
        s = s.trim_start_matches('\u{feff}').to_string();
        devs.push(Deviation {
            rule: "N001",
            sev: Sev::Fixed,
            line: 0,
            original: "UTF-8 BOM".into(),
            action: "已剥离 BOM".into(),
        });
    }
    if s.contains('\r') {
        s = s.replace("\r\n", "\n").replace('\r', "\n");
        devs.push(Deviation {
            rule: "N002",
            sev: Sev::Fixed,
            line: 0,
            original: "CRLF/CR 换行".into(),
            action: "统一为 LF".into(),
        });
    }
    s
}

/// Fold full-width forms that only ever appear as structural punctuation.
/// Applied per line, and only to the part before the value when parsing keys.
fn fold_struct(ch: char) -> Option<char> {
    Some(match ch {
        '［' => '[',
        '］' => ']',
        '＝' => '=',
        '；' => ';',
        '：' => ':',
        '，' => ',',
        '｜' => '|',
        '　' => ' ',
        _ => return None,
    })
}

fn fold_line(line: &str) -> String {
    line.chars().map(|c| fold_struct(c).unwrap_or(c)).collect()
}

// ---------------------------------------------------------------- tags

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tag {
    HeaderOpen,
    HeaderClose,
    BodyOpen,
    BodyClose,
    PromoOpen,
    PromoClose,
    /// `[ATTACH]` — 附件清单块。
    ///
    /// 存成尾部块而不是头部字段，原因和 [PROMOTION] 一样：清单是多行的
    /// （一条记录可以有几十个附件），头部字段是单行 `KEY=VALUE`，塞不下。
    AttachOpen,
    AttachClose,
}

/// Recognise a structural tag line tolerantly: brackets optional, case
/// insensitive, spacing flexible.
fn as_tag(line: &str) -> Option<(Tag, bool)> {
    let t = line.trim().trim_start_matches('\u{feff}').trim();
    if t.is_empty() {
        return None;
    }
    // reject obvious prose early: tags are short
    if t.chars().count() > 40 {
        return None;
    }
    let folded = fold_line(t);
    let mut s = folded.trim().to_string();
    let mut had_bracket = false;
    // Accept [TAG] and [/TAG] and <TAG> and bare TAG, plus END_TAG / CLOSE_TAG.
    for (open, close) in [('[', ']'), ('<', '>'), ('【', '】'), ('〔', '〕')] {
        if s.starts_with(open) && s.ends_with(close) && s.chars().count() >= 2 {
            had_bracket = true;
            let inner: String = s
                .chars()
                .skip(1)
                .take(s.chars().count() - 2)
                .collect();
            s = inner;
            break;
        }
    }
    if !had_bracket {
        // one-sided or naked: strip stray brackets and a leading slash
        s = s
            .trim_matches(|c| c == '[' || c == ']' || c == '<' || c == '>')
            .to_string();
    }
    let s = s.trim().trim_end_matches(':').trim().to_string();
    let up: String = s
        .chars()
        .map(|c| if c == '-' || c == ' ' || c == '_' { '_' } else { c.to_ascii_uppercase() })
        .collect();
    let up = {
        let mut t = up.clone();
        while t.contains("__") {
            t = t.replace("__", "_");
        }
        t.trim_matches('_').to_string()
    };
    // `/TAG` is the house spelling for a closing tag.
    let slash_close = up.starts_with('/') || up.starts_with('\\');
    let up = up.trim_start_matches('/').trim_start_matches('\\').to_string();
    let closing = slash_close || up.starts_with("END_") || up.starts_with("CLOSE_");
    let base = up
        .trim_start_matches("END_")
        .trim_start_matches("CLOSE_")
        .to_string();
    let tag = match base.as_str() {
        "CANDIDATE_MEMORY" | "CANDIDATE" | "MEMORY" | "CANDIDATE_MEMORY_V2" => {
            if closing { Tag::HeaderClose } else { Tag::HeaderOpen }
        }
        "CANONICAL_MEMORY" | "CANONICAL" | "CANONICAL_MEMORY_V2" => {
            if closing { Tag::HeaderClose } else { Tag::HeaderOpen }
        }
        "MEMORY_BODY" | "BODY" | "CONTENT" => {
            if closing { Tag::BodyClose } else { Tag::BodyOpen }
        }
        "PROMOTION" | "PROMOTED" => {
            if closing { Tag::PromoClose } else { Tag::PromoOpen }
        }
        "ATTACH" | "ATTACHMENTS" | "FILES" => {
            if closing { Tag::AttachClose } else { Tag::AttachOpen }
        }
        _ => return None,
    };
    Some((tag, had_bracket))
}

// ---------------------------------------------------------------- result

#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub lib_hint: Option<Lib>,
    pub fields: Vec<(String, String)>,
    pub body: String,
    pub promotion: Option<String>,
    /// 附件清单块的原始内容（块内文本，不含标签行）。
    pub attach: Option<String>,
    pub devs: Vec<Deviation>,
    pub had_header_tag: bool,
    pub body_inferred: bool,
}

impl Parsed {
    pub fn get(&self, k: &str) -> String {
        self.fields
            .iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
    pub fn has(&self, k: &str) -> bool {
        self.fields.iter().any(|(kk, _)| kk == k)
    }
    pub fn errors(&self) -> Vec<&Deviation> {
        self.devs.iter().filter(|d| d.sev == Sev::Error).collect()
    }
    pub fn has_error(&self) -> bool {
        self.devs.iter().any(|d| d.sev == Sev::Error)
    }
}

/// Parse one record body from raw text.
pub fn parse(src: &str, lib_hint: Option<Lib>) -> Parsed {
    let mut devs: Vec<Deviation> = Vec::new();
    let text = normalize(src, &mut devs);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Parsed {
        lib_hint,
        ..Default::default()
    };

    // ---- locate tags
    let mut header_open: Option<usize> = None;
    let mut header_close: Option<usize> = None;
    let mut body_open: Option<usize> = None;
    let mut body_close: Option<usize> = None;
    let mut promo_open: Option<usize> = None;
    let mut promo_close: Option<usize> = None;
    let mut attach_open: Option<usize> = None;
    let mut attach_close: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some((tag, bracketed)) = as_tag(line) {
            if !bracketed {
                devs.push(Deviation {
                    rule: "T001",
                    sev: Sev::Fixed,
                    line: i + 1,
                    original: line.trim().to_string(),
                    action: "标签缺方括号，已按标签识别".into(),
                });
            }
            match tag {
                Tag::HeaderOpen => {
                    if header_open.is_none() {
                        header_open = Some(i);
                    }
                    // lib hint from the tag spelling
                    let folded = fold_line(line).to_uppercase();
                    if out.lib_hint.is_none() {
                        out.lib_hint = if folded.contains("CANDIDATE") {
                            Some(Lib::Candidate)
                        } else {
                            Some(Lib::Canonical)
                        };
                    }
                }
                // a close without an open happened before: remember the last one
                Tag::HeaderClose => {
                    if header_close.is_none() {
                        header_close = Some(i);
                    }
                }
                Tag::BodyOpen => {
                    if body_open.is_none() {
                        body_open = Some(i);
                    }
                }
                Tag::BodyClose => {
                    if body_close.is_none() {
                        body_close = Some(i);
                    }
                }
                Tag::PromoOpen => {
                    if promo_open.is_none() {
                        promo_open = Some(i);
                    }
                }
                Tag::PromoClose => {
                    if promo_close.is_none() {
                        promo_close = Some(i);
                    }
                }
                Tag::AttachOpen => {
                    if attach_open.is_none() {
                        attach_open = Some(i);
                    }
                }
                Tag::AttachClose => {
                    if attach_close.is_none() {
                        attach_close = Some(i);
                    }
                }
            }
        }
    }

    // A close tag appearing before its open tag is a real hazard; if the only
    // header marker is a close tag, treat it as the open (and note it).
    if header_open.is_none() && header_close.is_some() {
        header_open = header_close;
        header_close = None;
        devs.push(Deviation {
            rule: "T002",
            sev: Sev::Warn,
            line: 0,
            original: "只有闭合标签".into(),
            action: "按开启标签处理".into(),
        });
    }
    // A second closing tag after the body is a real defect seen in 12 files.
    if let (Some(first), Some(bo)) = (header_close, body_open) {
        for (i, line) in lines.iter().enumerate().skip(bo) {
            if let Some((Tag::HeaderClose, _)) = as_tag(line) {
                if i > first {
                    devs.push(Deviation {
                        rule: "T009",
                        sev: Sev::Warn,
                        line: i + 1,
                        original: line.trim().to_string(),
                        action: "多余的头部闭合标签（正文之后），已忽略".into(),
                    });
                    break;
                }
            }
        }
    }
    out.had_header_tag = header_open.is_some();
    if header_open.is_none() {
        devs.push(Deviation {
            rule: "T003",
            sev: Sev::Warn,
            line: 0,
            original: "缺少头部标签".into(),
            action: "按首个 KEY= 行自动推断头部范围".into(),
        });
    }

    // ---- header range
    // A few records put KEY=value lines *above* the header tag. Starting at
    // `header_open + 1` silently dropped them (a record's own ID would read as
    // empty), so walk backwards over any leading key lines and absorb them.
    let mut hstart = match header_open {
        Some(i) => i + 1,
        None => 0,
    };
    if let Some(open) = header_open {
        let mut first_key = open;
        while first_key > 0 && extract_pairs(lines[first_key - 1]).is_some() {
            first_key -= 1;
        }
        if first_key < open {
            devs.push(Deviation {
                rule: "T010",
                sev: Sev::Fixed,
                line: first_key + 1,
                original: format!("头部标签之前有 {} 个字段行", open - first_key),
                action: "并入头部解析".into(),
            });
            hstart = first_key;
        }
    }
    // when the close tag is missing, the header ends where the body starts, or
    // at the first line that is clearly prose.
    let mut hend = match header_close {
        Some(i) if i >= hstart => i,
        _ => lines.len(),
    };
    if let Some(b) = body_open {
        if b < hend {
            hend = b;
        }
    }
    if header_close.is_none() && body_open.is_none() {
        // no tags at all: infer the end of the key block
        let mut last_key = hstart;
        for i in hstart..lines.len() {
            let l = lines[i];
            if l.trim().is_empty() {
                continue;
            }
            if extract_pairs(l).is_some() {
                last_key = i + 1;
            } else {
                break;
            }
        }
        hend = last_key;
        out.body_inferred = true;
    } else if header_close.is_none() {
        devs.push(Deviation {
            rule: "T004",
            sev: Sev::Warn,
            line: 0,
            original: "头部标签未闭合".into(),
            action: "以正文起始标签为界".into(),
        });
    }

    // ---- parse fields (values always come from the original bytes)
    let mut pending: Option<usize> = None; // index into out.fields
    for i in hstart..hend.min(lines.len()) {
        let raw = lines[i];
        if raw.trim().is_empty() {
            continue;
        }
        match extract_pairs(raw) {
            Some(pairs) => {
                // Several fields on one line is this library's house style
                // (~1738 lines across 98% of files), not a deviation.
                let _ = pairs.len();
                // a key spelled with a dash or odd case is worth noting
                let head = raw.trim_start();
                let raw_key: String = head
                    .chars()
                    .take_while(|c| *c != '=' && *c != '\u{ff1d}')
                    .collect();
                let (canon_head, was_alias) = canon_key(&raw_key);
                if was_alias {
                    devs.push(Deviation {
                        rule: "K003",
                        sev: Sev::Fixed,
                        line: i + 1,
                        original: raw_key.trim().to_string(),
                        action: format!("别名归一为 {canon_head}"),
                    });
                }
                for (k, v) in pairs {
                    push_field(&mut out, &mut devs, &k, &v, i + 1, raw);
                }
                if !out.fields.is_empty() {
                    pending = Some(out.fields.len() - 1);
                }
            }
            None => {
                // continuation of the previous value
                match pending {
                    Some(idx) => {
                        if !out.fields[idx].1.is_empty() {
                            out.fields[idx].1.push('\n');
                        }
                        out.fields[idx].1.push_str(raw.trim_end());
                        devs.push(Deviation {
                            rule: "K004",
                            sev: Sev::Fixed,
                            line: i + 1,
                            original: crate::util::truncate_chars(raw.trim_end(), 70),
                            action: format!("续接到 {}", out.fields[idx].0),
                        });
                    }
                    None => {
                        devs.push(Deviation {
                            rule: "K005",
                            sev: Sev::Warn,
                            line: i + 1,
                            original: crate::util::truncate_chars(raw.trim_end(), 70),
                            action: "头部内无法归属的行，已忽略".into(),
                        });
                    }
                }
            }
        }
    }

    // ---- body: from body_open to the LAST body_close (a literal close tag
    // inside prose must not truncate the record)
    let mut bstart = match body_open {
        Some(i) => i + 1,
        None => hend.min(lines.len()),
    };
    // Use the FIRST closing tag after the body opened. Using the last one
    // would swallow a literal `[MEMORY_BODY]` that appears inside prose.
    let mut bend = match body_close {
        Some(i) if i >= bstart => i,
        _ => {
            if body_open.is_some() {
                devs.push(Deviation {
                    rule: "T005",
                    sev: Sev::Warn,
                    line: 0,
                    original: "正文标签未闭合".into(),
                    action: "取到文件末尾（或晋升段之前）".into(),
                });
            }
            {
                // 正文止于第一个尾部块：晋升段或附件清单段，谁先到谁算。
                let a = attach_open.filter(|p| *p >= bstart);
                let p_ = promo_open.filter(|p| *p >= bstart);
                match (a, p_) {
                    (Some(x), Some(y)) => x.min(y),
                    (Some(x), None) => x,
                    (None, Some(y)) => y,
                    (None, None) => lines.len(),
                }
            }
        }
    };
    if body_open.is_none() && !lines.is_empty() {
        // No body tag at all. Everything after the header would be body, but
        // that range also contains the header close tag and any promotion
        // block -- treating it verbatim made such a record's "body" literally
        // read `[/CANDIDATE_MEMORY]`. Trim structural tags and the promotion
        // block out of the inferred range instead.
        if let Some(close) = header_close {
            if bstart <= close {
                bstart = close + 1;
            }
        }
        // The promotion block is a separate section, never body.
        if let Some(p) = promo_open.filter(|p| *p >= bstart) {
            bend = p;
        }
        if let Some(p) = attach_open.filter(|p| *p >= bstart) {
            if p < bend {
                bend = p;
            }
        }
        if bstart < lines.len() && !out.body_inferred {
            devs.push(Deviation {
                rule: "T006",
                sev: Sev::Warn,
                line: bstart + 1,
                original: "缺少正文标签".into(),
                action: "头部之后全文视为正文".into(),
            });
        }
    }
    if bstart < bend {
        out.body = lines[bstart..bend].join("\n").trim_matches('\n').to_string();
    }
    // A tag spelled out inside the prose is a trap for any line-based parser.
    let literal = out.body.matches("[MEMORY_BODY]").count()
        + out.body.matches("[/MEMORY_BODY]").count();
    if literal > 0 {
        devs.push(Deviation {
            rule: "T008",
            sev: Sev::Warn,
            line: 0,
            original: format!("正文内含 {literal} 处标签字面量"),
            action: "已按首个闭合标签截取，正文其余部分未被吞掉".into(),
        });
    }

    // ---- promotion block
    if let Some(po) = promo_open {
        let pc = match promo_close {
            Some(c) if c >= po => c,
            _ => lines.len(),
        };
        let mut s = lines[po + 1..pc.min(lines.len())].join("\n").trim_end().to_string();
        if promo_close.is_none() && po + 1 < lines.len() {
            devs.push(Deviation {
                rule: "T007",
                sev: Sev::Warn,
                line: po + 1,
                original: "晋升段未闭合".into(),
                action: "取到文件末尾".into(),
            });
        }
        // promotion fields are key=value too; normalise the separators
        s = s
            .lines()
            .map(|l| fold_line(l))
            .collect::<Vec<_>>()
            .join("\n");
        if !s.trim().is_empty() {
            out.promotion = Some(s);
        }
    }

    // ---- attach block
    //
    // 与晋升段并列的尾部块，位置无关：两者顺序互换也能解析。
    if let Some(ao) = attach_open {
        let ac = match attach_close {
            Some(c) if c >= ao => c,
            _ => lines.len(),
        };
        let mut s = lines[ao + 1..ac.min(lines.len())].join("\n").trim_end().to_string();
        if attach_close.is_none() && ao + 1 < lines.len() {
            devs.push(Deviation {
                rule: "T009",
                sev: Sev::Warn,
                line: ao + 1,
                original: "附件段未闭合".into(),
                action: "取到文件末尾".into(),
            });
        }
        // 附件清单是 `名	字节` 的行，不 fold：文件名里可能带 ';' 或 '='
        if !s.trim().is_empty() {
            out.attach = Some(s);
        }
    }

    // ---- record-level sanity
    if out.fields.is_empty() {
        devs.push(Deviation {
            rule: "R001",
            sev: Sev::Error,
            line: 0,
            original: "没有任何可识别字段".into(),
            action: "无法解析为记录".into(),
        });
    }
    if out.body.trim().is_empty() && !out.has("TITLE") {
        devs.push(Deviation {
            rule: "R002",
            sev: Sev::Error,
            line: 0,
            original: "既无正文也无标题".into(),
            action: "无法解析为记录".into(),
        });
    }
    out.devs = devs;
    out
}

fn push_field(
    out: &mut Parsed,
    devs: &mut Vec<Deviation>,
    key: &str,
    val: &str,
    line: usize,
    raw: &str,
) {
    if key.is_empty() {
        return;
    }
    if let Some(existing) = out.fields.iter_mut().find(|(k, _)| k == key) {
        if existing.1.trim() == val.trim() {
            devs.push(Deviation {
                rule: "K006",
                sev: Sev::Fixed,
                line,
                original: raw.trim().to_string(),
                action: format!("{key} 重复且相同，已去重"),
            });
        } else {
            devs.push(Deviation {
                rule: "K007",
                sev: Sev::Warn,
                line,
                original: format!("{key} 第二次出现: {}", crate::util::truncate_chars(val, 60)),
                action: format!("保留首个值: {}", crate::util::truncate_chars(&existing.1, 60)),
            });
        }
        return;
    }
    out.fields.push((key.to_string(), val.trim().to_string()));
}
