//! Record parsing/serialisation for the AgentsBook V2 experiment format.
//!
//! Layout:
//!   [CANDIDATE_MEMORY] | [CANONICAL_MEMORY]
//!   KEY=value            (may be packed as  KEY=a;KEY2=b)
//!   [/...]
//!   [MEMORY_BODY]
//!   ...
//!   [/MEMORY_BODY]
//!   [PROMOTION]  (optional, candidate only)
//!   [/PROMOTION]

use crate::sha256;
use crate::util::*;
use std::path::{Path, PathBuf};

pub const CAND_KEYS: &[&str] = &[
    "FORMAT", "ID", "PROJECT", "TASK", "SESSION", "KIND", "MATURITY", "AUTHORITY", "TITLE",
    "SUMMARY", "READ_WHEN", "NOT_FOR", "KEYS", "SIZE", "SECTIONS", "CONCEPTS", "ENTITIES",
    "PREVIOUS_ID", "SUPERSEDES", "RELATED_IDS", "EVIDENCE", "UNCERTAINTY", "NEXT_VALIDATION",
    "SOURCE", "SEQUENCE", "UPDATED",
];

pub const CANON_KEYS: &[&str] = &[
    "FORMAT", "ID", "PROJECT", "TASK", "SESSION", "KIND", "MATURITY", "AUTHORITY", "TITLE",
    "SUMMARY", "READ_WHEN", "NOT_FOR", "KEYS", "SIZE", "SECTIONS", "CONCEPTS", "ENTITIES",
    "PREVIOUS_ID", "SUPERSEDES", "RELATED_IDS", "EVIDENCE", "UNCERTAINTY", "NEXT_VALIDATION",
    "SOURCE", "STATUS", "PROMOTED_FROM", "PROMOTED_AT", "SEQUENCE", "UPDATED",
];

pub const KNOWN_KEYS: &[&str] = &[
    "FORMAT", "ID", "PROJECT", "TASK", "SESSION", "KIND", "MATURITY", "AUTHORITY", "TITLE",
    "SUMMARY", "READ_WHEN", "NOT_FOR", "KEYS", "SIZE", "SECTIONS", "CONCEPTS", "ENTITIES",
    "PREVIOUS_ID", "SUPERSEDES", "RELATED_IDS", "EVIDENCE", "UNCERTAINTY", "NEXT_VALIDATION",
    "SOURCE", "SEQUENCE", "STATUS", "PROMOTED_FROM", "PROMOTED_AT", "PROMOTED_TO", "UPDATED",
    // Found by reading the real library back (the self-test now does this every
    // run). All twelve appear as genuine `KEY=` at a field boundary, never as
    // continuation prose, so they belong here rather than being absorbed into
    // the previous value.
    "AT", "AUTHORIZED_BY", "CANDIDATE_SOURCE", "DECISION", "EFFECT", "NOTE",
    "NOTE_UNCERTAINTY", "PATH_CORRECTION", "PROMOTION_RECORD", "REVISION_ID",
    "TITLE_ALT", "VERIFIED_BY",
    // Second pass, same method, full-library scan. These 13 were found as
    // genuine `KEY=` at a field boundary across all 960 records.
    //
    // This list is no longer cosmetic. `extract_pairs` used to split on any
    // `;` that merely looked like the start of a key, which carved
    // `...git;a=shortlog;h=refs/heads/x` into three pieces: the real SUMMARY
    // was cut to 44 chars and the remainder was minted as bogus fields `A` and
    // `H` (see `C-20261010T210433-d6b85445`). Gating that split on this list is
    // what fixes it, so a name missing here now costs data rather than merely
    // producing a noisy self-test warning.
    "APPROVED_BY", "COLLISION_NOTE", "EVIDENCE2", "EVIDENCE3", "NOTE_ID_REASSIGNED",
    "PREVIOUS_REVISION", "PROMOTED_FILE", "PROMOTED_RECORD", "RELATED_CANDIDATES",
    "REVISED_AT", "REVISION_HASH", "SKILL_FEEDBACK", "VOPS",
    // 版本标记：真库里有些记录用 `V262=` 这种形式标注它属于哪一版。
    // 自检每次读真库都会重新发现它们——不认识就会被当成上一字段的续行，
    // 那一条字段值会莫名其妙变长，而头部看不出问题。按前缀认，不逐个列。
];

/// 这个大写标识是不是一个已知的记录字段。
///
/// 单独的 `V<数字>` 不在常量表里：它是版本标记，形态固定但会随版本增长，
/// 逐个列进表等于每出一版就改一次代码。按形态认，形态本身足够严——全大写
/// 字母加数字，不会与正文里的大写单词混淆（后者不会恰好是 `V` + 纯数字）。
pub fn is_known_key(k: &str) -> bool {
    if KNOWN_KEYS.contains(&k) {
        return true;
    }
    match k.strip_prefix('V') {
        Some(rest) => !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lib {
    Canonical,
    Candidate,
}

impl Lib {
    pub fn tag(self) -> &'static str {
        match self {
            Lib::Canonical => "CANONICAL_MEMORY",
            Lib::Candidate => "CANDIDATE_MEMORY",
        }
    }
    pub fn prefix(self) -> &'static str {
        match self {
            Lib::Canonical => "C",
            Lib::Candidate => "N",
        }
    }
    pub fn dir(self) -> &'static str {
        match self {
            Lib::Canonical => "canonical/records",
            Lib::Candidate => "candidate/records",
        }
    }
    pub fn atlas(self) -> &'static str {
        match self {
            Lib::Canonical => "canonical/Atlas.md",
            Lib::Candidate => "candidate/Atlas.md",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Lib::Canonical => "canonical",
            Lib::Candidate => "candidate",
        }
    }
    pub fn order(self) -> &'static [&'static str] {
        match self {
            Lib::Canonical => CANON_KEYS,
            Lib::Candidate => CAND_KEYS,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Record {
    pub path: PathBuf,
    pub lib: Lib,
    pub fields: Vec<(String, String)>,
    pub body: String,
    pub promotion: Option<String>,
    /// 附件清单块（`[ATTACH]`）的内容，一份行式清单：`名字<TAB>字节`。
    ///
    /// 这份清单是**记录的一部分**，不是缓存：它随记录一起被写、被搬、被读，
    /// 因此记录自己就能说清"我带了哪些附件"。真正的权威仍是磁盘上的目录
    /// （见 `attach::verify`），清单是索引与人类可读的目录页。
    pub attach: Option<String>,
    pub raw: String,
    /// Every deviation the tolerant parser had to absorb for this file.
    /// Empty means the file was in canonical form.
    pub devs: Vec<crate::parse::Deviation>,
    /// Set when the file had no header tag and the header was inferred.
    pub header_inferred: bool,
    /// Exactly what `raw` parsed into, so a write can splice only the fields
    /// that really changed instead of reformatting the file.
    pub base: Option<Orig>,
    /// The file's last-modified time, in Unix seconds; 0 when it could not be
    /// read (a record built from text that was never on disk).
    ///
    /// This is what makes "show me the newest first" mean *touched* rather than
    /// *created*: editing a record from last month should float it to the top,
    /// because having touched it means it was the most recent thing on your
    /// mind. A record's own declared time cannot express that.
    pub mtime: i64,
}

/// The parsed value of every field as it was read from disk.
///
/// Comparing against this is what makes a write minimal: a field that is set to
/// the value it already had produces no edit at all, so the file's bytes do not
/// move. Without it, `set TITLE --value <same title>` still rewrote the entire
/// header and, because this library packs several fields per line, expanded one
/// line into four.
#[derive(Clone, Debug)]
pub struct Orig {
    pub fields: Vec<(String, String)>,
    pub body: String,
    pub promo: Option<String>,
    pub attach: Option<String>,
}

/// Collapse every run of whitespace (including newlines) into a single space so
/// a value fits on one line. Leading and trailing whitespace is dropped.
pub fn fold_to_one_line(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut pending_space = false;
    for ch in v.trim().chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' || ch == ' ' {
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(ch);
        }
    }
    out
}

impl Record {
    pub fn get(&self, k: &str) -> String {
        if let Some((_, v)) = self.fields.iter().find(|(kk, _)| kk == k) {
            return v.clone();
        }
        self.promo_fields()
            .into_iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| v)
            .unwrap_or_default()
    }
    /// True if the key is readable by `get` -- a header field or a key inside a
    /// tail block.
    ///
    /// Tail blocks used to be invisible to `get`: the promotion marker written
    /// by `promote` lives in `[PROMOTION]` after the body, so `get <rec>
    /// PROMOTED_TO` answered "no such field" for a field that was plainly on
    /// disk in the same file. That is a trap, not a distinction -- a caller
    /// asking for a field should not have to know which block holds it.
    pub fn has(&self, k: &str) -> bool {
        self.fields.iter().any(|(kk, _)| kk == k)
            || self.promo_fields().into_iter().any(|(kk, _)| kk == k)
    }
    /// The `[PROMOTION]` block parsed into key/value pairs.
    pub fn promo_fields(&self) -> Vec<(String, String)> {
        let Some(p) = &self.promotion else {
            return Vec::new();
        };
        p.lines()
            .filter_map(|line| {
                let (k, v) = line.split_once('=')?;
                Some((k.trim().to_string(), v.trim().to_string()))
            })
            .collect()
    }
    /// Set a header field. Field values occupy exactly one line: a newline
    /// inside a value silently breaks every reader that walks the file line by
    /// line, including this tool. A multi-line body copied into SUMMARY did
    /// exactly that, so newlines are folded into single spaces here -- the one
    /// place every writer goes through.
    pub fn set(&mut self, k: &str, v: &str) {
        let v = fold_to_one_line(v);
        if let Some(e) = self.fields.iter_mut().find(|(kk, _)| kk == k) {
            e.1 = v;
        } else {
            self.fields.push((k.to_string(), v));
        }
    }
    pub fn id(&self) -> String {
        let v = self.get("ID");
        let first = if v.contains(';') {
            v.split(';').next().unwrap_or("").trim().to_string()
        } else {
            v.trim().to_string()
        };
        // 身份必须可解析，否则一切引用都会悬空。
        //
        // `ID` 是头部字段，能被写坏（曾经 `update` 就把 ID 改成了
        // `N-<旧ID>-<新哈希>`，套两层前缀）。而文件名是磁盘上的规范位置，
        // 写不坏。所以当 ID 字段不成形、文件名却是规范形态时，以文件名为准。
        //
        // 这一层兜底会让 `PROMOTED_FROM`/`RELATED_IDS` 这类由 `id()` 写出的
        // 引用天然可解析，不必依赖每个调用点各自小心。
        if !Self::is_scheme_name(&format!("{first}.md")) {
            if let Some(stem) = self.path.file_stem() {
                let stem = stem.to_string_lossy().to_string();
                if Self::is_scheme_name(&format!("{stem}.md")) {
                    return stem;
                }
            }
        }
        first
    }
    pub fn title(&self) -> String {
        self.get("TITLE")
    }
    pub fn kind(&self) -> String {
        self.get("KIND")
    }
    pub fn status(&self) -> String {
        let s = self.get("STATUS");
        if s.is_empty() {
            "ACTIVE".into()
        } else {
            s
        }
    }
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// Parse a record file through the tolerant parser.
    ///
    /// This never hard-matches: aliases, missing tags, full-width punctuation,
    /// packed lines and duplicate keys are all absorbed and recorded.
    pub fn parse(path: &Path, lib: Lib) -> R<Record> {
        let raw = read_str(path)?;
        Record::parse_str(&raw, path, lib)
    }

    /// Parse already-loaded text (used by tests and by `selftest`).
    pub fn parse_str(raw: &str, path: &Path, lib: Lib) -> R<Record> {
        let filename = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let parsed = crate::parse::parse(raw, Some(lib));
        if parsed.has_error() {
            let msgs: Vec<String> = parsed
                .errors()
                .iter()
                .map(|d| d.show())
                .collect();
            return integrity(format!(
                "无法解析 {}: {}",
                if filename.is_empty() { "<文本>" } else { &filename },
                msgs.join(" | ")
            ));
        }
        // The tag spelling wins when it disagrees with the caller's assumption.
        let lib = parsed.lib_hint.unwrap_or(lib);
        let header_inferred = !parsed.had_header_tag;
        Ok(Record {
            path: path.to_path_buf(),
            lib,
            fields: parsed.fields.clone(),
            body: parsed.body.clone(),
            promotion: parsed.promotion.clone(),
            attach: parsed.attach.clone(),
            raw: raw.to_string(),
            devs: parsed.devs.clone(),
            header_inferred,
            base: Some(Orig {
                fields: parsed.fields.clone(),
                body: parsed.body.clone(),
                promo: parsed.promotion.clone(),
                attach: parsed.attach.clone(),
            }),
            // Read the real file. `parse_str` is also reached from tests with a
            // path that has no file behind it, so a failure here is normal and
            // just means "no mtime available" -- never an error.
            mtime: std::fs::metadata(path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        })
    }

    /// The time this record claims for itself: a declared field first, then a
    /// timestamp leading the filename, else 0.
    ///
    /// Deliberately does *not* fall back to mtime, and does not read `UPDATED`
    /// either: conflating created with touched would make every record that was
    /// ever edited claim it was written on the day it was last edited, which
    /// destroys the distinction a reader uses to tell what is new from what was
    /// merely revisited.
    /// Falls back to mtime when the record carries no time of its own.
    ///
    /// Only for display and for legacy records: the migration-era files
    /// (`AB2-C-001.md` and friends) have neither a `TIME=` field nor a
    /// timestamped name, so without this they would show no date at all. mtime
    /// is an approximation, but "probably around then" beats a blank when a
    /// reader is trying to place a record in time. Anything the tool writes
    /// from now on carries a real stamp and never reaches this path.
    pub fn created(&self) -> i64 {
        let t = crate::util::record_time(&self.path, &self.raw);
        if t > 0 { t } else { self.mtime }
    }

    /// When this record was last touched.
    ///
    /// Reads the `UPDATED` field the write path maintains, so the answer
    /// survives copying the library, restoring a backup, or unpacking an
    /// archive -- all of which reset every file's mtime at once and would
    /// otherwise flatten a recency-ordered listing into noise.
    ///
    /// Falls back to the pre-`UPDATED` three-source order for records written
    /// before the field existed. That fallback is what makes this a
    /// non-migration: old records keep sorting exactly as they did, and each
    /// one picks up the field the first time it is edited.
    pub fn updated(&self) -> i64 {
        crate::util::record_updated(&self.path, &self.raw, &self.get("UPDATED"))
    }

    /// Bytes to write for this record.
    ///
    /// Used by every in-place edit. It splices the original file rather than
    /// re-rendering it, so a change to one field leaves the rest of the file
    /// byte-identical -- including the packed `A=x;B=y` lines this library
    /// uses. A record that was never read from disk (a new record, or a merge
    /// result) has no `base` and falls back to [`Record::render`].
    pub fn render_for_write(&self) -> String {
        if let Some(b) = &self.base {
            if let Some(s) = crate::splice::write_back(
                &self.raw,
                crate::splice::Base {
                    fields: &b.fields,
                    body: &b.body,
                    promo: b.promo.as_deref(),
                    attach: b.attach.as_deref(),
                },
                self.lib,
                &self.fields,
                &self.body,
                self.promotion.as_deref(),
                self.attach.as_deref(),
            ) {
                return s;
            }
        }
        self.render()
    }

    /// Remember the current state as the new on-disk baseline.
    ///
    /// Called right after a successful write so a second edit in the same run
    /// splices from the text that is actually on disk now.
    pub fn rebase(&mut self, written: &str) {
        self.raw = written.to_string();
        self.base = Some(Orig {
            fields: self.fields.clone(),
            body: self.body.clone(),
            promo: self.promotion.clone(),
            attach: self.attach.clone(),
        });
    }

    /// Serialise back to the on-disk format.
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("[{}]\n", self.lib.tag()));
        // emit in canonical key order, then any unknown keys in stored order
        let order = self.lib.order();
        for k in order {
            if let Some((_, v)) = self.fields.iter().find(|(kk, _)| kk == k) {
                if v.is_empty() && !matches!(*k, "PREVIOUS_ID" | "SUPERSEDES") {
                    // keep empty placeholders only for relations that the
                    // format always shows
                    if !matches!(*k, "PROJECT" | "TASK" | "SESSION") {
                        s.push_str(&format!("{k}={v}\n"));
                        continue;
                    }
                }
                s.push_str(&format!("{k}={v}\n"));
            }
        }
        for (k, v) in &self.fields {
            if !order.contains(&k.as_str()) {
                s.push_str(&format!("{k}={v}\n"));
            }
        }
        s.push_str(&format!("[/{}]\n", self.lib.tag()));
        s.push_str("[MEMORY_BODY]\n");
        s.push_str(&self.body);
        if !self.body.ends_with('\n') {
            s.push('\n');
        }
        s.push_str("[/MEMORY_BODY]\n");
        if let Some(p) = &self.promotion {
            s.push_str("[PROMOTION]\n");
            s.push_str(p.trim_matches('\n'));
            s.push('\n');
            s.push_str("[/PROMOTION]\n");
        }
        // 附件清单排在最后：它是"我带了什么"的目录页，位置固定在文件末尾，
        // 读的人往下翻到底就能看到全貌，不必先去列目录。
        if let Some(a) = &self.attach {
            s.push_str("[ATTACH]\n");
            s.push_str(a.trim_matches('\n'));
            s.push('\n');
            s.push_str("[/ATTACH]\n");
        }
        s
    }

    /// Filename stem from `RECORD_ID_SCHEME`: <prefix>-<stamp>-<hash8>.
    ///
    /// Kept for legacy callers that only have a body string. New code should
    /// use [`Record::id_for`], which hashes the whole record.
    pub fn new_id(lib: Lib, stamp: &str, body: &str) -> String {
        let h = sha256::digest_prefix(body.trim().as_bytes(), 8);
        format!("{}-{}-{}", lib.prefix(), stamp, h)
    }

    /// 8 hex digits identifying this record's content.
    ///
    /// Hashes every field except `ID` plus the body. Hashing the body alone
    /// collided for records that share a body but differ in title, and a
    /// collision here is not detected anywhere downstream -- the second write
    /// simply overwrites the first file and two records become one. Including
    /// the fields makes the name follow the record, not just its prose.
    pub fn content_hash(&self) -> String {
        let mut s = String::new();
        for (k, v) in &self.fields {
            if k == "ID" {
                continue;
            }
            s.push_str(k);
            s.push('=');
            s.push_str(v);
            s.push('\n');
        }
        s.push('\u{0}');
        s.push_str(self.body.trim());
        sha256::digest_prefix(s.as_bytes(), 8)
    }

    /// Filename stem derived from this record's own content.
    pub fn id_for(&self, stamp: &str) -> String {
        format!("{}-{}-{}", self.lib.prefix(), stamp, self.content_hash())
    }

    /// 从任意字符串里找出第一个合法的身份形态，返回它本身。
    ///
    /// 用途是**修复被写坏的引用**。坏值有几种已知形态，而它们共同点是：
    /// 真正的身份作为子串仍然完整地留在里面。
    ///
    ///   `N-N-20261024T124521-1a2c1f34-8aa8f744`   ← 双前缀 + 尾部多余哈希
    ///   `N-20261024T124521-1a2c1f34-8aa8f744`      ← 尾部多余哈希
    ///   `N-20261024T124521-1a2c1f34`               ← 正常
    ///
    /// 三者都含 `N-20261024T124521-1a2c1f34` 这个子串。所以不去猜"该剥几层"，
    /// 而是找出最早出现的合法身份——合法形态恰好是定长的三段结构，扫描判定
    /// 不存在歧义，也不需要看到其他记录。
    pub fn canonical_ref(s: &str) -> Option<String> {
        if Self::is_scheme_name(&format!("{s}.md")) {
            return Some(s.to_string());
        }
        let b = s.as_bytes();
        let n = b.len();
        // 最短合法身份：前缀(1) + '-' + 时间(15) + '-' + 哈希(8) = 26
        if n < 26 {
            return None;
        }
        let stamp_ok = |i: usize| -> bool {
            // ddddddddTdddddd
            (0..15).all(|k| {
                let c = b[i + k];
                if k == 8 {
                    c == b'T'
                } else {
                    c.is_ascii_digit()
                }
            })
        };
        for i in 0..=n.saturating_sub(26) {
            if b[i] != b'N' && b[i] != b'C' {
                continue;
            }
            if b[i + 1] != b'-' || !stamp_ok(i + 2) || b[i + 17] != b'-' {
                continue;
            }
            if !(0..8).all(|k| b[i + 18 + k].is_ascii_hexdigit()) {
                continue;
            }
            // 哈希 8 位之后不能再紧跟十六进制字符，否则我们只截到了一半
            if let Some(next) = b.get(i + 26) {
                if next.is_ascii_hexdigit() {
                    continue;
                }
            }
            return Some(s[i..i + 26].to_string());
        }
        None
    }

    pub fn is_scheme_name(name: &str) -> bool {
        let stem = match name.strip_suffix(".md") {
            Some(s) => s,
            None => return false,
        };
        let parts: Vec<&str> = stem.split('-').collect();
        if parts.len() != 3 {
            return false;
        }
        let ok_prefix = parts[0] == "C" || parts[0] == "N";
        let st = parts[1];
        let ok_stamp = st.len() == 15
            && st.as_bytes()[8] == b'T'
            && st[..8].bytes().all(|b| b.is_ascii_digit())
            && st[9..].bytes().all(|b| b.is_ascii_digit());
        let ok_hash = parts[2].len() == 8 && parts[2].bytes().all(|b| b.is_ascii_hexdigit());
        ok_prefix && ok_stamp && ok_hash
    }

    /// Header completeness problems for `lint`.
    /// Reconstruct a `Parsed` view so the shared validator can be reused.
    /// `lint` must not keep its own rule list: doing so previously made it
    /// disagree with `audit` (it demanded KEYS, which is explicitly derivable
    /// and therefore only a warning).
    pub fn as_parsed(&self) -> crate::parse::Parsed {
        crate::parse::Parsed {
            lib_hint: Some(self.lib),
            fields: self.fields.clone(),
            body: self.body.clone(),
            promotion: self.promotion.clone(),
            attach: None,
            devs: self.devs.clone(),
            had_header_tag: !self.header_inferred,
            body_inferred: false,
        }
    }

    /// Thin wrapper over the shared validator, so `lint` and `audit` can never
    /// report different rule sets for the same record.
    pub fn lint(&self) -> Vec<String> {
        let rep = crate::schema::validate(&self.as_parsed(), self.lib, Some(&self.file_name()));
        let mut out: Vec<String> = rep
            .issues
            .iter()
            .map(|i| format!("[{}] {} {} :: {}", i.sev.label(), i.rule, i.field, i.message))
            .collect();
        // Deviations are part of the record's health too. Omitting them made
        // `lint` report fewer bad records than `audit` for the same library.
        for d in &self.devs {
            out.push(format!("[{}] {} 行{} {} → {}", d.sev.label(), d.rule, d.line, d.original, d.action));
        }
        out
    }
}

/// How much of a record the fallback selector match is allowed to read.
///
/// Every key the fallback looks at (`ID`, `TITLE`) sits at the top of the
/// record, so this is not a guess that might clip real data: the format puts
/// those keys there.
///
/// Measured over the real library (962 records: 523 canonical + 439 candidate):
/// the first six lines -- exactly the slice the ID match below uses -- end by
/// byte 359, and the `TITLE=` line ends by byte 1662. 4 KiB therefore leaves a
/// 2.4x margin on the widest header actually present. The one `TITLE=`
/// occurrence past byte 4096 in the whole library sits in a record's *body*
/// prose (`KEYS=/TITLE=/SECTIONS=`), and a duplicate could not win anyway: the
/// first field wins, and that one is inside the header.
///
/// Reading the whole file instead bought nothing. A selector that matches
/// nothing used to read *and fully parse* all 962 records (84.5 ms median) only
/// to report that nothing matched, and the title loop used to run a full parse
/// per record -- body included -- to compare one header string.
const SELECTOR_HEAD_BYTES: usize = 4096;

/// The `TITLE` the fallback title match should see, read from the head only.
///
/// `None` means "not a usable record" and stands in for the `Err` arm of
/// `Record::parse`, so the two have to agree on *which* records are skipped, not
/// just on the title text. `Record::parse` fails for exactly two reasons, both
/// `Sev::Error` (verified: the only two `Sev::Error` sites in `src/parse.rs` are
/// the two sanity rules):
///
/// - `R001` no recognizable field. A header with no field has no body either,
///   and every record in the library has a `KEY=` line inside its first 4 KiB
///   (checked: 962/962), so the head decides this identically.
/// - `R002` no body *and* no title. Reading only the head makes a long body look
///   empty, so a record with no `TITLE` can look unparseable here where a full
///   parse accepted it. That record's title is empty either way, and an empty
///   title can neither equal nor be a prefix of a non-empty selector (`sel` is
///   never empty in this loop: `dir.join("")` is the records directory and was
///   resolved earlier), so both paths still skip it.
///
/// One more difference from `Record::get`: that falls back to a `TITLE` inside
/// the `[PROMOTION]` tail block, which by definition is past the head;
/// `Parsed::get` here does not. Measured over the real library: 0 records carry
/// `TITLE=` inside a promotion block and 0 lack a header `TITLE`, so that
/// fallback has never been reachable -- and `promote` writes only PROMOTED_TO /
/// PROMOTED_AT / BRANCH / NOTE into the block, so this tool cannot make it
/// reachable either.
fn title_from_head(path: &Path, lib: Lib) -> Option<String> {
    let head = read_head(path, SELECTOR_HEAD_BYTES).ok()?;
    let parsed = crate::parse::parse(&head, Some(lib));
    if parsed.has_error() {
        return None;
    }
    Some(parsed.get("TITLE"))
}

/// Resolve a user-supplied record selector to a path.
/// Accepts: full path, library-relative path, filename, record ID.
pub fn resolve_record(root: &Path, sel: &str) -> R<(PathBuf, Lib)> {
    let p = PathBuf::from(sel);
    if p.is_absolute() && p.exists() {
        let lib = if p.to_string_lossy().contains("candidate") {
            Lib::Candidate
        } else {
            Lib::Canonical
        };
        return Ok((p, lib));
    }
    let cand = root.join(sel);
    if cand.exists() {
        let lib = if sel.contains("candidate") {
            Lib::Candidate
        } else {
            Lib::Canonical
        };
        return Ok((cand, lib));
    }
    for lib in [Lib::Canonical, Lib::Candidate] {
        let dir = root.join(lib.dir());
        let by_name = dir.join(sel);
        if by_name.exists() {
            return Ok((by_name, lib));
        }
        let with_md = dir.join(format!("{sel}.md"));
        if with_md.exists() {
            return Ok((with_md, lib));
        }
        // ID match: filename stem contains the id, or header ID matches
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let pth = e.path();
                if !pth.extension().map(|x| x == "md").unwrap_or(false) {
                    continue;
                }
                let stem = pth
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if stem == sel || stem.ends_with(&format!("-{sel}")) {
                    return Ok((pth, lib));
                }
                // Head only, same six lines the full read was sliced to. The
                // `take(6)` stays: it is what keeps a `=x;` that merely
                // appears in the body from being read as a header match.
                if let Ok(txt) = read_head(&pth, SELECTOR_HEAD_BYTES) {
                    let head: String = txt.lines().take(6).collect::<Vec<_>>().join("\n");
                    if head.contains(&format!("ID={sel}"))
                        || head.contains(&format!(";{sel};"))
                        || head.contains(&format!("={sel};"))
                    {
                        return Ok((pth, lib));
                    }
                }
            }
        }
    }
    // Title match: users and models name a record by its heading far more
    // often than by filename. Exact title first, then unique prefix.
    //
    // This loop is why a title selector used to cost a full library scan: it
    // read every record in full and ran the whole parser over it, body and
    // all, to compare one header string. It now reads the head (see
    // `title_from_head`) and keeps the same three-way outcome — exact beats
    // prefix, a unique hit wins, multiple hits are an ambiguity error listing
    // *both* candidate sets — because that order is what callers already saw.
    for lib in [Lib::Candidate, Lib::Canonical] {
        let dir = root.join(lib.dir());
        let mut exact: Vec<PathBuf> = Vec::new();
        let mut prefix: Vec<PathBuf> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let pth = e.path();
                if !pth.extension().map(|x| x == "md").unwrap_or(false) {
                    continue;
                }
                let title = match title_from_head(&pth, lib) {
                    Some(t) => t,
                    None => continue,
                };
                if title == sel {
                    exact.push(pth.clone());
                } else if !title.is_empty() && title.starts_with(sel) {
                    prefix.push(pth.clone());
                }
            }
        }
        if exact.len() == 1 {
            return Ok((exact.pop().unwrap(), lib));
        }
        if exact.is_empty() && prefix.len() == 1 {
            return Ok((prefix.pop().unwrap(), lib));
        }
        if exact.len() > 1 || prefix.len() > 1 {
            let names: Vec<String> = exact
                .iter()
                .chain(prefix.iter())
                .filter_map(|p| p.file_name().map(|x| x.to_string_lossy().to_string()))
                .collect();
            return Err(EE::Usage(format!(
                "标题「{sel}」匹配到多条，请用文件名或 ID 指定：{}",
                names.join(", ")
            )));
        }
    }
    notfound(Fail::new(format!("未找到记录: {sel}")).hint(
        "三种可能：名字写错、该记录已被删除、或它属于另一个库。\
         用 `abook search <关键词>` 或 `abook list --q <词>` 找出真实名字；\
         删掉的记录用 `abook list --status DELETED` 能看到",
    ).render())
}

/// Load every record of a library, sorted by filename.
pub fn load_lib(root: &Path, lib: Lib) -> Vec<Record> {
    let dir = root.join(lib.dir());
    let mut v = Vec::new();
    for p in list_md(&dir) {
        if let Ok(r) = Record::parse(&p, lib) {
            v.push(r);
        }
    }
    v
}

/// Field-level listing filter used by `list`.
#[derive(Clone, Debug, Default)]
pub struct ListFilter {
    pub kind: String,
    pub maturity: String,
    pub status: String,
    pub project: String,
    pub lib: String,
    pub has_dev: bool,
}

impl ListFilter {
    pub fn from_args(a: &crate::util::Args) -> ListFilter {
        ListFilter {
            kind: a.get("kind").unwrap_or("").to_uppercase(),
            maturity: a.get("maturity").unwrap_or("").to_uppercase(),
            status: a.get("status").unwrap_or("").to_uppercase(),
            project: a.get("project").unwrap_or("").to_string(),
            lib: a.get("lib").unwrap_or("").to_string(),
            has_dev: a.has("deviations"),
        }
    }

    pub fn accept(&self, rec: &Record, a: &crate::util::Args) -> bool {
        if !self.lib.is_empty() {
            let want = self.lib.starts_with('c') && !self.lib.starts_with("cand");
            if want != (rec.lib == Lib::Canonical) {
                return false;
            }
        }
        if !self.kind.is_empty() && !rec.kind().eq_ignore_ascii_case(&self.kind) {
            return false;
        }
        if !self.maturity.is_empty() && !rec.get("MATURITY").eq_ignore_ascii_case(&self.maturity) {
            return false;
        }
        if !self.status.is_empty() {
            let st = rec.status();
            if !st.eq_ignore_ascii_case(&self.status) {
                return false;
            }
        } else if rec.status() == "DELETED" && !a.has("include-deleted") {
            return false;
        }
        if !self.project.is_empty() && !rec.get("PROJECT").eq_ignore_ascii_case(&self.project) {
            return false;
        }
        if self.has_dev && rec.devs.is_empty() {
            return false;
        }
        if let Some(k) = a.get("title") {
            if !rec.title().to_lowercase().contains(&k.to_lowercase()) {
                return false;
            }
        }
        // `--q` was accepted but never applied: `list --q GATE` returned the
        // whole library, which is the same class of bug as ignoring a flag.
        // Match any field value or the body, case-insensitively.
        if let Some(q) = a.get("q") {
            let needle = q.to_lowercase();
            if !needle.is_empty() {
                let hit_field = rec
                    .fields
                    .iter()
                    .any(|(_, v)| v.to_lowercase().contains(&needle));
                let hit_body = rec.body.to_lowercase().contains(&needle);
                if !hit_field && !hit_body {
                    return false;
                }
            }
        }
        true
    }
}

/// Every directory a record may live in, in a stable order.
///
/// Callers that sweep for files (residue, migration, indexing) previously each
/// spelled out `canonical/records` and `candidate/records` by hand; a third
/// library would silently go unswept. One list, one place to extend.
pub fn all_record_dirs(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    vec![Lib::Canonical.dir(), Lib::Candidate.dir()]
        .into_iter()
        .map(|d| root.join(d))
        .collect()
}
