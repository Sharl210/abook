//! Shared helpers: errors, arg parsing, paths, time formatting, fs utilities.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- errors

#[derive(Debug)]
pub enum EE {
    Usage(String),
    NotFound(String),
    Blocked(String),
    Integrity(String),
    Io(String),
    Other(String),
}

pub type R<T> = Result<T, EE>;

impl From<io::Error> for EE {
    fn from(e: io::Error) -> Self {
        EE::Io(e.to_string())
    }
}

impl EE {
    pub fn code(&self) -> i32 {
        match self {
            EE::Usage(_) => 2,
            EE::NotFound(_) => 3,
            EE::Blocked(_) => 4,
            EE::Integrity(_) => 5,
            EE::Io(_) => 6,
            EE::Other(_) => 1,
        }
    }
    pub fn msg(&self) -> &str {
        match self {
            EE::Usage(m)
            | EE::NotFound(m)
            | EE::Blocked(m)
            | EE::Integrity(m)
            | EE::Io(m)
            | EE::Other(m) => m,
        }
    }
    pub fn tag(&self) -> &'static str {
        match self {
            EE::Usage(_) => "USAGE",
            EE::NotFound(_) => "NOT_FOUND",
            EE::Blocked(_) => "BLOCKED",
            EE::Integrity(_) => "INTEGRITY",
            EE::Io(_) => "IO",
            EE::Other(_) => "ERROR",
        }
    }
}

pub fn usage<T>(m: impl Into<String>) -> R<T> {
    Err(EE::Usage(m.into()))
}
pub fn blocked<T>(m: impl Into<String>) -> R<T> {
    Err(EE::Blocked(m.into()))
}
pub fn notfound<T>(m: impl Into<String>) -> R<T> {
    Err(EE::NotFound(m.into()))
}
pub fn integrity<T>(m: impl Into<String>) -> R<T> {
    Err(EE::Integrity(m.into()))
}

// ---------------------------------------------------------------- args

/// Boolean switches that never consume the following token.

/// Sorted list of every value-taking parameter name in the tool. Kept sorted so
/// `known()` can binary-search it; a stale entry means a legitimate parameter
/// gets rejected, so this list is exercised by selftest.
pub const KNOWN_PARAMS: &[&str] = &[
    "after", "authority", "branch", "by", "concepts", "confidence", "content",
    "depth", "dest", "dir", "edits", "entities", "evidence", "field",
    "file", "for", "format", "from", "id", "ids", "into", "json-depth", "keep",
    "keys", "kind", "lib", "match", "maturity", "max", "mode", "name", "next",
    "not-for", "page", "path", "per-page", "project", "query", "read-when",
    "reason", "related", "root", "scope", "session", "since", "source",
    "status", "summary", "tag", "target", "task", "tier", "title", "to",
    "token", "url", "value", "with", "within",
];

const BOOLS: &[&str] = &[
    // A flag listed here never consumes the following token. Anything used as
    // `a.has(...)` MUST be here, otherwise a bare flag would swallow the next
    // positional argument — which is how `--semantic 查询词` used to lose its
    // query entirely.
    // output / behaviour
    "json", "quiet", "raw", "compact", "verbose", "help",
    // authorisation and safety
    "approve", "yes", "force", "hard", "dry-run", "apply",
    // scope switches
    "all", "include-promoted", "include-deleted", "hide-promoted",
    "no-expand", "no-index", "no-atlas", "no-commit",
    "summary-only", "purpose-only", "deviations", "fields",
    "remove-sources", "rebuild", "fix", "verify", "prune",
    // search modes
    "regex", "re", "fuzzy", "approx", "literal", "exact",
    "semantic", "sem", "ask", "case", "deep", "expand", "raw-lines", "lines", "whole-line",
];

#[derive(Debug, Default)]
pub struct Args {
    pub cmd: String,
    pub pos: Vec<String>,
    pub kv: BTreeMap<String, Vec<String>>,
}

impl Args {
    /// Fold the parameter spellings an assistant is likely to reach for onto
    /// the ones this tool actually reads. Silently ignoring `--limit` while
    /// returning the whole library is the worst possible outcome: the caller
    /// believes it paginated and gets 507 rows. Aliases remove the guesswork.
    fn alias(k: &str) -> &str {
        match k {
            "limit" | "size" | "per_page" | "perpage" | "count" | "n" => "per-page",
            "offset" => "page",
            "p" => "page",
            other => other,
        }
    }

    pub fn parse(argv: &[String]) -> R<Args> {
        let mut a = Args::default();
        let mut i = 0;
        if argv.is_empty() {
            return usage("需要子命令，运行 `abook help` 查看用法");
        }
        a.cmd = argv[0].clone();
        i += 1;
        while i < argv.len() {
            let t = &argv[i];
            if let Some(rest) = t.strip_prefix("--") {
                let (k, inline) = match rest.split_once('=') {
                    Some((k, v)) => (k.to_string(), Some(v.to_string())),
                    None => (rest.to_string(), None),
                };
                let k = Self::alias(&k).to_string();
                if BOOLS.contains(&k.as_str()) {
                    let v = inline.unwrap_or_else(|| "true".to_string());
                    a.kv.entry(k).or_default().push(v);
                } else if let Some(v) = inline {
                    a.kv.entry(k).or_default().push(v);
                } else {
                    // value follows, unless next token is another flag
                    if i + 1 < argv.len() && !argv[i + 1].starts_with("--") {
                        let v = argv[i + 1].clone();
                        a.kv.entry(k).or_default().push(v);
                        i += 1;
                    } else {
                        a.kv.entry(k).or_default().push(String::new());
                    }
                }
            } else if let Some(rest) = t.strip_prefix('-') {
                if rest == "h" {
                    a.kv.entry("help".into()).or_default().push("true".into());
                } else {
                    return usage(format!("未知短选项 -{rest}"));
                }
            } else {
                a.pos.push(t.clone());
            }
            i += 1;
        }
        Ok(a)
    }

    /// Every parameter name the tool understands anywhere. A name outside this
    /// set is almost always a typo, and a typo used to be ignored silently --
    /// `--limt 5` returned the whole library and looked like success.
    pub fn known(k: &str) -> bool {
        KNOWN_PARAMS.binary_search(&k).is_ok() || BOOLS.contains(&k)
    }

    /// All recognised parameter names, for typo suggestions.
    pub fn all_known() -> impl Iterator<Item = &'static str> {
        KNOWN_PARAMS.iter().copied().chain(BOOLS.iter().copied())
    }

    pub fn has(&self, k: &str) -> bool {
        self.kv.contains_key(k)
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.kv.get(k).and_then(|v| v.last()).map(|s| s.as_str())
    }
    pub fn req(&self, k: &str) -> R<&str> {
        match self.get(k) {
            Some(v) if !v.is_empty() => Ok(v),
            _ => usage(format!("缺少必填参数 --{k}")),
        }
    }
    pub fn all(&self, k: &str) -> Vec<String> {
        self.kv.get(k).cloned().unwrap_or_default()
    }
    pub fn num(&self, k: &str) -> Option<u64> {
        self.get(k).and_then(|v| v.parse().ok())
    }
    pub fn pos_or(&self, i: usize, d: &str) -> String {
        self.pos.get(i).cloned().unwrap_or_else(|| d.to_string())
    }
    pub fn need_pos(&self, i: usize, what: &str) -> R<&str> {
        match self.pos.get(i) {
            Some(s) => Ok(s.as_str()),
            None => usage(format!("缺少参数：{what}")),
        }
    }
}

// ---------------------------------------------------------------- time

/// Civil date from days since Unix epoch (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub struct Stamps {
    pub compact: String, // YYYYMMDDTHHMMSS
    pub iso_z: String,   // 2026-10-22T05:47:39Z
    pub date_only: String,
}

pub fn now_secs() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    }
}

/// UTC offset in seconds, best effort. Unix: ask `date +%z`. Otherwise 0.
pub fn local_offset_secs() -> i64 {
    if let Ok(off) = std::env::var("ABOOK_UTC_OFFSET") {
        if let Ok(n) = off.parse::<i64>() {
            return n;
        }
    }
    #[cfg(unix)]
    {
        if let Ok(out) = std::process::Command::new("date").arg("+%z").output() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.len() == 5 {
                let sign = if s.starts_with('-') { -1 } else { 1 };
                let h: i64 = s[1..3].parse().unwrap_or(0);
                let m: i64 = s[3..5].parse().unwrap_or(0);
                return sign * (h * 3600 + m * 60);
            }
        }
    }
    0
}

pub fn stamps_at(secs: i64) -> Stamps {
    let off = local_offset_secs();
    let l = secs + off;
    let days = l.div_euclid(86400);
    let sod = l.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    let udays = secs.div_euclid(86400);
    let usod = secs.rem_euclid(86400);
    let (uy, um, ud) = civil_from_days(udays);
    Stamps {
        compact: format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}"),
        iso_z: format!(
            "{uy:04}-{um:02}-{ud:02}T{:02}:{:02}:{:02}Z",
            usod / 3600,
            (usod % 3600) / 60,
            usod % 60
        ),
        date_only: format!("{y:04}-{m:02}-{d:02}"),
    }
}

pub fn stamps() -> Stamps {
    stamps_at(now_secs())
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` or `YYYYMMDDTHHMMSS` into epoch seconds (UTC).
pub fn parse_stamp(s: &str) -> Option<i64> {
    // Accepts the three spellings that occur in real records:
    //   2026-10-01T20:11:23+08:00[Asia/Shanghai]
    //   2026-09-27 13:15:47 +08:00
    //   20260927T131547
    let raw = s.trim();
    // strip a trailing [Zone] annotation
    let t0 = match raw.split_once('[') {
        Some((head, _)) => head.trim(),
        None => raw,
    };
    let t0 = t0.trim_end_matches('Z');
    let (date, time) = if let Some((d, t)) = t0.split_once('T') {
        (d, t)
    } else if let Some((d, t)) = t0.split_once(' ') {
        (d, t.trim())
    } else {
        return None;
    };
    // drop a numeric UTC offset that stuck to the time
    let time = {
        let mut cut = time.len();
        for (i, c) in time.char_indices() {
            if (c == '+' || c == '-') && i > 0 {
                cut = i;
                break;
            }
        }
        &time[..cut]
    };
    let time = time.trim();
    let (y, mo, d) = if date.contains('-') {
        let p: Vec<&str> = date.split('-').collect();
        if p.len() != 3 {
            return None;
        }
        (
            p[0].parse::<i64>().ok()?,
            p[1].parse::<u32>().ok()?,
            p[2].parse::<u32>().ok()?,
        )
    } else if date.len() == 8 {
        (
            date[0..4].parse::<i64>().ok()?,
            date[4..6].parse::<u32>().ok()?,
            date[6..8].parse::<u32>().ok()?,
        )
    } else {
        return None;
    };
    let tp: Vec<&str> = time.split(':').collect();
    let (h, mi, se) = if tp.len() == 3 {
        (
            tp[0].parse::<i64>().ok()?,
            tp[1].parse::<i64>().ok()?,
            tp[2].split('.').next()?.parse::<i64>().ok()?,
        )
    } else if time.len() == 6 {
        (
            time[0..2].parse::<i64>().ok()?,
            time[2..4].parse::<i64>().ok()?,
            time[4..6].parse::<i64>().ok()?,
        )
    } else {
        return None;
    };
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se)
}

// ---------------------------------------------------------------- paths

pub fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

pub fn home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    PathBuf::from(".")
}

/// Memory book root: --root > $ABOOK_ROOT > $HOME/AgentsBook
pub fn resolve_root(a: &Args) -> PathBuf {
    if let Some(r) = a.get("root") {
        if !r.is_empty() {
            return expand_tilde(r);
        }
    }
    let e = env_or("ABOOK_ROOT", "");
    if !e.is_empty() {
        return expand_tilde(&e);
    }
    home_dir().join("AgentsBook")
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        home_dir().join(rest)
    } else if p == "~" {
        home_dir()
    } else {
        PathBuf::from(p)
    }
}

// ---------------------------------------------------------------- fs

pub fn read_str(p: &Path) -> R<String> {
    match fs::read(p) {
        Ok(b) => Ok(String::from_utf8_lossy(&b).to_string()),
        Err(e) => Err(EE::Io(format!("读取失败 {}: {e}", p.display()))),
    }
}

pub fn write_atomic(p: &Path, data: &str) -> R<()> {
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension(format!(
        "tmp{}",
        std::process::id() as u64 % 100000
    ));
    fs::write(&tmp, data.as_bytes())
        .map_err(|e| EE::Io(format!("写入失败 {}: {e}", tmp.display())))?;
    // read back: a lost write must not be reported as success
    let back = fs::read(&tmp)?;
    if back != data.as_bytes() {
        let _ = fs::remove_file(&tmp);
        return integrity(format!("写入复读不一致: {}", p.display()));
    }
    match fs::rename(&tmp, p) {
        Ok(_) => Ok(()),
        Err(_) => {
            // Windows: rename over existing file needs the target removed first
            let _ = fs::remove_file(p);
            fs::rename(&tmp, p).map_err(|e| {
                EE::Io(format!("原子改名失败 {}: {e}", p.display()))
            })
        }
    }
}

pub fn ensure_dir(p: &Path) -> R<()> {
    fs::create_dir_all(p).map_err(|e| EE::Io(format!("创建目录失败 {}: {e}", p.display())))
}

pub fn list_md(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "md").unwrap_or(false) {
                v.push(p);
            }
        }
    }
    v.sort();
    v
}

pub fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn human_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n}B")
    } else if n < 1024 * 1024 {
        format!("{:.1}KB", n as f64 / 1024.0)
    } else {
        format!("{:.1}MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// Truncate a string to at most `n` chars, appending an ellipsis marker.
pub fn truncate_chars(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut c = 0;
    for ch in s.chars() {
        if c >= n {
            out.push('…');
            return out;
        }
        out.push(ch);
        c += 1;
    }
    out
}

// ---------------------------------------------------------------- json

pub fn jesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

pub fn jstr(s: &str) -> String {
    format!("\"{}\"", jesc(s))
}

/// Glob-free simple matcher: supports `*` and `?`.
pub fn wildcard_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Case-insensitive substring test.
pub fn ci_contains(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.to_lowercase().contains(&needle.to_lowercase())
}

/// Split a value list on `,` or `、` and trim.
pub fn split_list(s: &str) -> Vec<String> {
    s.split(|c| c == ',' || c == '，' || c == '、' || c == ';')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// The summary the tool derives from a body when the caller supplies none.
/// Kept as one function so edits can tell "still the derived value" from
/// "the author wrote this".
pub fn derive_summary(body: &str) -> String {
    truncate_chars(body.trim(), 160)
}

/// Re-derive body-dependent fields, but only where they still hold the
/// derived value. An authored SUMMARY or SIZE is never overwritten.
pub fn rederive_after_body_change(rec: &mut crate::rec::Record, old_body: &str) {
    let new_body = rec.body.clone();
    if rec.get("SUMMARY") == derive_summary(old_body) {
        let v = derive_summary(&new_body);
        rec.set("SUMMARY", &v);
    }
    // SIZE follows the same rule.
    let old_size = crate::schema::size_for(old_body.len());
    if rec.get("SIZE") == old_size {
        let v = crate::schema::size_for(new_body.len());
        rec.set("SIZE", v);
    }
}
