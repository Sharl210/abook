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
];

#[derive(Clone, Debug, Default)]
pub struct Branch {
    pub heading: String,
    pub emit_heading: bool,
    pub fields: Vec<(String, String)>,
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
    pub path: String,
    pub title: String,
    pub header: Vec<(String, String)>,
    pub branches: Vec<Branch>,
    pub preamble: Vec<String>,
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
        if let Some(e) = self.header.iter_mut().find(|(kk, _)| kk == k) {
            e.1 = v.to_string();
        } else {
            self.header.push((k.to_string(), v.to_string()));
        }
    }

    pub fn load(p: &Path) -> R<Atlas> {
        let raw = read_str(p)?;
        let mut a = Atlas {
            path: p.to_string_lossy().to_string(),
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

        for line in raw.lines() {
            let t = line.trim();
            if let Some(title) = t.strip_prefix("## ") {
                close(&mut a, &mut cur);
                section = Some(title.trim().to_string());
                section_has_card = false;
                continue;
            }
            if t.is_empty() {
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
                    };
                    b.fields.push(("BRANCH".into(), v.to_string()));
                    cur = Some(b);
                } else if let Some(b) = &mut cur {
                    b.set("BRANCH", v);
                }
                continue;
            }
            // A trailing bare `PATH=` is a card terminator written by the map
            // generator; it must not erase paths already listed on this card.
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
    pub fn branch_mut(&mut self, name: &str) -> Option<&mut Branch> {
        self.branches.iter_mut().find(|b| b.name() == name)
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
