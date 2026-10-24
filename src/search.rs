//! High-performance full-content search.
//!
//! Constraints that shape the design:
//!   * one binary, no side files — so no on-disk index may be built;
//!   * the corpus can be large, so a naive read-everything-and-scan loop is
//!     not acceptable;
//!   * the caller wants literal, regex and fuzzy matching over both metadata
//!     and prose.
//!
//! Strategy:
//!   1. Walk the record directories once and collect (path, size, mtime).
//!   2. Shard the file list across all cores (std::thread::scope, no deps).
//!      Each worker reads its own files and searches the RAW BYTES first —
//!      a byte-level prefilter is far cheaper than decoding and parsing.
//!   3. Only files whose bytes contain a candidate are decoded and parsed for
//!      field-scoped results and snippet extraction.
//!   4. Ranking is field-weighted: KEYS/CONCEPTS/TITLE hits outrank body hits,
//!      because a match in a recall key is what actually drives retrieval.
//!
//! Literal matching uses Boyer–Moore–Horspool over bytes, which is exact for
//! UTF-8 too (a UTF-8 substring match can never span a byte boundary in a way
//! that produces a false positive).

use crate::rec::{Lib, Record};
use crate::rx::Regex;
use crate::util::*;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Literal,
    Regex,
    Fuzzy,
    All,
}

#[derive(Clone, Debug)]
pub struct Opts {
    /// Match against `KEY=value` lines instead of bare values. This is what
    /// `^ID=` needs: the value alone never contains the field name.
    pub raw_lines: bool,
    pub patterns: Vec<String>,
    pub mode: Mode,
    pub ci: bool,
    pub fuzzy_k: usize,
    pub fields: Vec<String>,
    pub body_only: bool,
    pub context: usize,
    pub max_hits: usize,
    pub min_score: i32,
    pub lib: Option<Lib>,
    pub include_deleted: bool,
    pub include_promoted: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            patterns: Vec::new(),
            mode: Mode::All,
            ci: true,
            fuzzy_k: 1,
            fields: Vec::new(),
            body_only: false,
            context: 0,
            max_hits: 0,
            min_score: 0,
            raw_lines: false,
            lib: None,
            include_deleted: false,
            include_promoted: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: String,
    pub title: String,
    pub score: i32,
    pub where_: String,
    pub snippet: String,
    pub line: usize,
    /// When the record was last touched, so a hit list can be read in time
    /// order as well as score order. Ranked search stays sort-by-score -- that
    /// is what the query asked for -- but a reader still needs to know whether
    /// a match is from Tuesday or from three months ago.
    pub updated: i64,
    /// Size of the record file.
    ///
    /// Present so a caller can decide whether to page through it before
    /// paying to load it. A hit list without sizes forces a guess, and the
    /// guess is always wrong in the expensive direction.
    pub bytes: usize,
    /// Line count of the record file.
    ///
    /// Readers navigate by line number ("look at line 40"), so a listing that
    /// gives size but not lines still leaves them guessing how far in a
    /// location is. Both are cheap to compute from text already in hand.
    pub lines: usize,
}

// ---------------------------------------------------------------- BMH

fn bmh_any(hay: &[u8], needle: &[u8]) -> bool {
    let (n, m) = (hay.len(), needle.len());
    if m == 0 || m > n {
        return false;
    }
    if m == 1 {
        return hay.contains(&needle[0]);
    }
    let mut skip = [m; 256];
    for i in 0..m - 1 {
        skip[needle[i] as usize] = m - 1 - i;
    }
    let mut i = 0usize;
    while i + m <= n {
        let last = hay[i + m - 1];
        if last == needle[m - 1] && &hay[i..i + m] == needle {
            return true;
        }
        i += skip[last as usize].max(1);
    }
    false
}

/// Byte offset of the `ci`-th character in a char slice.
fn chars_to_byte(chars: &[char], ci: usize) -> usize {
    chars.iter().take(ci).map(|c| c.len_utf8()).sum()
}

/// Convert a character index into a byte index. Snippet extraction works on
/// bytes, so every matcher must report bytes or be converted here.
fn char_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices()
        .nth(ci)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn lower_bytes(b: &[u8]) -> Vec<u8> {
    b.to_ascii_lowercase()
}

// ---------------------------------------------------------------- fuzzy

/// Bounded edit distance between `pat` and the best substring of `text`.
/// Returns (distance, end_offset) of the best window.
/// Exposed for the self test.
pub fn fuzzy_best_pub(text: &[char], pat: &[char], k: usize) -> Option<(usize, usize)> {
    fuzzy_best(text, pat, k)
}

fn fuzzy_best(text: &[char], pat: &[char], k: usize) -> Option<(usize, usize)> {
    let m = pat.len();
    if m == 0 || text.is_empty() {
        return None;
    }
    // Sellers' algorithm: dp[j] = distance(pat[..j], text[..i])
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    let mut best: Option<(usize, usize)> = None;
    for i in 1..=text.len() {
        cur[0] = 0; // a match may start anywhere
        for j in 1..=m {
            let cost = if text[i - 1] == pat[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j - 1] + cost).min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        let d = cur[m];
        if d <= k {
            match best {
                Some((bd, _)) if bd <= d => {}
                _ => best = Some((d, i)),
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    best
}

// ---------------------------------------------------------------- scoring

const FIELD_WEIGHT: &[(&str, i32)] = &[
    ("KEYS", 100),
    ("CONCEPTS", 90),
    ("ENTITIES", 80),
    ("TITLE", 70),
    ("ID", 60),
    ("READ_WHEN", 45),
    ("NOT_FOR", 40),
    ("SUMMARY", 35),
    ("EVIDENCE", 20),
    ("SOURCE", 18),
    ("RELATED_IDS", 15),
    ("PREVIOUS_ID", 12),
    ("SUPERSEDES", 12),
    ("UNCERTAINTY", 10),
    ("NEXT_VALIDATION", 10),
    ("SECTIONS", 8),
];

fn field_weight(k: &str) -> i32 {
    FIELD_WEIGHT
        .iter()
        .find(|(kk, _)| *kk == k)
        .map(|(_, w)| *w)
        .unwrap_or(5)
}

// ---------------------------------------------------------------- matcher

struct Matcher {
    opts: Opts,
    /// compiled regexes (for Mode::Regex)
    res: Vec<Regex>,
    /// lowercased needles (for literal / fuzzy)
    needles: Vec<String>,
}

impl Matcher {
    fn new(opts: Opts) -> Result<Matcher, String> {
        let mut res = Vec::new();
        if opts.mode == Mode::Regex {
            for p in &opts.patterns {
                res.push(Regex::new(p, opts.ci)?);
            }
        }
        let needles: Vec<String> = opts
            .patterns
            .iter()
            .map(|p| if opts.ci { p.to_lowercase() } else { p.clone() })
            .collect();
        Ok(Matcher { opts, res, needles })
    }

    /// Cheap byte-level test used to skip a file entirely.
    fn bytes_possible(&self, raw: &[u8]) -> bool {
        match self.opts.mode {
            Mode::Regex => true, // must decode; still cheaper than parsing
            Mode::Fuzzy => true,
            _ => {
                let low;
                let hay: &[u8] = if self.opts.ci {
                    low = lower_bytes(raw);
                    &low
                } else {
                    raw
                };
                self.needles.iter().any(|n| bmh_any(hay, n.as_bytes()))
            }
        }
    }

    fn text_matches(&self, text: &str) -> Option<(i32, usize)> {
        if text.is_empty() {
            return None;
        }
        match self.opts.mode {
            Mode::Regex => {
                let chars: Vec<char> = text.chars().collect();
                let mut best: Option<(i32, usize)> = None;
                for re in &self.res {
                    if let Some((s, _)) = re.find_at(&chars, 0) {
                        let sc = 10 + (self.opts.patterns[0].chars().count() as i32).min(20);
                        match best {
                            Some((bs, _)) if bs >= sc => {}
                            _ => best = Some((sc, char_to_byte(text, s))),
                        }
                    }
                }
                best
            }
            Mode::Fuzzy => {
                let chars: Vec<char> = text.chars().collect();
                let lc: Vec<char> = if self.opts.ci {
                    text.to_lowercase().chars().collect()
                } else {
                    chars.clone()
                };
                let mut best: Option<(i32, usize)> = None;
                for n in &self.needles {
                    let pat: Vec<char> = n.chars().collect();
                    if let Some((d, end)) = fuzzy_best(&lc, &pat, self.opts.fuzzy_k) {
                        let sc = 20 - (d as i32) * 5;
                        let cpos = end.saturating_sub(pat.len());
                        match best {
                            Some((bs, _)) if bs >= sc => {}
                            _ => best = Some((sc, chars_to_byte(&lc, cpos))),
                        }
                    }
                }
                best
            }
            _ => {
                // Literal or All: substring, case-folded when requested
                let low;
                let hay: &str = if self.opts.ci {
                    low = text.to_lowercase();
                    &low
                } else {
                    text
                };
                let mut best: Option<(i32, usize)> = None;
                for n in &self.needles {
                    if let Some(pos) = hay.find(n.as_str()) {
                        let sc = 10 + (n.chars().count() as i32).min(20);
                        match best {
                            Some((bs, _)) if bs >= sc => {}
                            _ => best = Some((sc, pos)),
                        }
                    }
                }
                best
            }
        }
    }
}

// ---------------------------------------------------------------- run

pub fn search(root: &Path, opts: &Opts) -> Result<Vec<Hit>, String> {
    let libs: Vec<Lib> = match opts.lib {
        Some(l) => vec![l],
        None => vec![Lib::Canonical, Lib::Candidate],
    };

    // ---- collect the work list
    struct Job {
        path: std::path::PathBuf,
        lib: Lib,
    }
    let mut jobs: Vec<Job> = Vec::new();
    for lib in libs {
        let dir = root.join(lib.dir());
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "md").unwrap_or(false) {
                jobs.push(Job { path: p, lib });
            }
        }
    }
    if jobs.is_empty() {
        return Ok(Vec::new());
    }

    let m = Matcher::new(opts.clone())?;
    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1)
        .min(jobs.len());
    let chunk = (jobs.len() + nthreads - 1) / nthreads;
    let out: Mutex<Vec<Hit>> = Mutex::new(Vec::new());
    let scanned = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);

    std::thread::scope(|s| {
        for part in jobs.chunks(chunk.max(1)) {
            let m = &m;
            let out = &out;
            let scanned = &scanned;
            let skipped = &skipped;
            let root = root;
            s.spawn(move || {
                let mut local: Vec<Hit> = Vec::new();
                for j in part {
                    scanned.fetch_add(1, Ordering::Relaxed);
                    let raw = match std::fs::read(&j.path) {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    // stage 1: byte-level prefilter, no decode, no parse
                    if !m.bytes_possible(&raw) {
                        skipped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let text = match String::from_utf8(raw) {
                        Ok(t) => t,
                        Err(e) => String::from_utf8_lossy(e.as_bytes()).to_string(),
                    };
                    // stage 2: full parse only for survivors
                    let rec = match Record::parse_str(&text, &j.path, j.lib) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    if rec.status() == "DELETED" && !opts.include_deleted {
                        continue;
                    }
                    if j.lib == Lib::Candidate
                        && !opts.include_promoted
                        && (rec.promotion.is_some() || !rec.get("PROMOTED_TO").is_empty())
                    {
                        continue;
                    }
                    if let Some(h) = score_record(&m, &rec, root, opts) {
                        local.push(h);
                    }
                }
                if !local.is_empty() {
                    out.lock().unwrap().extend(local);
                }
            });
        }
    });

    let mut hits = out.into_inner().unwrap();
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.cmp(&b.path)));
    if opts.min_score > 0 {
        hits.retain(|h| h.score >= opts.min_score);
    }
    if opts.max_hits > 0 && hits.len() > opts.max_hits {
        hits.truncate(opts.max_hits);
    }
    Ok(hits)
}

fn snippet_for(text: &str, pos: usize, ctx: usize) -> (String, usize) {
    // Clamp to a char boundary; a stray byte offset must never panic.
    let mut pos = pos.min(text.len());
    while pos > 0 && !text.is_char_boundary(pos) {
        pos -= 1;
    }
    let bytes = text.as_bytes();
    let line_start = text[..pos.min(text.len())]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let line_end = text[pos.min(text.len())..]
        .find('\n')
        .map(|i| pos.min(text.len()) + i)
        .unwrap_or(text.len());
    let mut lo = line_start;
    let mut hi = line_end;
    for _ in 0..ctx {
        if lo > 0 {
            lo = text[..lo - 1].rfind('\n').map(|i| i + 1).unwrap_or(0);
        }
        if hi < text.len() {
            hi = text[hi + 1..]
                .find('\n')
                .map(|i| hi + 1 + i)
                .unwrap_or(text.len());
        }
    }
    let _ = bytes;
    let line_no = text[..pos.min(text.len())].matches('\n').count() + 1;
    (truncate_chars(text[lo..hi].trim(), 300), line_no)
}

fn score_record(m: &Matcher, rec: &Record, root: &Path, opts: &Opts) -> Option<Hit> {
    let mut best: Option<(i32, String, String, usize)> = None; // score, where, snippet, line

    if opts.raw_lines {
        // Match the reconstructed `KEY=value` lines so anchors and field-name
        // patterns behave the way a reader expects.
        for (k, v) in &rec.fields {
            let line = format!("{k}={v}");
            if let Some((base, pos)) = m.text_matches(&line) {
                let sc = base + field_weight(k);
                if best.as_ref().map(|(b, _, _, _)| sc > *b).unwrap_or(true) {
                    let (snip, ln) = snippet_for(&line, pos, 0);
                    best = Some((sc, k.clone(), snip, ln));
                }
            }
        }
        if best.is_none() {
            // fall back to the raw rendered text, which also covers body tags
            if let Some((base, pos)) = m.text_matches(&rec.raw) {
                let (snip, ln) = snippet_for(&rec.raw, pos, opts.context);
                best = Some((base, "RAW".to_string(), snip, ln));
            }
        }
        let (score, where_, snippet, line) = best?;
        return Some(Hit {
            path: rel(root, &rec.path),
            title: rec.title(),
            score,
            where_,
            snippet,
            line,
            updated: rec.updated(),
            bytes: rec.raw.len(),
            lines: rec.raw.lines().count(),
        });
    }

    let body_only = opts.body_only;
    if !body_only {
        for (k, v) in &rec.fields {
            if !opts.fields.is_empty()
                && !opts.fields.iter().any(|f| {
                    let (c, _) = crate::parse::canon_key(f);
                    c == *k
                })
            {
                continue;
            }
            if let Some((base, pos)) = m.text_matches(v) {
                let sc = base + field_weight(k);
                if best.as_ref().map(|(b, _, _, _)| sc > *b).unwrap_or(true) {
                    let (snip, line) = snippet_for(v, pos, 0);
                    best = Some((sc, k.clone(), snip, line));
                }
            }
        }
    }
    // body
    if let Some((base, pos)) = m.text_matches(&rec.body) {
        let sc = base + 25;
        if best.as_ref().map(|(b, _, _, _)| sc > *b).unwrap_or(true) {
            let (snip, line) = snippet_for(&rec.body, pos, opts.context);
            best = Some((sc, "BODY".to_string(), snip, line));
        }
    }
    let (score, where_, snippet, line) = best?;
    Some(Hit {
        path: rel(root, &rec.path),
        title: rec.title(),
        score,
        where_,
        snippet,
        line,
        updated: rec.updated(),
        bytes: rec.raw.len(),
        lines: rec.raw.lines().count(),
    })
}

