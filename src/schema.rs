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
    "FORMAT", "KIND", "MATURITY", "AUTHORITY", "SUMMARY", "BRIEF", "READ_WHEN", "NOT_FOR",
];

/// `BRIEF` 的硬上限（字符）。超了就当没写。
///
/// 它的用途只有一个：**预加载平面里那一行**。所以它必须是登记时写好的一句
/// 完整短话，而不是把 SUMMARY 截断——截断出来的东西读着像半截话
/// （"在输入法进程里，PopupWindow 与 type=0x3eb 的附加 …"），
/// 既占地方又没传达意思。
///
/// 100 字是平面里一条记录的省略界限。定成 100 而不是更短，是因为这个长度
/// 足够把"做了什么、为什么、结论是什么"讲完，读者不必点进全文就能判断这条
/// 跟自己当前这步有没有关系。
///
/// 反过来，正文本身不超过 100 字的记录不写 BRIEF——全文已经在界限内，再压
/// 一遍只会丢信息。这类记录 `derive_brief` 返回 `None`，平面直接显示正文。
pub const BRIEF_MAX_CHARS: usize = 100;

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
            "ERROR {} / WARN {} / 内存归一 {}",
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
/// `only` 限定只做哪一类修复；空表示全做。
///
/// 为什么要这个：`repair` 原本一执行就是"能修的全修"，于是想修 3 条记录的
/// 引用前缀，顺手给 248 条加上了 `SIZE`、给 242 条从 `CONCEPTS` 派生了 `KEYS`。
/// 那些派生值本身没算错，但**用户没有要求**——一次核对变成一次批量改写，而且
/// 改的是真库（没有版本控制，没有备份）。
///
/// 批量写入必须能把范围说清楚，否则"它会修什么"就只能靠读完整个 autofix 猜。
/// `--only` 让调用方表达意图，也让日志与改动一一对应。
///
/// 注意每个类别必须连 `set()` 一起跳过，不能只过滤日志——写盘是按 `p.fields`
/// 全量落地的，只不记录的话字段照样被改。
fn wanted(only: &[String], tag: &str) -> bool {
    only.is_empty() || only.iter().any(|x| x == tag)
}

/// 可用类别，供 `--only` 提示。
pub const FIX_CLASSES: &[(&str, &str)] = &[
    ("id", "ID 缺失时补文件名（已有则不动）"),
    ("format", "补 FORMAT"),
    ("enum", "KIND/MATURITY/AUTHORITY/STATUS/SIZE 枚举大小写归正"),
    ("refs", "引用值还原为规范身份"),
    ("sep", "列表字段的半角分号改为逗号"),
    ("trim", "字段值去首尾空白"),
    ("size", "按正文长度补 SIZE"),
    ("keys", "KEYS 与 CONCEPTS 互为派生"),
];

pub fn autofix(p: &mut Parsed, lib: Lib, filename: Option<&str>, only: &[String]) -> Vec<String> {
    let mut done = Vec::new();

    // ID 只在**缺失**时补，绝不在已有时覆盖。
    //
    // 这里曾经是「文件名合规范、ID 不合规范就覆盖成文件名」，而 `is_scheme_name`
    // 只认 `N-<15位时间戳>-<8位hex>` 一种形态。正式库的 523 条记录用的是
    // `AB2-IMS-051` 这类逻辑 ID，全部被判成"不合规范"，于是 `repair --only id
    // --apply` 一次把 523 个逻辑 ID 全换成了文件名——而地图里的条目仍写着旧 ID，
    // 所有 `RELATED_IDS` / `PROMOTED_FROM` 引用当场断裂。更糟的是三条自检
    // （state / reconcile / doctor）都报健康，因为它们查的是计数而不是 ID 形态。
    //
    // 现在的判据只有一条：ID 为空才补。ID 是人给它起的名字，工具没有资格替它改名。
    // 形态检查留给人用 `lint`/`audit` 看，不当修的依据——"看起来不像我们的格式"
    // 不等于"错了"，尤其当整个库都用这种格式的时候。
    //
    // 带分号的老式复合行（`ID=x;PROJECT=y;...`）不碰，那是旧库字段打包写法。
    if wanted(only, "id") {
        if let Some(f) = filename {
            let raw = p.get("ID");
            if !raw.contains(';') && raw.trim().is_empty() {
                let stem = f.trim_end_matches(".md");
                set(p, "ID", stem);
                done.push(format!("ID={stem}（原缺失，补文件名）"));
            }
        }
    }

    if wanted(only, "format") && p.get("FORMAT").trim().is_empty() {
        let v = if lib == Lib::Canonical {
            "CANONICAL_MEMORY_V2"
        } else {
            "CANDIDATE_MEMORY_V2"
        };
        set(p, "FORMAT", v);
        done.push(format!("FORMAT={v}"));
    }

    if wanted(only, "enum") {
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
    }

    // 引用值里的坏形态：从任意字符串里取出真正的身份。
    //
    // 判定纯语法，不需要看别的记录，所以不存在"猜错目标"的风险；而修复不了的
    // 那些（目标真被删了、指向旧库 References 路径）留着不动，由 `abook refs`
    // 报告——**能纯语法修的就修，需要判断的绝不猜**。
    if wanted(only, "refs") {
        for k in ["PROMOTED_FROM", "PROMOTED_TO", "SUPERSEDES", "PREVIOUS_ID", "RELATED_IDS"] {
            let v = p.get(k);
            if v.trim().is_empty() {
                continue;
            }
            let mut changed = false;
            let fixed: Vec<String> = v
                .split([',', '，'])
                .map(|x| x.trim())
                .filter(|x| !x.is_empty())
                .map(|one| match crate::rec::Record::canonical_ref(one) {
                    Some(good) if good != one => {
                        changed = true;
                        good
                    }
                    _ => one.to_string(),
                })
                .collect();
            if changed {
                set(p, k, &fixed.join(","));
                done.push(format!("{k} 引用值还原为规范身份"));
            }
        }
    }

    // 只有 ASCII 分号是结构性的；全角分号是内容，不动。
    if wanted(only, "sep") {
        for k in ["KEYS", "CONCEPTS", "ENTITIES", "RELATED_IDS"] {
            let v = p.get(k);
            if v.contains(';') {
                let fixed = v.replace(';', ",");
                set(p, k, &fixed);
                done.push(format!("{k} 半角分号改逗号"));
            }
        }
    }

    if wanted(only, "trim") {
        for (_, v) in p.fields.iter_mut() {
            let t = v.trim().to_string();
            if *v != t {
                *v = t;
            }
        }
    }

    if wanted(only, "size") && p.get("SIZE").trim().is_empty() {
        let v = size_for(p.body.len());
        set(p, "SIZE", v);
        done.push(format!("SIZE={v}"));
    }

    if wanted(only, "keys") {
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

/// 规则码 → 一句话含义。
///
/// 为什么需要这张表：`audit` 的汇总只印规则码（`S003`、`T008`、`K004`），
/// 而码本身不自解释。真实库里跑一次 `audit` 会看到
/// `S003 0/170/0`、`S010 0/28/0`，读的人得先知道 S003 是"缺某个元数据字段"、
/// S010 是"KIND 不在枚举内"才能判断这 170 条要不要管。把含义查出来本来该是
/// 工具的事，不该甩给读者去翻源码。
///
/// 收录不全时不假装知道：`rule_doc` 返回 None，调用方必须把"未收录说明"
/// 明确印出来。宁可显眼地缺一句话，也不要让一个码看起来像已经解释过了。
pub fn rule_doc(code: &str) -> Option<&'static str> {
    Some(match code {
        // ---- S：头部字段与内容规范
        "S002" => "缺标识性必填字段（如 ID/TITLE），记录无法被可靠识别",
        "S003" => "缺元数据字段（记录仍可读，但召回质量下降）",
        "S004" => "缺某个可接受的替代字段，且没有别的字段顶替",
        "S005" => "缺可派生字段，能从别的字段补出来",
        "S010" => "KIND 不在枚举内",
        "S011" => "MATURITY 不在枚举内",
        "S012" => "AUTHORITY 不在枚举内",
        "S013" => "正式记录缺 STATUS",
        "S014" => "STATUS 不在枚举内",
        "S015" => "SIZE 不在枚举内",
        "S020" => "ID 含空白字符",
        "S021" => "ID 与文件名不一致",
        "S030" => "正文为空",
        "S031" => "正文过短，可能漏了内容",
        "S040" => "列表字段用分号分隔（分号是字段分隔符，会被截断）",
        "S050" => "标题过长",
        "S051" => "标题含换行",
        // ---- T：标签与段落结构（偏差，多数已被自动修正）
        "T001" => "标签缺方括号，已按标签识别",
        "T002" => "只有闭合标签，按开启标签处理",
        "T003" => "缺头部标签，按首个 KEY= 行推断头部范围",
        "T004" => "头部标签未闭合，以正文起始标签为界",
        "T005" => "正文标签未闭合，取到文件末尾",
        "T006" => "缺正文标签，头部之后全文视为正文",
        "T007" => "晋升段未闭合，取到文件末尾",
        "T008" => "正文里出现标签字面量，已按首个闭合标签截取",
        "T009" => "多余的闭合标签（正文之后），已忽略",
        "T010" => "头部标签之前有字段行，已并入头部解析",
        // ---- K：字段键
        "K003" => "字段别名已归一为标准键",
        "K004" => "续行已接到上一字段（不是新字段）",
        "K005" => "头部内无法归属的行，已忽略",
        "K006" => "字段重复且取值相同，已去重",
        "K007" => "字段重复但取值不同，保留首个值",
        // ---- R / N：可读性与编码
        "R001" => "没有任何可识别字段，无法解析为记录",
        "R002" => "既无正文也无标题，无法解析为记录",
        "N001" => "文件带 UTF-8 BOM，已剥离",
        "N002" => "CRLF/CR 换行，已统一为 LF",
        _ => return None,
    })
}

#[cfg(test)]
mod rule_doc_tests {
    use super::rule_doc;

    /// 每个码都必须有自己的说明，且说明不能是空串。
    ///
    /// 这张表是审计读数的唯一解释来源；某条规则新增而忘了登记，`audit` 就会
    /// 重新退回"只印码不印意思"的老样子。这里把已知码逐个钉住。
    #[test]
    fn every_known_rule_has_a_description() {
        let all = [
            "S002", "S003", "S004", "S005", "S010", "S011", "S012", "S013", "S014", "S015",
            "S020", "S021", "S030", "S031", "S040", "S050", "S051", "T001", "T002", "T003",
            "T004", "T005", "T006", "T007", "T008", "T009", "T010", "K003", "K004", "K005",
            "K006", "K007", "R001", "R002", "N001", "N002",
        ];
        for c in all {
            let d = rule_doc(c).unwrap_or_else(|| panic!("规则码 {c} 没有说明"));
            assert!(!d.trim().is_empty(), "规则码 {c} 的说明是空的");
        }
        // 未收录的必须如实返回 None，不能编一句看起来像解释的话。
        assert!(rule_doc("ZZ999").is_none());
    }
}
