//! Built-in format validation.
//!
//! The tool owns the contract; the model is not trusted to remember it. Every
//! write and every read can be validated, and each issue says what to do about
//! it. Issues are graded so a caller can decide: block, warn, or fix.

use crate::parse::{Deviation, Parsed, Sev};
use crate::rec::Lib;

#[derive(Clone, Debug)]
pub struct Issue {
    pub rule: &'static str,
    pub sev: Sev,
    pub field: String,
    pub message: String,
    pub fix: String,
}

impl Issue {
    pub fn show(&self) -> String {
        let f = if self.field.is_empty() {
            "-".to_string()
        } else {
            self.field.clone()
        };
        format!(
            "[{}] {} {} :: {}  → 修复: {}",
            self.sev.label(),
            self.rule,
            f,
            self.message,
            self.fix
        )
    }
}

pub const KINDS: &[&str] = &[
    "CONSTRAINT",
    "OBSERVATION",
    "DECISION",
    "ATTEMPT",
    "FAILURE",
    "CORRECTION",
    "EVIDENCE",
    "RESULT",
    "HYPOTHESIS",
];

pub const MATURITIES: &[&str] = &[
    "OBSERVED",
    "HYPOTHESIS",
    "TESTED",
    "DISPROVED",
    "PARTIAL",
    "VERIFIED",
    "USER_EXPLICIT",
];

pub const AUTHORITIES: &[&str] = &[
    "USER_EXPLICIT",
    "USER_FEEDBACK",
    "TOOL_EVIDENCE",
    "MODEL_DERIVED",
    "UNKNOWN",
];

pub const STATUSES: &[&str] = &["ACTIVE", "DELETED", "SUPERSEDED", "CONFLICT", "PROMOTED", "PARTIAL"];

pub const SIZES: &[&str] = &["SMALL", "MEDIUM", "LARGE"];

/// Fields that must exist and be non-empty for a usable record.
/// Without these a record cannot be identified or recalled at all.
pub const REQUIRED: &[&str] = &["ID", "TITLE"];

/// Without these a record is still readable, but recall quality suffers.
/// Kept as WARN, not ERROR: a legacy record with no READ_WHEN is not broken,
/// it is merely weaker, and must stay usable.
pub const EXPECTED: &[&str] = &[
    "FORMAT", "KIND", "MATURITY", "AUTHORITY", "SUMMARY", "READ_WHEN", "NOT_FOR",
];

/// Fields that must exist in one of several acceptable spellings.
/// KEYS arrived after the first records were written, so CONCEPTS is an
/// accepted stand-in rather than a defect.
pub const EITHER: &[(&str, &[&str])] = &[("KEYS", &["KEYS", "CONCEPTS"])];

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub issues: Vec<Issue>,
    pub devs: Vec<Deviation>,
}

impl Report {
    pub fn has_error(&self) -> bool {
        self.issues.iter().any(|i| i.sev == Sev::Error)
            || self.devs.iter().any(|d| d.sev == Sev::Error)
    }
    pub fn warn_count(&self) -> usize {
        self.issues.iter().filter(|i| i.sev == Sev::Warn).count()
            + self.devs.iter().filter(|d| d.sev == Sev::Warn).count()
    }
    pub fn fixed_count(&self) -> usize {
        self.devs.iter().filter(|d| d.sev == Sev::Fixed).count()
    }
    pub fn clean(&self) -> bool {
        !self.has_error() && self.warn_count() == 0 && self.fixed_count() == 0
    }
    pub fn summary(&self) -> String {
        format!(
            "ERROR {} / WARN {} / 自动纠偏 {}",
            self.issues.iter().filter(|i| i.sev == Sev::Error).count()
                + self.devs.iter().filter(|d| d.sev == Sev::Error).count(),
            self.warn_count(),
            self.fixed_count()
        )
    }
}

fn in_set(v: &str, set: &[&str]) -> bool {
    set.iter().any(|x| x.eq_ignore_ascii_case(v))
}

/// Validate a parsed record against the schema.
pub fn validate(p: &Parsed, lib: Lib, filename: Option<&str>) -> Report {
    let mut r = Report::default();
    r.devs = p.devs.clone();
    let mut add = |rule: &'static str, sev: Sev, field: &str, msg: String, fix: &str| {
        r.issues.push(Issue {
            rule,
            sev,
            field: field.to_string(),
            message: msg,
            fix: fix.to_string(),
        });
    };

    // ---- required fields
    for (want, accept) in EITHER {
        let have = accept.iter().any(|k| !p.get(k).trim().is_empty());
        if !have {
            add(
                "S004",
                Sev::Error,
                want,
                format!("缺 {} 且无可替代字段", accept.join("/")),
                &format!("补 {want}=..."),
            );
        } else if p.get(want).trim().is_empty() {
            let src = accept
                .iter()
                .find(|k| !p.get(k).trim().is_empty())
                .copied()
                .unwrap_or("CONCEPTS");
            add(
                "S005",
                Sev::Warn,
                want,
                format!("缺 {want}，可由 {src} 派生"),
                &format!("`abook repair --apply` 从 {src} 派生"),
            );
        }
    }
    for k in REQUIRED {
        if !p.has(k) || p.get(k).trim().is_empty() {
            add(
                "S002",
                Sev::Error,
                k,
                "缺标识性必填字段，记录无法被可靠识别".into(),
                &format!("补 {k}=..."),
            );
        }
    }
    for k in EXPECTED {
        if !p.has(k) || p.get(k).trim().is_empty() {
            let fix = if *k == "FORMAT" {
                "`abook repair --apply` 可自动补".to_string()
            } else {
                format!("补 {k}=... 以提升召回质量")
            };
            add(
                "S003",
                Sev::Warn,
                k,
                "缺元数据字段（记录仍可读，召回质量下降）".into(),
                &fix,
            );
        }
    }

    // ---- enum fields
    let kind = p.get("KIND");
    if !kind.trim().is_empty() && !in_set(&kind, KINDS) {
        add(
            "S010",
            Sev::Warn,
            "KIND",
            format!("不在枚举内: {kind}"),
            &format!("改为 {}", KINDS.join("/")),
        );
    }
    let mat = p.get("MATURITY");
    if !mat.trim().is_empty() && !in_set(&mat, MATURITIES) {
        add(
            "S011",
            Sev::Warn,
            "MATURITY",
            format!("不在枚举内: {mat}"),
            &format!("改为 {}", MATURITIES.join("/")),
        );
    }
    let auth = p.get("AUTHORITY");
    if !auth.trim().is_empty() && !in_set(&auth, AUTHORITIES) {
        add(
            "S012",
            Sev::Warn,
            "AUTHORITY",
            format!("不在枚举内: {auth}"),
            &format!("改为 {}", AUTHORITIES.join("/")),
        );
    }
    if lib == Lib::Canonical {
        let st = p.get("STATUS");
        if st.trim().is_empty() {
            add("S013", Sev::Warn, "STATUS", "正式记录缺 STATUS".into(), "补 STATUS=ACTIVE");
        } else if !in_set(&st, STATUSES) {
            add("S014", Sev::Warn, "STATUS", format!("不在枚举内: {st}"), "改为 ACTIVE");
        }
    }
    let sz = p.get("SIZE");
    if !sz.trim().is_empty() && !in_set(&sz, SIZES) {
        add("S015", Sev::Warn, "SIZE", format!("不在枚举内: {sz}"), "改为 SMALL/MEDIUM/LARGE");
    }

    // ---- ID and filename
    let id = p.get("ID");
    let id_first = id.split(';').next().unwrap_or("").trim().to_string();
    if !id_first.is_empty() {
        if id_first.contains(char::is_whitespace) {
            add("S020", Sev::Warn, "ID", "ID 含空白".into(), "去掉空白");
        }
        // The filename is a locator, not an ID source: 3 naming schemes
        // coexist and 3 files carry non-hex hash tails. Only a file already in
        // the current scheme is expected to agree with its header ID.
        if let Some(f) = filename {
            let stem = f.trim_end_matches(".md");
            if !stem.is_empty()
                && stem != id_first
                && !id.contains(';')
                && crate::rec::Record::is_scheme_name(f)
            {
                add(
                    "S021",
                    Sev::Warn,
                    "ID",
                    format!("ID({id_first}) 与文件名({stem}) 不一致"),
                    "用 `abook repair --sync-id` 对齐",
                );
            }
        }
    }

    // ---- body
    if p.body.trim().is_empty() {
        add("S030", Sev::Error, "MEMORY_BODY", "正文为空".into(), "补正文内容");
    } else if p.body.trim().chars().count() < 10 {
        add("S031", Sev::Warn, "MEMORY_BODY", "正文过短".into(), "确认是否遗漏内容");
    }

    // ---- list-ish fields
    for k in ["KEYS", "CONCEPTS", "ENTITIES", "RELATED_IDS"] {
        let v = p.get(k);
        if v.contains(';') && !v.contains('；') {
            add(
                "S040",
                Sev::Warn,
                k,
                "列表字段用了分号".into(),
                "列表用逗号分隔，分号会被当作字段分隔符",
            );
        }
    }

    // ---- title sanity
    let t = p.get("TITLE");
    if t.chars().count() > 200 {
        add("S050", Sev::Warn, "TITLE", "标题过长".into(), "压到 200 字内");
    }
    if t.contains('\n') {
        add("S051", Sev::Warn, "TITLE", "标题含换行".into(), "改成单行");
    }

    r
}

/// Apply the fixes the validator can perform without judgement.
/// Returns the list of things changed.
pub fn autofix(p: &mut Parsed, lib: Lib) -> Vec<String> {
    let mut done = Vec::new();
    if p.get("FORMAT").trim().is_empty() {
        let v = if lib == Lib::Canonical {
            "CANONICAL_MEMORY_V2"
        } else {
            "CANDIDATE_MEMORY_V2"
        };
        set(p, "FORMAT", v);
        done.push(format!("FORMAT={v}"));
    }
    // enum casing
    for (k, allowed) in [
        ("KIND", KINDS),
        ("MATURITY", MATURITIES),
        ("AUTHORITY", AUTHORITIES),
        ("STATUS", STATUSES),
        ("SIZE", SIZES),
    ] {
        let v = p.get(k);
        if v.trim().is_empty() {
            continue;
        }
        if let Some(hit) = allowed.iter().find(|x| x.eq_ignore_ascii_case(v.trim())) {
            if *hit != v {
                set(p, k, hit);
                done.push(format!("{k}={hit}"));
            }
        }
    }
    if lib == Lib::Canonical && p.get("STATUS").trim().is_empty() {
        set(p, "STATUS", "ACTIVE");
        done.push("STATUS=ACTIVE".into());
    }
    // SEMICOLON lists -> commas, but only when it cannot be a field separator
    // Only the ASCII semicolon is structural; the full-width one is content.
    for k in ["KEYS", "CONCEPTS", "ENTITIES", "RELATED_IDS"] {
        let v = p.get(k);
        if v.contains(';') {
            let fixed = v.replace(';', ",");
            set(p, k, &fixed);
            done.push(format!("{k} 半角分号改逗号"));
        }
    }
    // trim values
    for (_, v) in p.fields.iter_mut() {
        let t = v.trim().to_string();
        if *v != t {
            *v = t;
        }
    }
    // derive SIZE from body length when missing
    if p.get("SIZE").trim().is_empty() {
        let n = p.body.len();
        let v = size_for(n);
        set(p, "SIZE", v);
        done.push(format!("SIZE={v}"));
    }
    // KEYS and CONCEPTS stand in for each other
    if p.get("KEYS").trim().is_empty() && !p.get("CONCEPTS").trim().is_empty() {
        let v = p.get("CONCEPTS");
        set(p, "KEYS", &v);
        done.push("KEYS 由 CONCEPTS 派生".into());
    }
    if p.get("CONCEPTS").trim().is_empty() && !p.get("KEYS").trim().is_empty() {
        let v = p.get("KEYS");
        set(p, "CONCEPTS", &v);
        done.push("CONCEPTS 由 KEYS 派生".into());
    }
    done
}

/// Body length -> SIZE bucket. Shared so write and re-derive agree.
pub fn size_for(n: usize) -> &'static str {
    if n < 2048 {
        "SMALL"
    } else if n < 8192 {
        "MEDIUM"
    } else {
        "LARGE"
    }
}

fn set(p: &mut Parsed, k: &str, v: &str) {
    if let Some(e) = p.fields.iter_mut().find(|(kk, _)| kk == k) {
        e.1 = v.to_string();
    } else {
        p.fields.push((k.to_string(), v.to_string()));
    }
}
