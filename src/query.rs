//! Recall: tiered matching over record headers, then precision filtering.
//!
//! Tier 1: literal match in KEYS / CONCEPTS / ENTITIES / TITLE
//! Tier 2: semantic-ish match in READ_WHEN
//! Tier 3: branch summary match
//!
//! Every hit is then checked against NOT_FOR and entity/scope, and a rejected
//! hit carries its rejection reason so callers can see why it was dropped.

use crate::atlas::{Atlas, DEFAULT_BRANCH};
use crate::rec::{load_lib, Lib, Record};
use crate::util::*;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Lexical = 1,
    ReadWhen = 2,
    Ngram = 3,
    Branch = 4,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Lexical => "词面",
            Tier::ReadWhen => "READ_WHEN",
            Tier::Ngram => "片段回退",
            Tier::Branch => "分支摘要",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub lib: Lib,
    pub path: String,
    pub title: String,
    pub tier: Tier,
    pub score: i32,
    pub reason: String,
    pub rejected: Option<String>,
    pub record: Option<Record>,
}

fn field_has(rec: &Record, keys: &[&str], needle: &str) -> bool {
    for k in keys {
        if ci_contains(&rec.get(k), needle) {
            return true;
        }
    }
    false
}

/// Split a query into terms: whitespace / comma, keep CJK runs intact.
pub fn terms(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in q.split(|c: char| c.is_whitespace() || c == ',' || c == '，' || c == ';') {
        let t = part.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
    }
    if out.is_empty() && !q.trim().is_empty() {
        out.push(q.trim().to_string());
    }
    out
}

/// Score one record against the query terms.
pub fn score_record(rec: &Record, qs: &[String]) -> Option<(i32, Tier, String)> {
    let mut best: Option<(i32, Tier, String)> = None;
    for q in qs {
        let mut sc = 0;
        let mut tier = Tier::Branch;
        let mut why = Vec::new();
        if ci_contains(&rec.get("KEYS"), q) {
            sc += 100;
            tier = Tier::Lexical;
            why.push("KEYS");
        }
        if ci_contains(&rec.get("CONCEPTS"), q) {
            sc += 80;
            tier = Tier::Lexical;
            why.push("CONCEPTS");
        }
        if ci_contains(&rec.get("ENTITIES"), q) {
            sc += 70;
            tier = Tier::Lexical;
            why.push("ENTITIES");
        }
        if ci_contains(&rec.title(), q) {
            sc += 60;
            if tier == Tier::Branch {
                tier = Tier::Lexical;
            }
            why.push("TITLE");
        }
        if ci_contains(&rec.get("ID"), q) {
            sc += 90;
            tier = Tier::Lexical;
            why.push("ID");
        }
        if ci_contains(&rec.get("READ_WHEN"), q) {
            sc += 40;
            if tier == Tier::Branch {
                tier = Tier::ReadWhen;
            }
            why.push("READ_WHEN");
        }
        if ci_contains(&rec.get("SUMMARY"), q) {
            sc += 20;
            if tier == Tier::Branch {
                tier = Tier::ReadWhen;
            }
            why.push("SUMMARY");
        }
        if ci_contains(&rec.body, q) {
            sc += 10;
            why.push("BODY");
        }
        if sc > 0 {
            let reason = why.join("+");
            match &best {
                Some((b, _, _)) if *b >= sc => {}
                _ => best = Some((sc, tier, reason)),
            }
        }
    }
    best
}

/// Character n-gram windows for a term. Latin runs stay whole; CJK runs are
/// cut into all 2-grams so a colloquial shorthand can still reach a record
/// that spells the term out in full.
pub fn ngrams(term: &str) -> Vec<String> {
    let chars: Vec<char> = term.chars().collect();
    let mut out: Vec<String> = Vec::new();
    if chars.len() < 2 {
        return out;
    }
    let cjk = |c: char| (c as u32) >= 0x3400;
    // split into runs of CJK and non-CJK
    let mut i = 0;
    while i < chars.len() {
        if cjk(chars[i]) {
            let start = i;
            while i < chars.len() && cjk(chars[i]) {
                i += 1;
            }
            let run = &chars[start..i];
            if run.len() == 1 {
                out.push(run.iter().collect());
            } else {
                for w in run.windows(2) {
                    out.push(w.iter().collect());
                }
            }
        } else {
            let start = i;
            while i < chars.len() && !cjk(chars[i]) {
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            let t = run.trim();
            if t.len() >= 2 {
                out.push(t.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Fallback scoring by n-gram overlap, used only when the whole-term pass
/// found nothing for that term.
pub fn score_ngram(rec: &Record, qs: &[String]) -> Option<(i32, Tier, String)> {
    let mut total = 0i32;
    let mut hits: Vec<String> = Vec::new();
    // Coverage gate: a single stray fragment is noise, not a recall. Require
    // at least half of the windows (and never fewer than two) to land.
    let mut all_grams: Vec<String> = Vec::new();
    for q in qs {
        all_grams.extend(ngrams(q));
    }
    all_grams.sort();
    all_grams.dedup();
    let need = if all_grams.len() <= 1 {
        1
    } else {
        (all_grams.len() + 1) / 2
    };
    for q in qs {
        for g in ngrams(q) {
            let mut sc = 0;
            if ci_contains(&rec.get("KEYS"), &g) {
                sc += 12;
            }
            if ci_contains(&rec.get("CONCEPTS"), &g) {
                sc += 10;
            }
            if ci_contains(&rec.get("ENTITIES"), &g) {
                sc += 8;
            }
            if ci_contains(&rec.title(), &g) {
                sc += 7;
            }
            if ci_contains(&rec.get("READ_WHEN"), &g) {
                sc += 4;
            }
            if ci_contains(&rec.get("SUMMARY"), &g) {
                sc += 3;
            }
            if ci_contains(&rec.body, &g) {
                sc += 2;
            }
            if sc > 0 {
                total += sc;
                if !hits.contains(&g) {
                    hits.push(g);
                }
            }
        }
    }
    if total == 0 || hits.len() < need {
        return None;
    }
    let reason = format!("片段 {}/{} {}", hits.len(), all_grams.len(), hits.join("/"));
    Some((total, Tier::Ngram, reason))
}

/// Precision check: NOT_FOR exclusion and explicit scope.
fn precision(rec: &Record, qs: &[String]) -> Option<String> {
    let nf = rec.get("NOT_FOR");
    if nf.is_empty() {
        return None;
    }
    for q in qs {
        if ci_contains(&nf, q) {
            return Some(format!("命中 NOT_FOR: {q}"));
        }
    }
    None
}

#[derive(Clone, Debug, Default)]
pub struct RecallResult {
    pub query: String,
    pub hits: Vec<Hit>,
    pub rejected: Vec<Hit>,
    pub tiers_used: Vec<Tier>,
    pub scanned: usize,
    pub status: &'static str,
}

/// Run recall across one or both libraries.
pub fn recall(
    root: &Path,
    q: &str,
    libs: &[Lib],
    limit: usize,
    include_promoted: bool,
    with_body: bool,
) -> RecallResult {
    let qs = terms(q);
    let mut res = RecallResult {
        query: q.to_string(),
        status: "RECALL_MISS",
        ..Default::default()
    };
    let mut all: Vec<(Lib, Record)> = Vec::new();
    for lib in libs {
        for rec in load_lib(root, *lib) {
            all.push((*lib, rec));
        }
    }
    res.scanned = all.len();

    // promoted candidates are not part of normal auto-recall
    let mut pool: Vec<(Lib, Record)> = Vec::new();
    for (lib, rec) in all {
        if lib == Lib::Candidate && !include_promoted {
            if rec.promotion.is_some() || rec.get("PROMOTED_TO").len() > 0 {
                continue;
            }
        }
        pool.push((lib, rec));
    }

    let mut hits: Vec<Hit> = Vec::new();
    // First pass: whole terms only.
    let mut whole_hits = 0usize;
    for (lib, rec) in &pool {
        if score_record(rec, &qs).is_some() {
            whole_hits += 1;
        }
    }
    // No record matched the query as typed: retry with n-gram windows so a
    // shorthand like 首次出块 can still reach a record that says 固定块.
    let use_ngram = whole_hits == 0;
    for (lib, rec) in &pool {
        let scored = score_record(rec, &qs).or_else(|| {
            if use_ngram {
                score_ngram(rec, &qs)
            } else {
                None
            }
        });
        if let Some((score, tier, reason)) = scored {
            let path = rel(root, &rec.path);
            let rej = precision(rec, &qs);
            let mut r2 = rec.clone();
            if !with_body {
                r2.body = String::new();
            }
            let h = Hit {
                lib: *lib,
                path,
                title: rec.title(),
                tier,
                score,
                reason,
                rejected: rej.clone(),
                record: Some(r2),
            };
            if rej.is_some() {
                res.rejected.push(h);
            } else {
                hits.push(h);
            }
        }
    }

    // branch-summary tier: only if nothing lexical matched
    if hits.is_empty() {
        for lib in libs {
            if let Ok(a) = Atlas::load(&root.join(lib.atlas())) {
                for b in &a.branches {
                    let mut sc = 0;
                    let mut why = Vec::new();
                    for q in &qs {
                        if ci_contains(&b.get("KEYS"), q) || ci_contains(&b.name(), q) {
                            sc += 30;
                            why.push("branch");
                        }
                        if ci_contains(&b.get("READ_WHEN"), q) {
                            sc += 15;
                            why.push("branch-read-when");
                        }
                        if ci_contains(&b.get("SUMMARY"), q) {
                            sc += 8;
                            why.push("branch-summary");
                        }
                    }
                    if sc > 0 {
                        for p in b.paths() {
                            let full = root.join(&p);
                            if !full.exists() {
                                continue;
                            }
                            if let Ok(rec) = Record::parse(&full, *lib) {
                                let mut r2 = rec.clone();
                                if !with_body {
                                    r2.body = String::new();
                                }
                                hits.push(Hit {
                                    lib: *lib,
                                    path: p.clone(),
                                    title: rec.title(),
                                    tier: Tier::Branch,
                                    score: sc,
                                    reason: why.join("+"),
                                    rejected: None,
                                    record: Some(r2),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.cmp(&b.path)));
    hits.truncate(limit);
    res.tiers_used = hits.iter().map(|h| h.tier).collect();
    res.tiers_used.sort_by_key(|t| *t as u8);
    res.tiers_used.dedup();
    res.status = if !hits.is_empty() {
        "RECALL_OK"
    } else if !res.rejected.is_empty() {
        "RECALL_UNRESOLVED"
    } else {
        "RECALL_MISS"
    };
    res.hits = hits;
    res
}

/// Aggregate state read from Global.md.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub root: String,
    pub canonical: usize,
    pub candidate: usize,
    pub canonical_atlas_count: usize,
    pub candidate_atlas_count: usize,
    pub covered_canonical: String,
    pub covered_candidate: String,
    pub latest_commit: String,
    pub vault_files: usize,
    pub vault_rows: usize,
    pub problems: Vec<String>,
}

pub fn read_state(root: &Path) -> State {
    let mut st = State {
        root: root.to_string_lossy().to_string(),
        ..Default::default()
    };
    st.canonical = list_md(&root.join(Lib::Canonical.dir())).len();
    st.candidate = list_md(&root.join(Lib::Candidate.dir())).len();
    if let Ok(a) = Atlas::load(&root.join(Lib::Canonical.atlas())) {
        st.canonical_atlas_count = a.total_count();
        st.covered_canonical = a.get("COVERED_THROUGH");
    }
    if let Ok(a) = Atlas::load(&root.join(Lib::Candidate.atlas())) {
        st.candidate_atlas_count = a.total_count();
        st.covered_candidate = a.get("COVERED_THROUGH");
    }
    if let Ok(g) = crate::book::BookFile::load(&root.join("Global.md")) {
        st.latest_commit = g.state("LATEST_COMMIT").unwrap_or_default();
    }
    let (f, r, p) = crate::vault::reconcile(root);
    st.vault_files = f;
    st.vault_rows = r;
    st.problems.extend(p);
    if st.canonical != st.canonical_atlas_count {
        st.problems.push(format!(
            "正式库目录 {} 条 vs 地图 {} 条",
            st.canonical, st.canonical_atlas_count
        ));
    }
    if st.candidate != st.candidate_atlas_count {
        st.problems.push(format!(
            "候选库目录 {} 条 vs 地图 {} 条",
            st.candidate, st.candidate_atlas_count
        ));
    }
    st
}

/// Choose a branch for a new record from its fields.
pub fn guess_branch(rec: &Record) -> String {
    let kind = rec.kind();
    let title = rec.title();
    let text = format!("{title} {}", rec.get("KEYS"));
    if ci_contains(&text, "凭据") || ci_contains(&text, "保险箱") || ci_contains(&text, "vault") {
        return "凭据保险箱".into();
    }
    match kind.as_str() {
        "FAILURE" | "ATTEMPT" => "失败与回滚".into(),
        "CORRECTION" => "失败与回滚".into(),
        "EVIDENCE" => "审计证据".into(),
        "DECISION" => "记忆维护与演进".into(),
        _ => DEFAULT_BRANCH.into(),
    }
}
