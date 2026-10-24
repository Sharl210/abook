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
];

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
        if v.contains(';') {
            v.split(';').next().unwrap_or("").trim().to_string()
        } else {
            v
        }
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
                if let Ok(txt) = std::fs::read_to_string(&pth) {
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
                if let Ok(rec) = Record::parse(&pth, lib) {
                    let t = rec.title();
                    if t == sel {
                        exact.push(pth.clone());
                    } else if !t.is_empty() && t.starts_with(sel) {
                        prefix.push(pth.clone());
                    }
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
