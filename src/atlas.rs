//! Atlas (branch map) parsing and refreshing.
//!
//! One `## Section` may contain several branch cards; the section title is the
//! grouping label and each card carries its own BRANCH / KEYS / COUNT /
//! MAXSIZE / SUMMARY / READ_WHEN / PATH / REVISION / STATUS. A section's first
//! card emits the heading; later cards in the same section do not.
//!
//! PATH lists the first entry with its directory and the rest as bare
//! basenames that inherit that directory.

use crate::util::*;
use std::path::Path;

/// Field names that may appear inside an Atlas branch card.
const CARD_KEYS: &[&str] = &[
    "BRANCH", "KEYS", "COUNT", "MAXSIZE", "SUMMARY", "READ_WHEN", "PATH", "REVISION",
    "STATUS", "PROMOTED_TO",
    // Hierarchy. Both are optional, and a card without them is a top-level
    // branch -- which is what every card in this library is today, so the
    // feature arrives without touching existing files.
    //
    // PARENT names another BRANCH (not a heading): headings group cards for
    // display, they are not a tree, and reusing a heading as a parent would
    // make two unrelated mechanisms look like one.
    "PARENT", "LEVEL",
];

#[derive(Clone, Debug, Default)]
pub struct Branch {
    pub heading: String,
    pub emit_heading: bool,
    pub fields: Vec<(String, String)>,
    /// This card's original bytes, from its first line up to the next card.
    ///
    /// Atlas.md is a user file that predates this tool, so it has to survive a
    /// write unchanged apart from the line being edited. Re-rendering from
    /// parsed fields lost data every round trip -- blank lines multiplied,
    /// a bare trailing `PATH=` marker became a blank line, the title line was
    /// re-appended to the header on every write. Keeping the original bytes and
    /// editing them in place means an untouched card is byte-identical, and a
    /// touched card changes exactly one line.
    pub raw: String,
}

/// Replace the `KEY=...` line for `k` inside `raw`, or append one if absent.
fn set_line(raw: &mut String, k: &str, v: &str) {
    let newline = format!("{k}={v}");
    let mut out = String::with_capacity(raw.len() + v.len() + 1);
    let mut done = false;
    for line in raw.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let hit = !done && body.starts_with(k) && body.as_bytes().get(k.len()) == Some(&b'=');
        if hit {
            out.push_str(&newline);
            if line.ends_with('\n') {
                out.push('\n');
            }
            done = true;
        } else {
            out.push_str(line);
        }
    }
    if !done {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&newline);
        out.push('\n');
    }
    *raw = out;
}

impl Branch {
    pub fn get(&self, k: &str) -> String {
        self.fields
            .iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
    pub fn set(&mut self, k: &str, v: &str) {
        if !self.raw.is_empty() {
            if self.get(k) != v {
                set_line(&mut self.raw, k, v);
            }
        }
        if let Some(e) = self.fields.iter_mut().find(|(kk, _)| kk == k) {
            e.1 = v.to_string();
        } else {
            self.fields.push((k.to_string(), v.to_string()));
        }
    }
    pub fn name(&self) -> String {
        let b = self.get("BRANCH");
        if b.is_empty() {
            self.heading.clone()
        } else {
            b
        }
    }

    /// Raw PATH entries with directory inheritance applied. Duplicates kept.
    pub fn paths(&self) -> Vec<String> {
        let raw = self.get("PATH");
        let mut out = Vec::new();
        let mut dir: Option<String> = None;
        for seg in raw.split([',', '，']) {
            let s = seg.trim();
            if s.is_empty() {
                continue;
            }
            if let Some(idx) = s.rfind('/') {
                if dir.is_none() {
                    dir = Some(s[..idx].to_string());
                }
                out.push(s.to_string());
            } else if let Some(d) = &dir {
                out.push(format!("{d}/{s}"));
            } else {
                out.push(s.to_string());
            }
        }
        out
    }

    /// The `PATH=` elements exactly as written, with no directory inheritance
    /// applied and no rewriting. `paths()` is for queries; this is for edits
    /// that must leave untouched elements byte-identical. Mixing the two forms
    /// inside one card is normal (185 full paths / 136 bare names), so a
    /// round trip through `paths()` + `set_paths()` silently re-spells them.
    pub fn raw_paths(&self) -> Vec<String> {
        self.get("PATH")
            .split([',', '，'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Replace `PATH=` with these elements verbatim. No dedupe, no directory
    /// abbreviation: what you pass is what gets written.
    pub fn set_paths_raw(&mut self, elems: &[String]) {
        self.set("PATH", &elems.join(","));
    }

    pub fn set_paths(&mut self, paths: &[String]) {
        let mut dir: Option<String> = None;
        let mut out: Vec<String> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for p in paths {
            let base = p.rsplit('/').next().unwrap_or(p).to_string();
            if seen.contains(&base) {
                continue;
            }
            seen.push(base);
            match p.rfind('/') {
                Some(idx) => {
                    let d = p[..idx].to_string();
                    if dir.is_none() {
                        dir = Some(d);
                        out.push(p.clone());
                    } else if dir.as_deref() == Some(d.as_str()) {
                        out.push(p[idx + 1..].to_string());
                    } else {
                        out.push(p.clone());
                    }
                }
                None => out.push(p.clone()),
            }
        }
        self.set("PATH", &out.join(","));
    }

    /// This branch's parent branch name, or empty for a top-level branch.
    pub fn parent(&self) -> String {
        self.get("PARENT")
    }

    /// Declared depth, or 0 when the card does not say.
    ///
    /// Advisory only: `tree` computes real depth by walking PARENT, because a
    /// hand-written LEVEL can disagree with the chain above it and the chain is
    /// the thing that actually decides where a card appears.
    pub fn level(&self) -> usize {
        self.get("LEVEL").trim().parse().unwrap_or(0)
    }

    pub fn count(&self) -> usize {
        let mut seen: Vec<String> = Vec::new();
        for p in self.paths() {
            let b = p.rsplit('/').next().unwrap_or(&p).to_string();
            if !seen.contains(&b) {
                seen.push(b);
            }
        }
        seen.len()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Atlas {
    pub title: String,
    pub header: Vec<(String, String)>,
    pub branches: Vec<Branch>,
    pub preamble: Vec<String>,
    /// Everything before the first card, byte-exact (see `Branch::raw`).
    pub head_raw: String,
}

impl Atlas {
    pub fn get(&self, k: &str) -> String {
        self.header
            .iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
    pub fn set(&mut self, k: &str, v: &str) {
        if !self.head_raw.is_empty() && self.get(k) != v {
            set_line(&mut self.head_raw, k, v);
        }
        if let Some(e) = self.header.iter_mut().find(|(kk, _)| kk == k) {
            e.1 = v.to_string();
        } else {
            self.header.push((k.to_string(), v.to_string()));
        }
    }

    /// Parse from a string (used by the self test, which must not touch disk).
    pub fn load_str(raw: &str, path: &str) -> R<Atlas> {
        Atlas::parse_raw(raw, path)
    }

    pub fn load(p: &Path) -> R<Atlas> {
        let raw = read_str(p)?;
        Atlas::parse_raw(&raw, &p.to_string_lossy())
    }

    fn parse_raw(raw: &str, path: &str) -> R<Atlas> {
        let mut a = Atlas {
            ..Default::default()
        };
        let mut cur: Option<Branch> = None;
        let mut section: Option<String> = None;
        let mut section_has_card = false;

        let close = |a: &mut Atlas, cur: &mut Option<Branch>| {
            if let Some(b) = cur.take() {
                a.branches.push(b);
            }
        };

        // Byte-exact card boundaries: every source line belongs either to
        // `head_raw` or to exactly one card's `raw`, so a rewrite cannot lose
        // or duplicate a line.
        let lines: Vec<&str> = raw.split_inclusive('\n').collect();
        let mut card_starts: Vec<usize> = Vec::new();
        let mut section_line: Option<usize> = None;

        for (i, line) in lines.iter().enumerate() {
            let line = *line;
            let t = line.trim();
            if let Some(title) = t.strip_prefix("## ") {
                close(&mut a, &mut cur);
                section = Some(title.trim().to_string());
                section_has_card = false;
                section_line = Some(i);
                continue;
            }
            if t.is_empty() {
                continue;
            }
            if a.title.is_empty() {
                if let Some(h) = t.strip_prefix("# ") {
                    a.title = h.trim().to_string();
                    continue;
                }
            }
            let (k, v) = match t.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => {
                    match &mut cur {
                        Some(b) => {
                            if let Some(last) = b.fields.last_mut() {
                                last.1.push('\n');
                                last.1.push_str(t);
                            }
                        }
                        None => a.preamble.push(t.to_string()),
                    }
                    continue;
                }
            };
            if k.is_empty()
                || !k
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                match &mut cur {
                    Some(b) => {
                        if let Some(last) = b.fields.last_mut() {
                            last.1.push('\n');
                            last.1.push_str(t);
                        }
                    }
                    None => a.preamble.push(t.to_string()),
                }
                continue;
            }
            if k == "BRANCH" {
                // A BRANCH line starts a new card when the current one is
                // already complete enough to stand on its own.
                let start_new = match &cur {
                    None => true,
                    Some(b) => {
                        b.get("BRANCH").len() > 0 && (b.get("PATH").len() > 0 || b.get("STATUS").len() > 0)
                    }
                };
                if start_new {
                    close(&mut a, &mut cur);
                    let emit = !section_has_card;
                    section_has_card = true;
                    let mut b = Branch {
                        heading: section.clone().unwrap_or_default(),
                        emit_heading: emit,
                        fields: Vec::new(),
                        raw: String::new(),
                    };
                    card_starts.push(if emit {
                        section_line.unwrap_or(i)
                    } else {
                        i
                    });
                    b.fields.push(("BRANCH".into(), v.to_string()));
                    cur = Some(b);
                } else if let Some(b) = &mut cur {
                    b.set("BRANCH", v);
                }
                continue;
            }
            // A trailing bare `PATH=` is a card terminator written by the map
            // generator; it must not erase paths already listed on this card.
            // A bare `PATH=` after a real path list is an end-of-card marker.
            // It is preserved inside `raw`, so it only has to be skipped here.
            if k == "PATH" && v.is_empty() {
                if let Some(b) = &cur {
                    if !b.get("PATH").is_empty() {
                        continue;
                    }
                }
            }
            // A card value may carry a following card field inside it, e.g.
            // canonical/Atlas.md has READ_WHEN embedded in a SUMMARY value.
            // Split it out so the real field is not lost.
            let mut extra: Vec<(String, String)> = Vec::new();
            let mut head_val = v.to_string();
            if !CARD_KEYS.contains(&k) {
                // not a card field; keep verbatim
            } else {
                // scan for '；KEY=' where KEY is a card field
                let mut search = 0usize;
                loop {
                    let rest = &v[search..];
                    let mut found: Option<(usize, usize, String)> = None;
                    for (i, c) in rest.char_indices() {
                        if c == '；' || c == ';' {
                            let after = search + i + c.len_utf8();
                            let tail = &v[after..];
                            let kend: String = tail
                                .chars()
                                .take_while(|x| x.is_ascii_uppercase() || *x == '_')
                                .collect();
                            if !kend.is_empty()
                                && CARD_KEYS.contains(&kend.as_str())
                                && tail.chars().nth(kend.chars().count()) == Some('=')
                            {
                                found = Some((search + i, after + kend.len() + 1, kend));
                                break;
                            }
                        }
                    }
                    match found {
                        Some((cut, vstart, kk)) => {
                            head_val = v[..cut].to_string();
                            // the embedded value runs to the end of the line
                            extra.push((kk.clone(), v[vstart..].to_string()));
                            search = v.len();
                            let _ = search;
                            break;
                        }
                        None => break,
                    }
                }
            }
            match &mut cur {
                Some(b) => {
                    if let Some(e) = b.fields.iter_mut().find(|(kk, _)| kk == k) {
                        e.1 = head_val.clone();
                    } else {
                        b.fields.push((k.to_string(), head_val.clone()));
                    }
                    for (kk, vv) in extra {
                        if let Some(e) = b.fields.iter_mut().find(|(x, _)| *x == kk) {
                            if e.1.trim().is_empty() {
                                e.1 = vv;
                            }
                        } else {
                            b.fields.push((kk, vv));
                        }
                    }
                }
                None => {
                    if let Some(e) = a.header.iter_mut().find(|(kk, _)| kk == k) {
                        e.1 = v.to_string();
                    } else {
                        a.header.push((k.to_string(), v.to_string()));
                    }
                    if let Some(t) = t.strip_prefix("# ") {
                        if a.title.is_empty() {
                            a.title = t.to_string();
                        }
                    }
                }
            }
        }
        close(&mut a, &mut cur);
        if card_starts.len() == a.branches.len() && !card_starts.is_empty() {
            a.head_raw = lines[..card_starts[0]].concat();
            for j in 0..a.branches.len() {
                let end = if j + 1 < card_starts.len() {
                    card_starts[j + 1]
                } else {
                    lines.len()
                };
                a.branches[j].raw = lines[card_starts[j]..end].concat();
            }
        } else {
            a.head_raw = raw.to_string();
        }
        // title from the leading "# ..." line
        if a.title.is_empty() {
            for line in raw.lines() {
                if let Some(t) = line.trim().strip_prefix("# ") {
                    a.title = t.to_string();
                    break;
                }
            }
        }
        Ok(a)
    }

    pub fn render(&self) -> String {
        if !self.head_raw.is_empty() {
            let mut s = String::with_capacity(
                self.head_raw.len() + self.branches.iter().map(|b| b.raw.len()).sum::<usize>(),
            );
            s.push_str(&self.head_raw);
            // A card created in this run has no source bytes, so skipping it
            // here dropped it from the output entirely. `Atlas::load` sets
            // head_raw to the whole file when it finds no cards at all, so the
            // very first write into an empty library produced an atlas with a
            // correct RECORD_COUNT and zero PATH entries -- the record existed
            // but could not be found through the map.
            let mut emitted_any = false;
            for b in &self.branches {
                if !b.raw.is_empty() {
                    s.push_str(&b.raw);
                    emitted_any = true;
                    continue;
                }
                if b.emit_heading {
                    if !emitted_any && !s.ends_with('\n') {
                        s.push('\n');
                    }
                    s.push('\n');
                    s.push_str(&format!("## {}\n", b.heading));
                }
                for (k, v) in &b.fields {
                    s.push_str(&format!("{k}={v}\n"));
                }
                emitted_any = true;
            }
            return s;
        }
        let mut s = String::new();
        s.push_str(&format!("# {}\n\n", self.title));
        for (k, v) in &self.header {
            s.push_str(&format!("{k}={v}\n"));
        }
        for l in &self.preamble {
            s.push_str(l);
            s.push('\n');
        }
        for b in &self.branches {
            if !b.raw.is_empty() {
                s.push_str(&b.raw);
                continue;
            }
            // Newly created card: no source bytes to preserve.
            if b.emit_heading {
                s.push('\n');
                s.push_str(&format!("## {}\n", b.heading));
            }
            for (k, v) in &b.fields {
                s.push_str(&format!("{k}={v}\n"));
            }
        }
        s
    }

    pub fn branch(&self, name: &str) -> Option<&Branch> {
        self.branches.iter().find(|b| b.name() == name)
    }
    pub fn all_paths(&self) -> Vec<String> {
        let mut v = Vec::new();
        for b in &self.branches {
            v.extend(b.paths());
        }
        v
    }

    /// Distinct record basenames mapped by any branch.
    pub fn mapped_names(&self) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for p in self.all_paths() {
            let b = p.rsplit('/').next().unwrap_or(&p).to_string();
            if !seen.contains(&b) {
                seen.push(b);
            }
        }
        seen
    }

    pub fn total_count(&self) -> usize {
        self.mapped_names().len()
    }

    /// Branch holding the given record (by basename).
    pub fn branch_of(&self, rec_rel: &str) -> Option<String> {
        let base = rec_rel.rsplit('/').next().unwrap_or(rec_rel);
        for b in &self.branches {
            for p in b.paths() {
                if p.rsplit('/').next().unwrap_or(&p) == base {
                    return Some(b.name());
                }
            }
        }
        None
    }
}

/// Resolve one `PATH=` element against the library root.
///
/// The shipped library writes both forms and mixes them inside a single card:
/// 185 elements are `canonical/records/<name>.md` and 136 are a bare
/// `<name>.md`. A bare name means "in this library's records directory", so it
/// cannot be joined to the root directly -- doing that made MAXSIZE come out
/// as 0 for every card that used the short form.
pub fn resolve_path(root: &std::path::Path, lib: crate::rec::Lib, elem: &str) -> std::path::PathBuf {
    if elem.contains('/') {
        root.join(elem)
    } else {
        // `Lib::dir()` already ends in `/records`.
        root.join(lib.dir()).join(elem)
    }
}

/// Where new records land when no branch is given.
pub const DEFAULT_BRANCH: &str = "unclassified";

/// Append a record path to a branch, creating the branch when missing.
pub fn add_to_branch(a: &mut Atlas, branch: &str, rec_rel: &str) -> bool {
    let base = rec_rel.rsplit('/').next().unwrap_or(rec_rel).to_string();
    if let Some(idx) = a.branches.iter().position(|b| b.name() == branch) {
        let mut ps = a.branches[idx].paths();
        if ps.iter().any(|p| p.rsplit('/').next().unwrap_or(p) == base) {
            return false;
        }
        ps.push(rec_rel.to_string());
        a.branches[idx].set_paths(&ps);
        let c = a.branches[idx].count();
        a.branches[idx].set("COUNT", &c.to_string());
        let rev = a.branches[idx].get("REVISION").parse::<u64>().unwrap_or(0) + 1;
        a.branches[idx].set("REVISION", &rev.to_string());
        return true;
    }
    // Reuse a card that exists but currently maps nothing.
    if let Some(idx) = a
        .branches
        .iter()
        .position(|b| b.count() == 0 && b.get("PATH").is_empty())
    {
        a.branches[idx].set("BRANCH", branch);
        a.branches[idx].set("PATH", rec_rel);
        a.branches[idx].set("COUNT", "1");
        let rev = a.branches[idx].get("REVISION").parse::<u64>().unwrap_or(0) + 1;
        a.branches[idx].set("REVISION", &rev.to_string());
        return true;
    }
    let mut b = Branch {
        heading: branch.to_string(),
        emit_heading: true,
        fields: Vec::new(),
        raw: String::new(),
    };
    b.fields.push(("BRANCH".into(), branch.to_string()));
    b.fields.push(("KEYS".into(), String::new()));
    b.fields.push(("COUNT".into(), "1".into()));
    b.fields.push(("MAXSIZE".into(), "0".into()));
    b.fields.push(("SUMMARY".into(), String::new()));
    b.fields.push(("READ_WHEN".into(), String::new()));
    b.fields.push(("PATH".into(), rec_rel.to_string()));
    b.fields.push(("REVISION".into(), "1".into()));
    b.fields.push(("STATUS".into(), "ACTIVE".into()));
    a.branches.push(b);
    true
}

/// Remove a record from every branch; returns branches touched.
pub fn remove_from_all(a: &mut Atlas, rec_rel: &str) -> Vec<String> {
    let base = rec_rel.rsplit('/').next().unwrap_or(rec_rel).to_string();
    let mut touched = Vec::new();
    for b in a.branches.iter_mut() {
        let ps = b.paths();
        let kept: Vec<String> = ps
            .iter()
            .filter(|p| p.rsplit('/').next().unwrap_or(p) != base.as_str())
            .cloned()
            .collect();
        if kept.len() != ps.len() {
            b.set_paths(&kept);
            let c = b.count();
            b.set("COUNT", &c.to_string());
            let rev = b.get("REVISION").parse::<u64>().unwrap_or(0) + 1;
            b.set("REVISION", &rev.to_string());
            touched.push(b.name());
        }
    }
    touched
}
