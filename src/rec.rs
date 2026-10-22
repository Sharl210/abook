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
    "SOURCE", "SEQUENCE",
];

pub const CANON_KEYS: &[&str] = &[
    "FORMAT", "ID", "PROJECT", "TASK", "SESSION", "KIND", "MATURITY", "AUTHORITY", "TITLE",
    "SUMMARY", "READ_WHEN", "NOT_FOR", "KEYS", "SIZE", "SECTIONS", "CONCEPTS", "ENTITIES",
    "PREVIOUS_ID", "SUPERSEDES", "RELATED_IDS", "EVIDENCE", "UNCERTAINTY", "NEXT_VALIDATION",
    "SOURCE", "STATUS", "PROMOTED_FROM", "PROMOTED_AT", "SEQUENCE",
];

pub const KNOWN_KEYS: &[&str] = &[
    "FORMAT", "ID", "PROJECT", "TASK", "SESSION", "KIND", "MATURITY", "AUTHORITY", "TITLE",
    "SUMMARY", "READ_WHEN", "NOT_FOR", "KEYS", "SIZE", "SECTIONS", "CONCEPTS", "ENTITIES",
    "PREVIOUS_ID", "SUPERSEDES", "RELATED_IDS", "EVIDENCE", "UNCERTAINTY", "NEXT_VALIDATION",
    "SOURCE", "SEQUENCE", "STATUS", "PROMOTED_FROM", "PROMOTED_AT", "PROMOTED_TO",
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
    pub raw: String,
    /// Every deviation the tolerant parser had to absorb for this file.
    /// Empty means the file was in canonical form.
    pub devs: Vec<crate::parse::Deviation>,
    /// Set when the file had no header tag and the header was inferred.
    pub header_inferred: bool,
}

/// Parse `KEY=value` pairs from one header line, tolerating `;` inside values
/// and several pairs packed on the same line.
pub fn parse_kv_line(line: &str, out: &mut Vec<(String, String)>) {
    let bytes: Vec<char> = line.chars().collect();
    let n = bytes.len();
    // find candidate start positions: [A-Z][A-Z0-9_]* '='
    let mut marks: Vec<(usize, usize, String)> = Vec::new(); // (start, eq_pos, key)
    let mut i = 0;
    while i < n {
        let at_boundary = i == 0 || bytes[i - 1] == ';';
        if at_boundary && bytes[i].is_ascii_uppercase() {
            let start = i;
            let mut j = i;
            while j < n && (bytes[j].is_ascii_uppercase() || bytes[j].is_ascii_digit() || bytes[j] == '_')
            {
                j += 1;
            }
            if j < n && bytes[j] == '=' {
                let key: String = bytes[start..j].iter().collect();
                if KNOWN_KEYS.contains(&key.as_str()) {
                    marks.push((start, j, key));
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    if marks.is_empty() {
        // continuation of the previous value
        if let Some(last) = out.last_mut() {
            if !last.1.is_empty() {
                last.1.push('\n');
            }
            last.1.push_str(line.trim_end());
        } else {
            out.push(("_UNPARSED".into(), line.trim_end().to_string()));
        }
        return;
    }
    for (idx, (start, eq, key)) in marks.iter().enumerate() {
        let end = if idx + 1 < marks.len() {
            marks[idx + 1].0
        } else {
            n
        };
        let mut val: String = bytes[eq + 1..end].iter().collect();
        // strip the separator that belonged to the next pair
        if idx + 1 < marks.len() {
            if val.ends_with(';') {
                val.pop();
            }
        }
        let _ = start;
        out.push((key.clone(), val.trim_end().to_string()));
    }
}

impl Record {
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
    pub fn set(&mut self, k: &str, v: &str) {
        if let Some(e) = self.fields.iter_mut().find(|(kk, _)| kk == k) {
            e.1 = v.to_string();
        } else {
            self.fields.push((k.to_string(), v.to_string()));
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
            raw: raw.to_string(),
            devs: parsed.devs.clone(),
            header_inferred,
        })
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
        s
    }

    /// Filename stem from `RECORD_ID_SCHEME`: <prefix>-<stamp>-<hash8 of body>.
    pub fn new_id(lib: Lib, stamp: &str, body: &str) -> String {
        let h = sha256::digest_prefix(body.trim().as_bytes(), 8);
        format!("{}-{}-{}", lib.prefix(), stamp, h)
    }

    /// Legacy sequential names look like `AB2-N-001.md`.
    pub fn is_legacy_name(name: &str) -> bool {
        let stem = name.trim_end_matches(".md");
        stem.starts_with("AB2-") || (stem.len() <= 12 && !stem.contains("-2"))
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
    notfound(format!("未找到记录: {sel}"))
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
        true
    }
}
