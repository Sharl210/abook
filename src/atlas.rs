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

/// One entry of the `FILES=` list form.
///
/// A different library layout (`FORMAT=AGENTSBOOK_V2_EXPERIMENT`, as shipped by
/// the AgentsBook v2 migration scripts) lists records as
///
/// ```text
/// FILES=
///   - AB2-CMP-074|FAILURE|VERIFIED|canonical/records/C-xxx.md|<title>
/// ```
///
/// instead of a comma-separated `PATH=`. The title is free text and may itself
/// contain `|`, so only the first four separators are structural.
#[derive(Clone, Debug, Default)]
pub struct FilesEntry {
    pub id: String,
    pub kind: String,
    pub maturity: String,
    pub path: String,
    pub title: String,
}

impl FilesEntry {
    /// Parse one `  - a|b|c|path|title` line; `None` when it is not one.
    fn parse(line: &str) -> Option<FilesEntry> {
        let t = line.trim();
        let rest = t.strip_prefix("- ")?;
        let parts: Vec<&str> = rest.splitn(5, '|').collect();
        if parts.len() < 4 || parts[3].trim().is_empty() {
            return None;
        }
        let path = parts[3].trim();
        // A record path always names a markdown file. Requiring that keeps an
        // ordinary `- some bullet` line in a summary from being read as an
        // entry and rewritten as one.
        if !path.ends_with(".md") {
            return None;
        }
        // The id column is a record id or short unit code: never prose. A summary
        // bullet that happens to contain four pipes and a `.md` name would
        // otherwise be read as an entry and then *written back* as one, inventing
        // a record the library does not have.
        let idcol = parts[0].trim();
        if idcol.is_empty() || idcol.split_whitespace().count() != 1 {
            return None;
        }
        if !idcol
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ':')
        {
            return None;
        }
        Some(FilesEntry {
            id: parts[0].trim().to_string(),
            kind: parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default(),
            maturity: parts.get(2).map(|s| s.trim().to_string()).unwrap_or_default(),
            path: path.to_string(),
            title: parts.get(4).map(|s| s.trim().to_string()).unwrap_or_default(),
        })
    }

    fn render(&self) -> String {
        format!(
            "  - {}|{}|{}|{}|{}",
            self.id, self.kind, self.maturity, self.path, self.title
        )
    }
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

    /// True when this card lists records as `FILES=` entry lines.
    ///
    /// Three signals, because the shipped library is not uniform: some cards
    /// put `- ...` lines directly under `BRANCH=`, some put them after
    /// `SUMMARY=`, and one uses `FILES=` as a marker with the entries further
    /// down. Keying off the `FILES=` marker alone read every variant as an
    /// empty card.
    pub fn uses_files(&self) -> bool {
        if !self.files().is_empty() || self.fields.iter().any(|(k, _)| k == "FILES") {
            return true;
        }
        self.raw.split_inclusive('\n').any(|l| {
            let t = l.trim_end_matches(['\n', '\r']);
            t.starts_with("FILES=")
        })
    }

    /// The text that holds this card's entry lines.
    ///
    /// `raw` wins when present -- it is what gets written and what the user's
    /// editor showed. A card created during this run has no source bytes, so its
    /// list only exists in the parsed field.
    fn files_source(&self) -> String {
        if !self.raw.is_empty() {
            return self.raw.clone();
        }
        self.get("FILES")
    }

    /// Every `- id|kind|maturity|path|title` line on this card, in source order.
    ///
    /// Read from the raw bytes rather than from `fields`: the parser folds the
    /// whole list into one multi-line field value, which is lossy for anything
    /// we want to rewrite line by line.
    pub fn files(&self) -> Vec<FilesEntry> {
        self.files_source()
            .lines()
            .filter_map(FilesEntry::parse)
            .collect()
    }

    /// Append one entry after the last existing one.
    ///
    /// Insertion keeps the list in place: the library's order carries meaning to
    /// whoever reads the map, so a new entry goes to the end of its own branch
    /// rather than to the end of the card or the file. When the card has no
    /// entries yet, it lands under its `FILES=` marker if there is one.
    pub fn push_files_entry(&mut self, entry: &FilesEntry) {
        let line = entry.render();
        let mut lines: Vec<String> = self
            .raw
            .split_inclusive('\n')
            .map(|s| s.to_string())
            .collect();

        let last_entry = lines
            .iter()
            .rposition(|l| FilesEntry::parse(l).is_some());
        let anchor = match last_entry {
            Some(i) => Some(i + 1),
            None => lines.iter().position(|l| l.starts_with("FILES=")).map(|i| i + 1),
        };
        match anchor {
            Some(i) => lines.insert(i, format!("{line}\n")),
            None => {
                if !self.raw.is_empty() && !self.raw.ends_with('\n') {
                    lines.push("\n".to_string());
                }
                lines.push(format!("{line}\n"));
            }
        }
        self.raw = lines.concat();
        self.refresh_files_field();
    }

    /// Replace this card's whole entry list with `entries`.
    ///
    /// Needed when *creating* an entry rather than renaming one: `raw` is what
    /// gets written and `fields` is what queries read, so both have to carry the
    /// new row. Building the row from a default `FilesEntry` and pushing it
    /// through `sync_files_entries` left `kind`, `maturity` and `title` empty --
    /// the row existed, counted towards the totals, and told a reader nothing.
    pub fn set_files(&mut self, entries: &[FilesEntry]) {
        // Drop every entry line currently in the card, keeping everything else
        // (BRANCH, COUNT, SUMMARY, prose) in place.
        let kept: Vec<String> = self
            .raw
            .split_inclusive('\n')
            .filter(|l| FilesEntry::parse(l).is_none())
            .map(|s| s.to_string())
            .collect();
        self.raw = kept.concat();
        // Remove the old field so the marker is rebuilt rather than appended to.
        self.fields.retain(|(k, _)| k != "FILES");
        let joined = entries
            .iter()
            .map(|e| e.render())
            .collect::<Vec<_>>()
            .join("\n");
        if !joined.is_empty() {
            self.fields.push(("FILES".into(), joined.clone()));
        }
        // Write the rows into `raw` right after the `FILES=` marker when there is
        // one, otherwise append a marker plus the rows at the end.
        let mut out = String::new();
        let mut placed = false;
        for line in self.raw.split_inclusive('\n') {
            out.push_str(line);
            if !placed && line.trim_end().starts_with("FILES=") {
                for e in entries {
                    out.push_str(&e.render());
                    out.push('\n');
                }
                placed = true;
            }
        }
        if !placed && !joined.is_empty() {
            if !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
            }
            out.push_str("FILES=\n");
            for e in entries {
                out.push_str(&e.render());
                out.push('\n');
            }
        }
        self.raw = out;
        self.refresh_files_field();
    }

    /// Drop every entry whose path basename is `base`.
    pub fn drop_files_entry(&mut self, base: &str) {
        let mut changed = false;
        let kept: Vec<String> = self
            .raw
            .split_inclusive('\n')
            .filter(|l| {
                match FilesEntry::parse(l) {
                    Some(e) => {
                        if e.path.rsplit('/').next().unwrap_or(&e.path) == base {
                            changed = true;
                            false
                        } else {
                            true
                        }
                    }
                    None => true,
                }
            })
            .map(|s| s.to_string())
            .collect();
        if changed {
            self.raw = kept.concat();
            self.refresh_files_field();
        }
    }

    /// Keep the parsed `FILES` field in step with the bytes we just edited.
    ///
    /// `raw` is what gets written; `fields` is what queries read. They are two
    /// views of one card, and `render()` prefers `raw` -- so a stale `fields`
    /// would answer from an old list while the file on disk says something else.
    fn refresh_files_field(&mut self) {
        let entries = self.files();
        let joined = entries
            .iter()
            .map(|e| e.render())
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(e) = self.fields.iter_mut().find(|(k, _)| k == "FILES") {
            e.1 = joined;
        } else if self.uses_files() {
            self.fields.push(("FILES".into(), joined));
        }
    }

    /// Make this card's `FILES=` list match `elems` (paths), preserving the
    /// existing lines for paths that stay.
    fn sync_files_entries(&mut self, elems: &[String]) {
        let want: Vec<String> = elems
            .iter()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            .collect();
        for e in self.files() {
            let base = e.path.rsplit('/').next().unwrap_or(&e.path).to_string();
            if !want.contains(&base) {
                self.drop_files_entry(&base);
            }
        }
        for p in elems {
            let base = p.rsplit('/').next().unwrap_or(p).to_string();
            if !self
                .files()
                .iter()
                .any(|e| e.path.rsplit('/').next().unwrap_or(&e.path) == base)
            {
                let mut e = FilesEntry::default();
                e.id = base.trim_end_matches(".md").to_string();
                e.path = p.clone();
                self.push_files_entry(&e);
            }
        }
    }

    /// Raw PATH entries with directory inheritance applied. Duplicates kept.
    pub fn paths(&self) -> Vec<String> {
        // The `FILES=` layout wins whenever it is present: its entries are the
        // complete list, while any `PATH=` on such a card is a human note about
        // the directory (one card literally says
        // `PATH=candidate/records/（下列 FILES 的父目录）`) and must never be
        // mistaken for a record.
        if self.uses_files() {
            return self.files().iter().map(|e| e.path.clone()).collect();
        }
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
        if self.uses_files() {
            return self.files().iter().map(|e| e.path.clone()).collect();
        }
        self.get("PATH")
            .split([',', '，'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Replace `PATH=` with these elements verbatim. No dedupe, no directory
    /// abbreviation: what you pass is what gets written.
    pub fn set_paths_raw(&mut self, elems: &[String]) {
        if self.uses_files() {
            // Rewriting a `PATH=` line here would leave the card with two
            // layouts and no readers. Edit the list it actually has instead.
            self.sync_files_entries(elems);
            return;
        }
        self.set("PATH", &elems.join(","));
    }

    pub fn set_paths(&mut self, paths: &[String]) {
        if self.uses_files() {
            self.sync_files_entries(paths);
            return;
        }
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
            // `- id|kind|maturity|path|title` is a list entry, not a wrapped
            // value. This test has to run *before* the `=` split: a record title
            // may legitimately contain `=` (a real one reads "根因=无语音机型…"),
            // and then the line used to take the field-shaped path, fail the
            // uppercase-key check and be appended to the previous field. For a
            // card that places entries straight under `BRANCH=`, the branch *name*
            // swallowed a whole record title, so `branch` lookups matched nothing
            // and a later move created a second card for the same branch.
            if let Some(e) = FilesEntry::parse(t) {
                if let Some(b) = &mut cur {
                    let line = e.render();
                    match b.fields.iter_mut().find(|(kk, _)| kk == "FILES") {
                        Some(f) if f.1.is_empty() => f.1 = line,
                        Some(f) => {
                            f.1.push('\n');
                            f.1.push_str(&line);
                        }
                        None => b.fields.push(("FILES".into(), line)),
                    }
                } else {
                    a.preamble.push(t.to_string());
                }
                continue;
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
                        // `FILES=` counts as "complete enough to stand on its
                        // own". Without it, every card in the shipped library
                        // that carries `BRANCH` + `SUMMARY` + `FILES` (no
                        // `PATH`, no `STATUS`) was treated as unfinished and
                        // had its BRANCH overwritten by the next card's name,
                        // silently folding several branches into one.
                        b.get("BRANCH").len() > 0
                            && (b.get("PATH").len() > 0
                                || b.get("STATUS").len() > 0
                                || b.uses_files())
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
            // Same rule for `FILES=`. Some cards list entries *before* the marker
            // (and interleave `COUNT=`), then start the marked list with a bare
            // `FILES=`. Letting that empty assignment overwrite the field dropped
            // every entry already collected above it: one real card has 16 such
            // entries, so `--json` reported 44 entries while the byte-exact view
            // reported all 59. Two answers to the same question is worse than
            // either, and the short one is the one queries read.
            if k == "FILES" && v.is_empty() {
                if let Some(b) = &cur {
                    if b.files().is_empty() || !b.get("FILES").is_empty() {
                        if !b.get("FILES").is_empty() {
                            continue;
                        }
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
                    // A `FILES` list is a marker followed by entry lines:
                    //   FILES=
                    //     - id|kind|maturity|path|title
                    // Writing `FILES={entries}` glued the first entry onto the
                    // marker line, which no reader of this format accepts -- the
                    // card then held one record the map could not show.
                    if k == "FILES" && !v.is_empty() && !v.starts_with('\n') {
                        s.push_str(&format!("{k}=\n{v}\n"));
                    } else {
                        s.push_str(&format!("{k}={v}\n"));
                    }
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

    /// True when most cards in this map list records as `FILES=` entries.
    ///
    /// A map is written in one style by whoever maintains it, so the majority is
    /// the map's own answer to "what shape should a new card be". Without this,
    /// adding a record to the FILES-style library created a one-line `PATH=`
    /// card: the record landed on disk and `state` counted it, but
    /// `grep -c '^  - ' Atlas.md` did not -- four counts that are supposed to
    /// agree stopped agreeing, and every later reconciliation had to guess which
    /// one lied.
    pub fn prefers_files(&self) -> bool {
        let total = self.branches.len();
        if total == 0 {
            return false;
        }
        let files = self.branches.iter().filter(|b| b.uses_files()).count();
        files * 2 > total
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
/// Metadata a new `FILES=` entry has to carry.
///
/// A map entry is `id|kind|maturity|path|title`. Creating one with only `id` and
/// `path` produced `id|||path|` -- a row that still counts towards the map's
/// totals but tells a reader nothing, and that `atlas --card` shows as a record
/// with no kind, no maturity and no title. It also made the same record appear
/// twice (once complete, once empty) after `restore`, because `delete` removes
/// the entry and `restore` has to guess the branch again.
#[derive(Default, Clone)]
pub struct EntryMeta {
    pub kind: String,
    pub maturity: String,
    pub title: String,
}

pub fn add_to_branch(a: &mut Atlas, branch: &str, rec_rel: &str) -> bool {
    add_to_branch_meta(a, branch, rec_rel, &EntryMeta::default())
}

/// `add_to_branch`, telling the new entry what the record actually is.
pub fn add_to_branch_meta(
    a: &mut Atlas,
    branch: &str,
    rec_rel: &str,
    meta: &EntryMeta,
) -> bool {
    let base = rec_rel.rsplit('/').next().unwrap_or(rec_rel).to_string();
    let files_style = a.prefers_files();
    // One record belongs to one branch. Checking only the *target* branch let the
    // same record land in a second branch whenever the caller could not recover
    // where it used to live (which is exactly what `delete` + `restore` does),
    // and the map then reported more records than the library holds.
    if let Some(home) = a.branch_of(rec_rel) {
        if home != branch {
            return false;
        }
    }
    if let Some(idx) = a.branches.iter().position(|b| b.name() == branch) {
        let mut ps = a.branches[idx].paths();
        if ps.iter().any(|p| p.rsplit('/').next().unwrap_or(p) == base) {
            return false;
        }
        ps.push(rec_rel.to_string());
        a.branches[idx].set_paths(&ps);
        // A FILES card is maintained by hand and carries `id|kind|maturity|
        // path|title` plus free-form prose. Injecting COUNT/REVISION into it
        // would be the tool editing lines the user never asked it to touch, and
        // an earlier version did exactly that. Only PATH-style cards get the
        // bookkeeping fields.
        if !a.branches[idx].uses_files() {
            let c = a.branches[idx].count();
            a.branches[idx].set("COUNT", &c.to_string());
            let rev = a.branches[idx].get("REVISION").parse::<u64>().unwrap_or(0) + 1;
            a.branches[idx].set("REVISION", &rev.to_string());
        }
        return true;
    }
    // Reuse a card that exists but currently maps nothing.
    if let Some(idx) = a
        .branches
        .iter()
        .position(|b| b.count() == 0 && b.get("PATH").is_empty())
    {
        a.branches[idx].set("BRANCH", branch);
        if a.branches[idx].uses_files() || files_style {
            // Keep the card in its own layout: an entry line, no `PATH=`.
            let mut e = FilesEntry::default();
            e.id = base.trim_end_matches(".md").to_string();
            e.path = rec_rel.to_string();
            e.kind = meta.kind.clone();
            e.maturity = meta.maturity.clone();
            e.title = meta.title.clone();
            a.branches[idx].set_files(&[e]);
        } else {
            a.branches[idx].set("PATH", rec_rel);
        }
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
    if files_style {
        // Match the map: a `FILES=` marker plus one entry. The id is the file
        // stem, which is what every entry in the shipped library uses.
        let mut e = FilesEntry::default();
        e.id = base.trim_end_matches(".md").to_string();
        e.path = rec_rel.to_string();
        e.kind = meta.kind.clone();
        e.maturity = meta.maturity.clone();
        e.title = meta.title.clone();
        b.fields.push(("FILES".into(), e.render()));
    } else {
        b.fields.push(("PATH".into(), rec_rel.to_string()));
    }
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
